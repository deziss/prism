#!/usr/bin/env bash
# ==============================================================================
# remove-prism.sh — audit and remove every trace of PRISM from this machine.
#
# DEFAULT IS READ-ONLY. Running this with no arguments changes nothing: it
# reports what PRISM has left behind and exits non-zero if anything remains.
# Pass --remove to actually delete.
#
# WHY IT WORKS THIS WAY
# ---------------------
# The previous version removed by default and verified by grepping the same
# hardcoded paths it had just deleted. That made its final "OK" self-confirming:
# it reported a clean machine while an entire second IDE profile, a systemd unit,
# an MCP registration and a set of helper scripts were untouched. Audit and
# removal now share one detector, so the closing verdict cannot disagree with
# reality, and the audit is safe to run at any time to answer the only question
# that matters — "is it actually gone?"
#
# WHAT THIS SCRIPT CANNOT DO
# --------------------------
# It cannot repair a process that is already running. Environment is copied into
# a process at exec time; `unset` affects only the calling shell and
# `systemctl --user unset-environment` affects only units started afterwards. A
# long-lived login session keeps handing dead PRISM variables to everything it
# spawns until it ends. That is why removal appeared to fail for weeks: it
# succeeded every time, and the symptom lived in processes it could not touch.
# The detector lists those processes by PID so they can be restarted
# deliberately, and logging out clears all of them at once.
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ------------------------------------------------------------------------------
# Manifest — the single source of truth for everything PRISM writes.
# ------------------------------------------------------------------------------

MANIFEST="$SCRIPT_DIR/scripts/prism-manifest.sh"
if [[ ! -f "$MANIFEST" ]]; then
    echo "ERROR: manifest not found at $MANIFEST" >&2
    echo "Run this script from a PRISM checkout." >&2
    exit 2
fi
# shellcheck source=scripts/prism-manifest.sh
source "$MANIFEST"

# ------------------------------------------------------------------------------
# Arguments
# ------------------------------------------------------------------------------

MODE="audit"
ASSUME_YES=false
VERBOSE=false

usage() {
    cat <<'USAGE'
Usage: ./remove-prism.sh [--remove] [--yes]

  (no flags)   Audit only. Changes nothing. Exits non-zero if PRISM traces remain.
  --remove     Actually remove everything found. Prompts once before acting.
  --yes, -y    With --remove, skip the confirmation prompt.
  --verbose    List every affected process individually instead of a summary.
  --help, -h   This message.

Run the audit first. It reports every leftover and, importantly, every running
process still holding dead PRISM environment variables — those cannot be fixed by
any script and need the process restarted, or a logout.
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --remove) MODE="remove"; shift ;;
        --check|--audit|--dry-run) MODE="audit"; shift ;;
        -y|--yes) ASSUME_YES=true; shift ;;
        -v|--verbose) VERBOSE=true; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

# ------------------------------------------------------------------------------
# Safety: never run as root.
#
# Everything PRISM installs for a user lives under $HOME. Running this with sudo
# would resolve $HOME to root's and silently do nothing to the account that is
# actually affected. The one artifact that genuinely needs root — the system-wide
# trust anchor — is reported with the exact commands to paste, rather than this
# script escalating on its own.
# ------------------------------------------------------------------------------

if [[ "${EUID}" -eq 0 ]]; then
    echo "ERROR: do not run this script with sudo." >&2
    echo "Run: ./remove-prism.sh" >&2
    exit 2
fi

# ------------------------------------------------------------------------------
# Output helpers
# ------------------------------------------------------------------------------

if [[ -t 1 ]]; then
    C_RESET=$'\033[0m'; C_BOLD=$'\033[1m'; C_RED=$'\033[31m'
    C_GREEN=$'\033[32m'; C_YELLOW=$'\033[33m'; C_DIM=$'\033[2m'
else
    C_RESET=""; C_BOLD=""; C_RED=""; C_GREEN=""; C_YELLOW=""; C_DIM=""
fi

section() { printf '\n%s%s%s\n' "$C_BOLD" "$1" "$C_RESET"; }
found()   { printf '  %s✗%s %s\n' "$C_RED" "$C_RESET" "$1"; }
clean()   { printf '  %s✓%s %s\n' "$C_GREEN" "$C_RESET" "$1"; }
warn()    { printf '  %s!%s %s\n' "$C_YELLOW" "$C_RESET" "$1"; }
note()    { printf '    %s%s%s\n' "$C_DIM" "$1" "$C_RESET"; }
act()     { printf '  %s→%s %s\n' "$C_YELLOW" "$C_RESET" "$1"; }

