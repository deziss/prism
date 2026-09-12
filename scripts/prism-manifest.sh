#!/usr/bin/env bash
# ==============================================================================
# prism-manifest.sh — the single source of truth for everything PRISM writes
#                     outside its own source tree.
#
# WHY THIS FILE EXISTS
# --------------------
# PRISM used to have three independent "off" switches — remove-prism.sh,
# prism-disable, and partial cleanup inside `prism init` — each with a different
# and incomplete idea of what "on" had created. The result was that a "COMPLETE
# REMOVAL" left behind systemd units, NSS trust entries, IDE proxy settings, an
# MCP registration pointing at a deleted binary, and helper scripts able to
# re-poison the machine on demand. Every removal reported success; none of them
# were complete.
#
# Every host-mutating path, variable, unit, marker and trust entry is declared
# here exactly once. prism-enable, prism-disable and remove-prism.sh all source
# this file and carry no private lists of their own. tests/uninstall_symmetry.rs
# scans the source tree for host writes and fails the build if a target is not
# declared here, so an install step can no longer be added without an uninstall
# step existing for it.
#
# This file is sourced, never executed. It must remain free of side effects:
# declarations and pure functions only, so that a read-only audit can source it
# safely.
# ==============================================================================

# Guard against double-sourcing (prism-enable may source it before calling a
# helper that sources it again).
[ -n "${PRISM_MANIFEST_LOADED:-}" ] && return 0
PRISM_MANIFEST_LOADED=1

# ------------------------------------------------------------------------------
# Ports
# ------------------------------------------------------------------------------

PRISM_PROXY_PORT="27181"
PRISM_MCP_PORT="27182"
PRISM_HUB_PORT="27183"

# ------------------------------------------------------------------------------
# Environment variables PRISM has ever set, in any version.
#
# Current code sets none of these globally — `prism init` only exports
# PRISM_HUB_URL, and proxy vars are per-process. Older versions wrote all of them
# into ~/.config/environment.d/10-prism.conf and into shell rc files, which is
# why they still haunt long-running login sessions. Removal must keep looking for
# all of them regardless of what the current version writes.
#
# SSL_CERT_FILE, CURL_CA_BUNDLE and REQUESTS_CA_BUNDLE *replace* the trust store
# rather than appending to it. Left pointing at a deleted file, a process has no
# trusted CAs at all — a worse failure than the dead proxy, and a silent one.
# ------------------------------------------------------------------------------

PRISM_ENV_VARS=(
    HTTP_PROXY
    HTTPS_PROXY
    http_proxy
    https_proxy
    ALL_PROXY
    all_proxy
    NO_PROXY
    no_proxy
    PRISM_HUB_URL
    PRISM_MCP_TOKEN
    NODE_EXTRA_CA_CERTS
    CURL_CA_BUNDLE
    REQUESTS_CA_BUNDLE
    SSL_CERT_FILE
    SSL_CERT_DIR
)

# The subset whose value points at a filesystem path. A dangling path in one of
# these is what the live-process detector reports as actively broken.
PRISM_ENV_PATH_VARS=(
    NODE_EXTRA_CA_CERTS
    CURL_CA_BUNDLE
    REQUESTS_CA_BUNDLE
    SSL_CERT_FILE
    SSL_CERT_DIR
)

# ------------------------------------------------------------------------------
# Filesystem locations
# ------------------------------------------------------------------------------

PRISM_BIN_DIR="$HOME/.local/bin"
PRISM_DATA_DIR="$HOME/.local/share/prism"
PRISM_CONFIG_DIR="$HOME/.config/prism"

# Deliberately outside PRISM_DATA_DIR: `rm -rf` of the data directory must not
# destroy the record of what still needs removing.
PRISM_STATE_DIR="$HOME/.local/state/prism"
PRISM_RECEIPT="$PRISM_STATE_DIR/receipt.json"

PRISM_CA_DIR="$PRISM_DATA_DIR/ca"
PRISM_CA_CERT="$PRISM_CA_DIR/ca.crt"
PRISM_CA_KEY="$PRISM_CA_DIR/ca.key"
PRISM_CA_BUNDLE="$PRISM_CA_DIR/ca-bundle.crt"
PRISM_SHIM_DIR="$PRISM_DATA_DIR/shims"

PRISM_SYSTEMD_USER_DIR="$HOME/.config/systemd/user"