# Findings that removal can fix. Incremented by each detector.
LEFTOVERS=0
# Findings that removal cannot fix (running processes, system CA needing root).
UNFIXABLE=0

# ==============================================================================
# DETECTORS — pure, read-only. Used identically by the audit and by the
# post-removal verification, so the two can never disagree.
# ==============================================================================

detect_binaries() {
    section "PRISM executables"
    local any=false f
    while IFS= read -r f; do
        [[ -z "$f" ]] && continue
        any=true
        LEFTOVERS=$((LEFTOVERS + 1))
        if [[ -L "$f" ]]; then
            found "$f -> $(readlink "$f")"
        else
            found "$f"
        fi
    done < <(prism_installed_binaries)
    $any || clean "no PRISM executables in $PRISM_BIN_DIR"

    # prism-env is the dangerous one: it survives removal and re-exports
    # SSL_CERT_FILE pointing at the bare CA, which replaces the trust store
    # instead of extending it. Running it once undoes a completed removal.
    if [[ -e "$PRISM_BIN_DIR/prism-env" ]]; then
        note "prism-env re-applies PRISM's environment on demand — a removed PRISM"
        note "stays removed only once this is gone."
    fi
}

detect_units() {
    section "systemd user units"
    local any=false f state
    while IFS= read -r f; do
        [[ -z "$f" ]] && continue
        any=true
        LEFTOVERS=$((LEFTOVERS + 1))
        state="$(systemctl --user is-active "$(basename "$f")" 2>/dev/null || true)"
        found "$f  [${state:-unknown}]"
        if grep -q 'Restart=always' "$f" 2>/dev/null; then
            note "Restart=always — this unit resurrects the daemon and regenerates"
            note "the CA even after the data directory is deleted."
        fi
    done < <(prism_unit_files)

    while IFS= read -r f; do
        [[ -z "$f" ]] && continue
        any=true
        LEFTOVERS=$((LEFTOVERS + 1))
        found "$f  [enable symlink]"
    done < <(prism_unit_wants_links)

    $any || clean "no PRISM systemd units"
}

detect_ports() {
    section "Listening ports"
    local any=false line port
    for port in "$PRISM_PROXY_PORT" "$PRISM_MCP_PORT"; do
        line="$(ss -ltnp 2>/dev/null | grep ":$port " || true)"
        [[ -z "$line" ]] && continue
        any=true
        LEFTOVERS=$((LEFTOVERS + 1))
        if [[ "$line" == *"0.0.0.0:$port"* ]]; then
            found "port $port listening on 0.0.0.0 (all interfaces, not loopback)"
            note "An intercepting proxy bound to 0.0.0.0 accepts connections from"
            note "the whole network, not just this machine."
        else
            found "port $port listening"
        fi
    done
    # The hub runs in Docker and is not part of a PRISM host install: worth
    # mentioning so the port is not mistaken for a leftover daemon, but this
    # script neither can nor should stop a container.
    line="$(ss -ltnp 2>/dev/null | grep ":$PRISM_HUB_PORT " || true)"
    if [[ -n "$line" ]]; then
        note "port $PRISM_HUB_PORT is open (prism-hub, normally Docker) — not a host install"
    fi
    $any || clean "ports $PRISM_PROXY_PORT and $PRISM_MCP_PORT closed"
}

detect_data() {
    section "Data and configuration directories"
    local any=false d
    for d in "$PRISM_DATA_DIR" "$PRISM_CONFIG_DIR" "$PRISM_STATE_DIR"; do
        if [[ -e "$d" ]]; then
            any=true
            LEFTOVERS=$((LEFTOVERS + 1))
            found "$d"
        fi
    done
    if [[ -e "$PRISM_LEGACY_ENV_FILE" ]]; then
        any=true
        LEFTOVERS=$((LEFTOVERS + 1))
        found "$PRISM_LEGACY_ENV_FILE (legacy login-session environment)"
        note "This is how the variables got into the login session in the first"
        note "place. Deleting it does not retract them from a running session."
    fi
    $any || clean "no PRISM data, config or state directories"
}