# Legacy: written by versions before the per-process proxy model. Current code
# only ever deletes this.
PRISM_LEGACY_ENV_FILE="$HOME/.config/environment.d/10-prism.conf"

# Executables install.sh places in ~/.local/bin. `prism-env` is not installed by
# install.sh but has been hand-written on at least one machine; it survives
# removal and re-poisons the environment on demand, so it is listed here.
PRISM_BIN_NAMES=(
    prism
    prism-enable
    prism-disable
    prism-on
    prism-off
    prism-env
    prism-bridge
)

# ------------------------------------------------------------------------------
# systemd user units
#
# Matched by glob, not by name. remove-prism.sh previously hardcoded
# prism-proxy.service and a prism-bridge.service that nothing creates, which let
# prism-mcp.service survive every removal — and an ad-hoc unit with
# `Description=PRISM AI Proxy Service`, whose text appears nowhere in this repo,
# survived too. A glob catches units this repo has never heard of.
# ------------------------------------------------------------------------------

PRISM_UNIT_GLOB='prism-*'
PRISM_UNIT_SUFFIXES=(service socket timer)

# ------------------------------------------------------------------------------
# NSS trust databases (Chromium/Electron apps and Firefox profiles)
# ------------------------------------------------------------------------------

PRISM_NSS_NICKNAME="PRISM Local CA"