detect_rc() {
    section "Shell startup files"
    local any=false rc block start alias_name
    for rc in "${PRISM_RC_FILES[@]}"; do
        [[ -f "$rc" ]] || continue
        for block in "${PRISM_RC_BLOCKS[@]}"; do
            start="${block%%|||*}"
            if grep -qF "$start" "$rc" 2>/dev/null; then
                any=true
                LEFTOVERS=$((LEFTOVERS + 1))
                found "$rc: block '$start'"
            fi
        done
        for alias_name in "${PRISM_RC_ALIASES[@]}"; do
            if grep -qE "^[[:space:]]*alias[[:space:]]+$alias_name=\"?prism" "$rc" 2>/dev/null; then
                any=true
                LEFTOVERS=$((LEFTOVERS + 1))
                found "$rc: orphaned alias '$alias_name'"
            fi
        done
        if grep -qE "$PRISM_SHIM_DIR|PRISM_HUB_URL|:$PRISM_PROXY_PORT" "$rc" 2>/dev/null; then
            any=true
            LEFTOVERS=$((LEFTOVERS + 1))
            found "$rc: PRISM path or variable export"
        fi
    done
    if [[ -f "$PRISM_FISH_CONFIG" ]] && grep -qi prism "$PRISM_FISH_CONFIG" 2>/dev/null; then
        any=true
        LEFTOVERS=$((LEFTOVERS + 1))
        found "$PRISM_FISH_CONFIG: PRISM path entry"
    fi
    $any || clean "shell startup files contain no PRISM configuration"
}

detect_ide() {
    section "IDE settings"
    local any=false f hits
    while IFS= read -r f; do
        [[ -z "$f" ]] && continue
        # Match only what PRISM writes. A bare "proxySupport" match would flag
        # `"http.proxySupport": "off"`, which is a user deliberately disabling
        # proxy support -- the opposite of PRISM's doing.
        hits="$(grep -nEi "prism|:$PRISM_PROXY_PORT|:$PRISM_MCP_PORT|\"http\.proxyStrictSSL\"[[:space:]]*:[[:space:]]*false|\"http\.proxySupport\"[[:space:]]*:[[:space:]]*\"override\"" "$f" 2>/dev/null || true)"
        if [[ -n "$hits" ]]; then
            any=true
            LEFTOVERS=$((LEFTOVERS + 1))
            found "$f"
            printf '%s\n' "$hits" | while IFS= read -r h; do note "$h"; done
            if grep -q '"http.proxyStrictSSL"[[:space:]]*:[[:space:]]*false' "$f" 2>/dev/null; then
                note "proxyStrictSSL:false disables TLS certificate verification for"
                note "ALL of this IDE's traffic, indefinitely, with PRISM gone."
            fi
        fi
    done < <(prism_ide_settings_files)
    $any || clean "no PRISM configuration in any IDE settings.json"
}

detect_claude() {
    section "Claude Code configuration"
    local any=false
    if [[ -f "$PRISM_CLAUDE_SETTINGS" ]] &&
       grep -q "$PRISM_SHIM_DIR" "$PRISM_CLAUDE_SETTINGS" 2>/dev/null; then
        any=true
        LEFTOVERS=$((LEFTOVERS + 1))
        found "$PRISM_CLAUDE_SETTINGS: env.PATH references the PRISM shims directory"
        note "Claude Code does not expand \${PATH} here. Once the shims directory is"
        note "deleted the agent's shell has one missing directory plus a literal"
        note "string on PATH, and every command returns 'command not found'."
    fi
    if [[ -f "$PRISM_CLAUDE_CONFIG" ]] &&
       grep -q "\"$PRISM_CLAUDE_MCP_NAME\"" "$PRISM_CLAUDE_CONFIG" 2>/dev/null &&
       grep -q "mcpServers" "$PRISM_CLAUDE_CONFIG" 2>/dev/null; then
        if python3 - "$PRISM_CLAUDE_CONFIG" "$PRISM_CLAUDE_MCP_NAME" <<'PY'
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as f:
        data = json.load(f)
except Exception:
    sys.exit(1)
sys.exit(0 if sys.argv[2] in (data.get("mcpServers") or {}) else 1)
PY
        then
            any=true
            LEFTOVERS=$((LEFTOVERS + 1))
            found "$PRISM_CLAUDE_CONFIG: mcpServers.$PRISM_CLAUDE_MCP_NAME"
            note "Points at a binary this script deletes, leaving Claude Code with a"
            note "permanently failing MCP server on every start."
        fi
    fi
    $any || clean "Claude Code configuration is free of PRISM"
}

detect_nss() {
    section "NSS certificate trust (Chromium/Electron apps, Firefox)"
    if ! command -v certutil >/dev/null 2>&1; then
        UNFIXABLE=$((UNFIXABLE + 1))
        warn "certutil is not installed — NSS trust cannot be inspected or removed."
        note "The PRISM CA may still be trusted by every Electron app and Firefox"
        note "profile on this machine. Install it and re-run:"
        note "  sudo apt install libnss3-tools"
        return
    fi
    local any=false db hit
    while IFS= read -r db; do
        [[ -z "$db" ]] && continue
        hit="$(certutil -d "$db" -L 2>/dev/null | grep -F "$PRISM_NSS_NICKNAME" || true)"
        if [[ -n "$hit" ]]; then
            any=true
            LEFTOVERS=$((LEFTOVERS + 1))
            found "$db trusts '$PRISM_NSS_NICKNAME'"
        fi
    done < <(prism_nss_dbs)
    $any || clean "'$PRISM_NSS_NICKNAME' is not trusted by any NSS database"
}

detect_system_ca() {
    section "System-wide trust anchor"
    if [[ -e "$PRISM_SYSTEM_CA_LINUX" ]]; then
        UNFIXABLE=$((UNFIXABLE + 1))
        found "$PRISM_SYSTEM_CA_LINUX"
        note "A self-signed interception root installed into the system trust store."
        note "This script does not run as root, so remove it yourself:"
        printf '\n      sudo rm -f %s\n      sudo update-ca-certificates --fresh\n\n' \
            "$PRISM_SYSTEM_CA_LINUX"
        note "Do this even if PRISM is otherwise gone: removal deletes the CA private"
        note "key, so the machine would keep trusting a root nobody can audit."
    else
        clean "no PRISM root in the system trust store"
    fi
}

# ------------------------------------------------------------------------------
# The detector nothing else has: processes already holding dead PRISM variables.
#
# This is the reason removal has appeared to fail. A process started from a
# poisoned login session keeps its copy of the environment forever. Naming them
# by PID turns an invisible problem into a list of applications to restart.
# ------------------------------------------------------------------------------