# Echoes one `sql:<dir>` per NSS database present on this machine.
prism_nss_dbs() {
    [ -d "$HOME/.pki/nssdb" ] && echo "sql:$HOME/.pki/nssdb"
    local prof
    for prof in \
        "$HOME"/.mozilla/firefox/*/ \
        "$HOME"/snap/firefox/common/.mozilla/firefox/*/ \
        "$HOME"/.var/app/org.mozilla.firefox/.mozilla/firefox/*/
    do
        [ -f "${prof}cert9.db" ] && echo "sql:${prof%/}"
    done
    return 0
}

# ------------------------------------------------------------------------------
# System-wide trust anchor
#
# src/proxy.rs installs a self-signed MITM root here via update-ca-certificates.
# Nothing removed it, while the private key under PRISM_CA_DIR *was* removed — so
# a "complete removal" could leave the machine trusting a CA whose key is gone.
# Removal needs root; remove-prism.sh refuses to run under sudo by design, so it
# reports this and prints the commands rather than escalating.
# ------------------------------------------------------------------------------

PRISM_SYSTEM_CA_LINUX="/usr/local/share/ca-certificates/prism.crt"
PRISM_SYSTEM_CA_MACOS_KEYCHAIN="/Library/Keychains/System.keychain"

# ------------------------------------------------------------------------------
# Shell rc files and the marker blocks written into them
#
# Three different marker styles exist across versions. All three must be matched,
# together with the aliases inside the first block — remove-prism.sh used to
# delete the PRISM_HUB_URL line from inside a block while leaving the block
# markers and all eight aliases pointing at a deleted binary.
# ------------------------------------------------------------------------------

PRISM_RC_FILES=(
    "$HOME/.bashrc"
    "$HOME/.profile"
    "$HOME/.zshrc"
    "$HOME/.bash_profile"
)

PRISM_FISH_CONFIG="$HOME/.config/fish/config.fish"

# Each entry is "START|||END" for a sed range delete.
PRISM_RC_BLOCKS=(
    '# >>> PRISM DEFAULT LLM PROXY >>>|||# <<< PRISM DEFAULT LLM PROXY <<<'
    '# >>> PRISM shims >>>|||# <<< PRISM shims <<<'
    '# PRISM environment (added by|||# end PRISM'
    '# PRISM — transparent LLM proxy|||# end PRISM'
)

# Aliases written by prism-enable, left orphaned when only the block markers
# were removed.
PRISM_RC_ALIASES=(p prtk pread ptoon pcomp pgraph pcache pgain)

# ------------------------------------------------------------------------------
# IDE / agent configuration files
#
# Matched by glob. remove-prism.sh hardcoded the single literal path
# "$HOME/.config/Antigravity IDE/User/settings.json"; the machine had three
# Antigravity profile directories and the script cleaned one of them, then
# printed "OK: Antigravity settings are clean" for the other two.
# ------------------------------------------------------------------------------

# Directory-name globs under ~/.config, each expected to hold User/settings.json.
PRISM_IDE_CONFIG_GLOBS=(
    'Antigravity*'
    'Code'
    'Code - OSS'
    'VSCodium'
    'Cursor'
    'Windsurf'
    'Qoder'
)

# Echoes the path of every IDE settings.json present on this machine.
prism_ide_settings_files() {
    local g dir
    for g in "${PRISM_IDE_CONFIG_GLOBS[@]}"; do
        # Globs here contain spaces ("Code - OSS") and the directories they match
        # do too ("Antigravity IDE"). An unquoted shell glob word-splits on both,
        # which silently scanned "Code" twice and skipped spaced directories.
        while IFS= read -r dir; do
            [ -f "$dir/User/settings.json" ] && echo "$dir/User/settings.json"
        done < <(find "$HOME/.config" -maxdepth 1 -name "$g" -type d 2>/dev/null)
    done
    return 0
}

# Top-level IDE settings keys PRISM's setup adds.
#
# http.proxy is the hard dependency on a local daemon: with PRISM stopped the IDE
# loses networking entirely (ERR_PROXY_CONNECTION_FAILED). proxyStrictSSL:false
# disables TLS verification for all IDE traffic and is the one that matters most
# — it used to survive every removal because the old guard tested
# `"27181" in str(value)`, which is never true for a boolean.
PRISM_IDE_PROXY_KEYS=(
    'http.proxy'
    'http.proxySupport'
    'http.proxyStrictSSL'
)

# Nested objects inside IDE settings that PRISM injects environment into.
PRISM_IDE_ENV_KEYS=(
    'terminal.integrated.env.linux'
    'terminal.integrated.env.osx'
    'codeiumDev.languageServerEnv'
)

# Claude Code. Neither removal script ever opened either of these.
#
# ~/.claude/settings.json gets an env.PATH of
# "<shims dir>:${PATH}". Claude Code does not expand ${PATH}, so once the shims
# directory is deleted the agent's shell resolves a PATH of one missing directory
# plus a literal string, and every command returns "command not found".
#
# ~/.claude.json gets mcpServers.prism pointing at the binary that removal
# deletes, leaving a permanently failing MCP server.
PRISM_CLAUDE_SETTINGS="$HOME/.claude/settings.json"
PRISM_CLAUDE_CONFIG="$HOME/.claude.json"
PRISM_CLAUDE_MCP_NAME="prism"

# ------------------------------------------------------------------------------
# Project-local artifacts
#
# Written into whatever directory `prism hook install` / `prism init` ran in.
# Not removed by anything: they live in user projects and deleting them from a
# global uninstall would be overreach. Declared so the symmetry test knows they
# are a deliberate omission rather than an oversight, and so the audit can
# mention them.
# ------------------------------------------------------------------------------

PRISM_PROJECT_LOCAL=(
    '.prismrc'
    '.prism/hooks'
)

# ------------------------------------------------------------------------------
# Helpers shared by the enable/disable/remove scripts
# ------------------------------------------------------------------------------

# Echoes every systemd user unit file matching the PRISM glob.
prism_unit_files() {
    local suffix f
    for suffix in "${PRISM_UNIT_SUFFIXES[@]}"; do
        # The directory is quoted (it may contain spaces); the pattern must not
        # be, or the shell treats it as a literal filename and matches nothing.
        for f in "$PRISM_SYSTEMD_USER_DIR"/prism-*."$suffix"; do
            [ -e "$f" ] && echo "$f"
        done
    done
    return 0
}

# Echoes every *.wants symlink pointing at a PRISM unit.
prism_unit_wants_links() {
    local f
    for f in "$PRISM_SYSTEMD_USER_DIR"/*.wants/prism-*; do
        [ -e "$f" ] || [ -L "$f" ] && echo "$f"
    done
    return 0
}

# Echoes every ~/.local/bin entry PRISM installs that currently exists.
prism_installed_binaries() {
    local name
    for name in "${PRISM_BIN_NAMES[@]}"; do
        [ -e "$PRISM_BIN_DIR/$name" ] || [ -L "$PRISM_BIN_DIR/$name" ] &&
            echo "$PRISM_BIN_DIR/$name"
    done
    return 0
}

# True when a value looks like something PRISM wrote.
prism_value_is_ours() {
    case "$1" in
        *"$PRISM_PROXY_PORT"* | *"$PRISM_MCP_PORT"* | *"$PRISM_HUB_PORT"* | \
        *prism* | *PRISM*) return 0 ;;
        *) return 1 ;;
    esac
}