detect_processes() {
    section "Running processes holding PRISM environment"

    local var_re
    var_re="$(IFS='|'; echo "${PRISM_ENV_VARS[*]}")"

    # One row per process, not per variable: a machine in this state has ~20
    # processes carrying ~7 variables each, and 140 lines of near-identical text
    # buries the one fact that matters -- which programs need restarting.
    local rows="" dead_rows="" pid comm environ line name value vars dead_paths
    for pid in /proc/[0-9]*; do
        pid="${pid#/proc/}"
        # Reading another user's /proc entry is a permission error from the shell
        # itself, not from the command, so redirecting the command's stderr is
        # not enough -- cat owns the open() and can be silenced.
        environ="$(cat "/proc/$pid/environ" 2>/dev/null | tr '\0' '\n' || true)"
        [[ -z "$environ" ]] && continue
        comm="$(cat "/proc/$pid/comm" 2>/dev/null || echo '?')"

        vars=""
        dead_paths=""
        while IFS= read -r line; do
            name="${line%%=*}"
            value="${line#*=}"
            [[ "$name" == "$line" ]] && continue
            prism_value_is_ours "$value" || continue
            vars+="${vars:+,}$name"
            case " ${PRISM_ENV_PATH_VARS[*]} " in
                *" $name "*)
                    if [[ ! -e "$value" ]]; then
                        dead_paths+="${dead_paths:+,}$name"
                        dead_rows+="$(printf '    %-8s %-18s %s=%s' "$pid" "$comm" "$name" "$value")"$'\n'
                    fi
                    ;;
            esac
        done < <(printf '%s\n' "$environ" | grep -E "^($var_re)=" || true)

        [[ -z "$vars" ]] && continue
        if [[ -n "$dead_paths" ]]; then
            rows+="$(printf '    %-8s %-18s %s  [DEAD PATH: %s]' "$pid" "$comm" "$vars" "$dead_paths")"$'\n'
        else
            rows+="$(printf '    %-8s %-18s %s' "$pid" "$comm" "$vars")"$'\n'
        fi
    done

    if [[ -z "$rows" ]]; then
        clean "no running process carries PRISM environment variables"
        return
    fi

    local count procs dead_count session_hits
    count="$(printf '%s' "$rows" | grep -c . || true)"
    dead_count="$(printf '%s' "$dead_rows" | awk '{print $1}' | sort -u | grep -c . || true)"

    UNFIXABLE=$((UNFIXABLE + 1))
    found "$count running process(es) carry PRISM environment variables"

    # The single most important question is not which programs are affected but
    # whether the login session itself is. A poisoned session leader hands the
    # same variables to every program started from the desktop afterwards, so no
    # amount of restarting individual applications will ever finish the job.
    session_hits="$(printf '%s' "$rows" |
        grep -E ' (systemd|gnome-session[^ ]*|gnome-shell|plasmashell|dbus-daemon|Xwayland|sway|xfce4-session) ' || true)"
    if [[ -n "$session_hits" ]]; then
        printf '\n'
        found "YOUR LOGIN SESSION ITSELF is carrying them:"
        printf '%s\n' "$session_hits"
        printf '\n'
        note "Every program you start from the desktop inherits this. Restarting"
        note "individual applications will not finish the job — only logging out"
        note "and back in replaces the session and its environment."
    fi

    # Dangling paths are the actively harmful subset, so they are never elided.
    if (( dead_count > 0 )); then
        printf '\n'
        found "$dead_count process(es) point at files that NO LONGER EXIST:"
        printf '%s' "$dead_rows"
        printf '\n'
        note "SSL_CERT_FILE, CURL_CA_BUNDLE and REQUESTS_CA_BUNDLE REPLACE the trust"
        note "store rather than adding to it. Pointed at a deleted file, those"
        note "processes have no trusted CAs at all — every TLS connection they make"
        note "through a library honouring these variables fails, with an error that"
        note "will not mention PRISM."
    else
        printf '\n'
        note "None of the referenced paths are dangling right now, because PRISM is"
        note "currently installed. They become dangling the moment it is removed —"
        note "which is what turns a tidy uninstall into a broken machine."
    fi

    # A full dump is ~100 near-identical lines and buries everything above it,
    # including the session finding that actually determines what to do.
    printf '\n'
    if $VERBOSE; then
        note "Every affected process:"
        printf '%s' "$rows"
    else
        note "Affected programs (process count):"
        printf '%s' "$rows" | awk '{print $2}' | sort | uniq -c | sort -rn |
            awk 'NR<=12 {printf "      %-22s %s\n", $2, $1}
                 NR>12  {more++}
                 END    {if (more) printf "      ... and %d more program(s)\n", more}'
        printf '\n'
        note "Run with --verbose to list every process individually."
    fi

    printf '\n'
    warn "No script can repair these, including this one."
    note "A process receives a copy of the environment when it starts. Unsetting a"
    note "variable changes only the shell that ran the unset, and"
    note "systemctl --user unset-environment only affects units started afterwards."
    note "This is why removing PRISM has never appeared to fix anything: removal"
    note "succeeded every time, and the damage lived in processes it could not reach."
    printf '\n'
    note "Fix: log out and back in. That replaces the session and every process in"
    note "it at once, and is the only complete fix."
}

# ------------------------------------------------------------------------------
# Run every detector.
# ------------------------------------------------------------------------------

detect_all() {
    LEFTOVERS=0
    UNFIXABLE=0
    detect_binaries
    detect_units
    detect_ports
    detect_data
    detect_rc
    detect_ide
    detect_claude
    detect_nss
    detect_system_ca
    detect_processes
}

# ==============================================================================
# REMOVAL
# ==============================================================================

# Stop and delete the units FIRST, and confirm the daemon is actually dead before
# anything else is deleted. The previous version removed unit files early but
# never verified, then deleted the data directory several steps later — long
# enough for a surviving Restart=always unit to respawn the daemon and regenerate
# the CA it had just deleted.
remove_units_and_processes() {
    section "Stopping services"

    local f unit
    while IFS= read -r f; do
        [[ -z "$f" ]] && continue
        unit="$(basename "$f")"
        act "stop + disable $unit"
        systemctl --user stop "$unit" 2>/dev/null || true
        systemctl --user disable "$unit" 2>/dev/null || true
    done < <(prism_unit_files)

    while IFS= read -r f; do
        [[ -z "$f" ]] && continue
        act "rm $f"
        rm -f "$f"
    done < <(prism_unit_wants_links)

    while IFS= read -r f; do
        [[ -z "$f" ]] && continue
        act "rm $f"
        rm -f "$f"
    done < <(prism_unit_files)

    systemctl --user daemon-reload 2>/dev/null || true
    systemctl --user reset-failed 2>/dev/null || true

    act "systemctl --user unset-environment (affects units started from now on)"
    systemctl --user unset-environment "${PRISM_ENV_VARS[@]}" 2>/dev/null || true

    act "terminating PRISM processes"
    pkill -f "$PRISM_BIN_DIR/prism" 2>/dev/null || true

    # Confirm nothing respawns before we delete the files it would regenerate.
    local tries=0
    while pgrep -f "$PRISM_BIN_DIR/prism" >/dev/null 2>&1; do
        tries=$((tries + 1))
        if (( tries > 10 )); then
            pkill -9 -f "$PRISM_BIN_DIR/prism" 2>/dev/null || true
            sleep 1
            break
        fi
        sleep 1
    done

    if pgrep -f "$PRISM_BIN_DIR/prism" >/dev/null 2>&1; then
        warn "a PRISM process is still running after SIGKILL — something outside"
        note "systemd is restarting it. Removal continues, but re-run the audit."
    else
        clean "no PRISM process running; safe to delete files"
    fi
}

remove_binaries_and_data() {
    section "Removing executables and data"
    local f d
    while IFS= read -r f; do
        [[ -z "$f" ]] && continue
        act "rm $f"
        rm -f "$f"
    done < <(prism_installed_binaries)

    for d in "$PRISM_DATA_DIR" "$PRISM_CONFIG_DIR" "$PRISM_STATE_DIR"; do
        if [[ -e "$d" ]]; then
            act "rm -rf $d"
            rm -rf "$d"
        fi
    done

    if [[ -e "$PRISM_LEGACY_ENV_FILE" ]]; then
        act "rm $PRISM_LEGACY_ENV_FILE"
        rm -f "$PRISM_LEGACY_ENV_FILE"
    fi

    # Any other environment.d file mentioning PRISM, whatever it is named.
    local envd="$HOME/.config/environment.d"
    if [[ -d "$envd" ]]; then
        while IFS= read -r -d '' f; do
            if grep -qEi "prism|:$PRISM_PROXY_PORT|:$PRISM_HUB_PORT" "$f" 2>/dev/null; then
                act "rm $f"
                rm -f "$f"
            fi
        done < <(find "$envd" -type f -print0 2>/dev/null)
    fi
}

remove_rc() {
    section "Cleaning shell startup files"
    local rc block start end alias_name
    for rc in "${PRISM_RC_FILES[@]}"; do
        [[ -f "$rc" ]] || continue
        cp "$rc" "$rc.before-prism-removal"
        for block in "${PRISM_RC_BLOCKS[@]}"; do
            start="${block%%|||*}"
            end="${block##*|||}"
            sed -i "\|$start|,\|$end|d" "$rc"
        done
        for alias_name in "${PRISM_RC_ALIASES[@]}"; do
            sed -i -E "/^[[:space:]]*alias[[:space:]]+$alias_name=\"?prism/d" "$rc"
        done
        sed -i -E "/^[[:space:]]*export[[:space:]]+PRISM_[A-Z_]+=/d" "$rc"
        sed -i -E "/^[[:space:]]*export[[:space:]]+(HTTP_PROXY|HTTPS_PROXY|http_proxy|https_proxy|ALL_PROXY|all_proxy)=.*:$PRISM_PROXY_PORT/d" "$rc"
        sed -i -E "/^[[:space:]]*export[[:space:]]+(NODE_EXTRA_CA_CERTS|CURL_CA_BUNDLE|REQUESTS_CA_BUNDLE|SSL_CERT_FILE|SSL_CERT_DIR)=.*prism/d" "$rc"
        sed -i "\|$PRISM_SHIM_DIR|d" "$rc"
        sed -i -E "/^[[:space:]]*#[[:space:]]*(PRISM (Environment|Aliases)|end PRISM)[[:space:]]*$/d" "$rc"
        act "cleaned $rc (backup: $rc.before-prism-removal)"
    done

    if [[ -f "$PRISM_FISH_CONFIG" ]]; then
        cp "$PRISM_FISH_CONFIG" "$PRISM_FISH_CONFIG.before-prism-removal"
        sed -i "\|$PRISM_SHIM_DIR|d" "$PRISM_FISH_CONFIG"
        sed -i -E "/^[[:space:]]*set -gx PRISM_[A-Z_]+/d" "$PRISM_FISH_CONFIG"
        act "cleaned $PRISM_FISH_CONFIG"
    fi
}

remove_ide_settings() {
    section "Cleaning IDE settings"
    local f
    while IFS= read -r f; do
        [[ -z "$f" ]] && continue
        cp "$f" "$f.before-prism-removal"
        python3 - "$f" "$PRISM_SHIM_DIR" "$PRISM_PROXY_PORT" "$PRISM_MCP_PORT" "$PRISM_HUB_PORT" <<'PY'
import json
import sys

path, shim_dir = sys.argv[1], sys.argv[2]
ports = set(sys.argv[3:6])

try:
    with open(path, encoding="utf-8") as fh:
        data = json.load(fh)
except Exception as exc:                      # noqa: BLE001
    print(f"    skipped (not valid JSON): {exc}")
    sys.exit(0)

# http.proxy is deleted unconditionally: it is a hard dependency on a local
# daemon, so leaving it behind costs the IDE all networking whenever PRISM is not
# running. The other two are deleted only when they hold the value PRISM's setup
# writes, so a user who chose them deliberately keeps their setting.
#
# The guard here used to be `"27181" in str(value)`, which is never true for a
# boolean — so http.proxyStrictSSL:false survived every removal and left the IDE
# skipping TLS verification permanently.
if "http.proxy" in data:
    del data["http.proxy"]
if data.get("http.proxyStrictSSL") is False:
    del data["http.proxyStrictSSL"]
if data.get("http.proxySupport") == "override":
    del data["http.proxySupport"]

PRISM_KEYS = {
    "HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy",
    "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy",
    "PRISM_HUB_URL", "PRISM_MCP_TOKEN",
    "NODE_EXTRA_CA_CERTS", "CURL_CA_BUNDLE", "REQUESTS_CA_BUNDLE",
    "SSL_CERT_FILE", "SSL_CERT_DIR",
}


def is_ours(value):
    text = str(value)
    return "prism" in text.lower() or any(p in text for p in ports)


def clean(obj):
    if not isinstance(obj, dict):
        return
    for key in list(obj):
        value = obj[key]
        if key in PRISM_KEYS and is_ours(value):
            del obj[key]
            continue
        if key == "PATH" and shim_dir in str(value):
            del obj[key]
            continue
        if isinstance(value, dict):
            clean(value)
        elif isinstance(value, list):
            for item in value:
                clean(item)


clean(data)

with open(path, "w", encoding="utf-8") as fh:
    json.dump(data, fh, indent=4, ensure_ascii=False)
    fh.write("\n")
print(f"    cleaned {path}")
PY
        act "cleaned $f (backup: $f.before-prism-removal)"
    done < <(prism_ide_settings_files)
}

remove_claude_config() {
    section "Cleaning Claude Code configuration"

    if [[ -f "$PRISM_CLAUDE_SETTINGS" ]]; then
        cp "$PRISM_CLAUDE_SETTINGS" "$PRISM_CLAUDE_SETTINGS.before-prism-removal"
        python3 - "$PRISM_CLAUDE_SETTINGS" "$PRISM_SHIM_DIR" <<'PY'
import json
import sys

path, shim_dir = sys.argv[1], sys.argv[2]
try:
    with open(path, encoding="utf-8") as fh:
        data = json.load(fh)
except Exception as exc:                      # noqa: BLE001
    print(f"    skipped (not valid JSON): {exc}")
    sys.exit(0)

env = data.get("env")
changed = False
if isinstance(env, dict):
    for key in list(env):
        if shim_dir in str(env[key]):
            del env[key]
            changed = True
    # An empty env block is noise, and an env block containing only an unexpanded
    # "${PATH}" is worse than no env block at all.
    if not env:
        del data["env"]
        changed = True

if changed:
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(data, fh, indent=2, ensure_ascii=False)
        fh.write("\n")
    print("    removed PRISM entries from env")
else:
    print("    nothing to remove")
PY
        act "cleaned $PRISM_CLAUDE_SETTINGS"
    fi

    if [[ -f "$PRISM_CLAUDE_CONFIG" ]]; then
        cp "$PRISM_CLAUDE_CONFIG" "$PRISM_CLAUDE_CONFIG.before-prism-removal"
        python3 - "$PRISM_CLAUDE_CONFIG" "$PRISM_CLAUDE_MCP_NAME" <<'PY'
import json
import sys

path, name = sys.argv[1], sys.argv[2]
try:
    with open(path, encoding="utf-8") as fh:
        data = json.load(fh)
except Exception as exc:                      # noqa: BLE001
    print(f"    skipped (not valid JSON): {exc}")
    sys.exit(0)

removed = 0


def strip(obj):
    global removed
    if not isinstance(obj, dict):
        return
    servers = obj.get("mcpServers")
    if isinstance(servers, dict) and name in servers:
        del servers[name]
        removed += 1
    for value in obj.values():
        if isinstance(value, dict):
            strip(value)


strip(data)

if removed:
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(data, fh, indent=2, ensure_ascii=False)
        fh.write("\n")
print(f"    removed {removed} mcpServers.{name} entr{'y' if removed == 1 else 'ies'}")
PY
        act "cleaned $PRISM_CLAUDE_CONFIG"
    fi
}

remove_nss() {
    section "Removing NSS certificate trust"
    if ! command -v certutil >/dev/null 2>&1; then
        warn "certutil is not installed — NSS trust CANNOT be removed."
        note "The PRISM CA stays trusted by every Electron app and Firefox profile"
        note "on this machine until you install libnss3-tools and re-run:"
        note "  sudo apt install libnss3-tools && ./remove-prism.sh --remove"
        return
    fi
    local db removed=0
    while IFS= read -r db; do
        [[ -z "$db" ]] && continue
        if certutil -D -d "$db" -n "$PRISM_NSS_NICKNAME" -f /dev/null >/dev/null 2>&1; then
            act "removed '$PRISM_NSS_NICKNAME' from $db"
            removed=$((removed + 1))
        fi
    done < <(prism_nss_dbs)
    if (( removed == 0 )); then
        clean "no NSS database trusted '$PRISM_NSS_NICKNAME'"
    fi
    return 0
}

remove_all() {
    remove_units_and_processes
    remove_nss
    remove_ide_settings
    remove_claude_config
    remove_rc
    remove_binaries_and_data
}

# ==============================================================================
# MAIN
# ==============================================================================

printf '%s========================================%s\n' "$C_BOLD" "$C_RESET"
if [[ "$MODE" == "audit" ]]; then
    printf '%s  PRISM REMOVAL — AUDIT (read-only)%s\n' "$C_BOLD" "$C_RESET"
else
    printf '%s  PRISM REMOVAL%s\n' "$C_BOLD" "$C_RESET"
fi
printf '%s========================================%s\n' "$C_BOLD" "$C_RESET"
printf '\nUser: %s    Home: %s\n' "$(whoami)" "$HOME"

detect_all

if [[ "$MODE" == "audit" ]]; then
    section "Summary"
    if (( LEFTOVERS == 0 && UNFIXABLE == 0 )); then
        clean "PRISM is fully removed from this machine."
        exit 0
    fi
    if (( LEFTOVERS > 0 )); then
        found "$LEFTOVERS removable leftover(s) — run: ./remove-prism.sh --remove"
    fi
    if (( UNFIXABLE > 0 )); then
        warn "$UNFIXABLE finding(s) no script can fix (see above): restart the"
        note "listed processes, log out, or run the printed sudo commands."
    fi
    printf '\n'
    note "This was a read-only audit. Nothing was changed."
    exit 1
fi

# --- removal path ---

if (( LEFTOVERS == 0 )); then
    section "Summary"
    clean "nothing to remove."
    if (( UNFIXABLE > 0 )); then
        warn "$UNFIXABLE finding(s) still need manual action (see above)."
    fi
    exit 0
fi

if ! $ASSUME_YES; then
    printf '\n'
    read -r -p "Remove the $LEFTOVERS item(s) listed above? [y/N]: " answer
    case "$answer" in
        [yY]|[yY][eE][sS]) ;;
        *) echo "Aborted. Nothing was changed."; exit 1 ;;
    esac
fi

remove_all

printf '\n%s========================================%s\n' "$C_BOLD" "$C_RESET"
printf '%s  VERIFICATION (same detector as above)%s\n' "$C_BOLD" "$C_RESET"
printf '%s========================================%s\n' "$C_BOLD" "$C_RESET"

detect_all

section "Result"
if (( LEFTOVERS == 0 )); then
    clean "every removable trace of PRISM is gone."
else
    found "$LEFTOVERS item(s) still present — see above."
fi

if (( UNFIXABLE > 0 )); then
    printf '\n'
    warn "$UNFIXABLE finding(s) remain that removal cannot address."
    note "Running processes keep the environment they were started with. Restart the"
    note "programs listed above, or log out and back in to clear all of them at once."
fi

printf '\n'
note "Your source checkouts were not touched."
note "Backups were written next to every file this script edited, suffixed"
note "  .before-prism-removal"
printf '\n'

(( LEFTOVERS == 0 )) && exit 0 || exit 1
