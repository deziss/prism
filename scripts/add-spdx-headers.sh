#!/usr/bin/env bash
#
# add-spdx-headers.sh — add `SPDX-License-Identifier` headers to prism's sources.
#
# Why this exists: prism has no per-file licence headers. For a dual-licensed project
# (AGPL-3.0-only + a commercial licence, see ../docs/LICENSING.md) per-file identifiers are
# the durable, per-file record of what was licensed under what — they survive file moves and
# code copying, and they make it visible when a file arrives from somewhere else under other
# terms.
#
# Placement: line 1, then a blank line, then the file's original content.
#
#   // SPDX-License-Identifier: AGPL-3.0-only
#
#   //! Module docs, untouched.
#
# That is safe above a `//!` inner doc comment: a plain `//` comment is whitespace to the
# parser, and rustdoc still attributes the `//!` block below it to the module (verified with
# a standalone rustdoc run). What would break rustdoc is inserting the line *inside* a `//!`
# block, which this script never does — it only ever prepends, and it skips any file that
# already carries an identifier.
#
# Idempotent: a file with `SPDX-License-Identifier` in its first 10 lines is left alone, so
# re-running is a no-op.
#
# Usage:
#   scripts/add-spdx-headers.sh --dry-run            # print what would change, write nothing
#   scripts/add-spdx-headers.sh                      # apply
#   scripts/add-spdx-headers.sh --check              # CI gate: exit 1 if any file lacks one
#   scripts/add-spdx-headers.sh --license MIT        # override the identifier
#   scripts/add-spdx-headers.sh --copyright "2026 Some Name"   # also add a copyright line
#   scripts/add-spdx-headers.sh --git                # enumerate via `git ls-files` instead of find
#   scripts/add-spdx-headers.sh src/hub.rs src/mcp.rs          # explicit files only
#
# SPDX-License-Identifier: AGPL-3.0-only

set -euo pipefail

LICENSE_ID="AGPL-3.0-only"
COPYRIGHT=""
DRY_RUN=1   # default: show, do not write. --apply opts in.
CHECK=0
USE_GIT=0
QUIET=0
FILES=()

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Directories searched when no explicit files are given.
SEARCH_DIRS=(src tests bench examples)
# Never walked into.
PRUNE_DIRS=(target .git node_modules graphify-out vendor dist build .venv)

die() { printf 'add-spdx-headers: %s\n' "$*" >&2; exit 2; }

usage() {
    sed -n '3,40p' "${BASH_SOURCE[0]}" | sed 's|^# \{0,1\}||'
    exit 0
}

while [ $# -gt 0 ]; do
    case "$1" in
        --apply)       DRY_RUN=0 ;;
        --dry-run|-n)  DRY_RUN=1 ;;   # the default; accepted for explicitness
        --check)       CHECK=1; DRY_RUN=1 ;;
        --git)         USE_GIT=1 ;;
        --quiet|-q)    QUIET=1 ;;
        --license)     shift; [ $# -gt 0 ] || die "--license needs a value"; LICENSE_ID="$1" ;;
        --license=*)   LICENSE_ID="${1#*=}" ;;
        --copyright)   shift; [ $# -gt 0 ] || die "--copyright needs a value"; COPYRIGHT="$1" ;;
        --copyright=*) COPYRIGHT="${1#*=}" ;;
        -h|--help)     usage ;;
        --)            shift; while [ $# -gt 0 ]; do FILES+=("$1"); shift; done ;;
        -*)            die "unknown flag: $1 (try --help)" ;;
        *)             FILES+=("$1") ;;
    esac
    shift
done

[ -n "$LICENSE_ID" ] || die "empty --license"

# ------------------------------------------------------------------ file list

collect_files() {
    if [ "${#FILES[@]}" -gt 0 ]; then
        printf '%s\n' "${FILES[@]}"
        return
    fi

    if [ "$USE_GIT" -eq 1 ]; then
        # Respects .gitignore, so target/ is excluded for free. Opt-in only.
        command -v git >/dev/null 2>&1 || die "--git given but git not found"
        ( cd "$REPO_ROOT" && git ls-files -- '*.rs' '*.sh' ) \
            | while IFS= read -r f; do printf '%s/%s\n' "$REPO_ROOT" "$f"; done
        return
    fi

    # Default: plain find, no git invocation.
    local prune=() d
    for d in "${PRUNE_DIRS[@]}"; do
        prune+=( -name "$d" -o )
    done
    unset 'prune[${#prune[@]}-1]'   # drop the trailing -o

    local dirs=() sd
    for sd in "${SEARCH_DIRS[@]}"; do
        [ -d "$REPO_ROOT/$sd" ] && dirs+=("$REPO_ROOT/$sd")
    done
    [ "${#dirs[@]}" -gt 0 ] || die "none of ${SEARCH_DIRS[*]} exist under $REPO_ROOT"

    find "${dirs[@]}" \( "${prune[@]}" \) -prune -o -type f -name '*.rs' -print | LC_ALL=C sort
}

# ------------------------------------------------------------- comment syntax

comment_prefix() {
    case "$1" in
        *.rs|*.ts|*.tsx|*.js|*.mjs|*.cjs|*.c|*.h|*.cpp|*.go|*.java|*.kt) printf '//' ;;
        *.sh|*.bash|*.py|*.rb|*.pl|*.yml|*.yaml|*.toml|*.cfg)            printf '#'  ;;
        *) return 1 ;;
    esac
}

has_header() {
    # Idempotence check: identifier anywhere in the first 10 lines.
    head -n 10 "$1" 2>/dev/null | grep -q 'SPDX-License-Identifier'
}

# ------------------------------------------------------------------ insertion

insert_header() {
    # $1 = path, $2 = comment prefix. Writes in place, preserving mode and inode.
    local f="$1" c="$2" tmp first_line shebang="" body_start=1

    first_line="$(head -n 1 "$f" 2>/dev/null || true)"
    case "$first_line" in
        '#!'*) shebang="$first_line"; body_start=2 ;;
    esac

    tmp="$(mktemp "${TMPDIR:-/tmp}/spdx.XXXXXX")"
    # shellcheck disable=SC2064
    trap "rm -f '$tmp'" RETURN

    {
        [ -n "$shebang" ] && printf '%s\n' "$shebang"
        [ -n "$COPYRIGHT" ] && printf '%s Copyright (C) %s\n' "$c" "$COPYRIGHT"
        printf '%s SPDX-License-Identifier: %s\n' "$c" "$LICENSE_ID"

        # A blank separator, unless the next original line is already blank.
        local next
        next="$(sed -n "${body_start}p" "$f" 2>/dev/null || true)"
        [ -n "$next" ] && printf '\n'

        tail -n "+${body_start}" "$f"
    } > "$tmp"

    # `cat >` rather than `mv` so the original file's mode, owner and inode survive.
    cat "$tmp" > "$f"
}

describe_insertion() {
    # What the top of the file would look like, for --dry-run.
    local f="$1" c="$2" first_line
    first_line="$(head -n 1 "$f" 2>/dev/null || true)"
    case "$first_line" in
        '#!'*) printf '      after shebang: %s SPDX-License-Identifier: %s\n' "$c" "$LICENSE_ID" ;;
        '//!'*|'#!['*)
            printf '      line 1 (above %s): %s SPDX-License-Identifier: %s\n' \
                   "$(printf '%s' "$first_line" | cut -c1-32)" "$c" "$LICENSE_ID" ;;
        *) printf '      line 1: %s SPDX-License-Identifier: %s\n' "$c" "$LICENSE_ID" ;;
    esac
}

# ---------------------------------------------------------------------- main

scanned=0; skipped=0; changed=0; unsupported=0; missing=0
module_doc=0
declare -a would_change=()

while IFS= read -r f; do
    [ -n "$f" ] || continue
    if [ ! -f "$f" ]; then
        printf 'add-spdx-headers: not a file, skipping: %s\n' "$f" >&2
        continue
    fi

    if ! prefix="$(comment_prefix "$f")"; then
        unsupported=$((unsupported + 1))
        continue
    fi

    scanned=$((scanned + 1))

    if has_header "$f"; then
        skipped=$((skipped + 1))
        continue
    fi

    missing=$((missing + 1))
    case "$(head -n 1 "$f" 2>/dev/null || true)" in
        '//!'*) module_doc=$((module_doc + 1)) ;;
    esac

    rel="${f#"$REPO_ROOT"/}"
    if [ "$DRY_RUN" -eq 1 ]; then
        would_change+=("$rel")
        if [ "$QUIET" -eq 0 ]; then
            printf '  would add  %s\n' "$rel"
            describe_insertion "$f" "$prefix"
        fi
    else
        insert_header "$f" "$prefix"
        changed=$((changed + 1))
        [ "$QUIET" -eq 0 ] && printf '  added      %s\n' "$rel"
    fi
done < <(collect_files)

printf '\n'
if [ "$DRY_RUN" -eq 1 ]; then
    printf 'dry run: %d file(s) scanned, %d already have a header, %d would be changed' \
           "$scanned" "$skipped" "$missing"
    [ "$module_doc" -gt 0 ] && printf ' (%d start with a //! module doc, which is preserved below the header)' "$module_doc"
    printf '\n'
    [ "$unsupported" -gt 0 ] && printf 'note: %d file(s) skipped, unrecognised extension\n' "$unsupported"
    printf 'identifier: %s%s\n' "$LICENSE_ID" \
           "$( [ -n "$COPYRIGHT" ] && printf ' + copyright line "%s"' "$COPYRIGHT" )"
    printf 'nothing was written. Re-run with --apply to write these headers.\n'
    if [ "$CHECK" -eq 1 ] && [ "$missing" -gt 0 ]; then
        printf '\ncheck failed: %d file(s) lack an SPDX identifier. Run scripts/add-spdx-headers.sh\n' "$missing" >&2
        exit 1
    fi
else
    printf '%d file(s) scanned, %d already had a header, %d changed.\n' "$scanned" "$skipped" "$changed"
    if [ "$changed" -gt 0 ]; then
        printf 'next: `cargo fmt --check` and `cargo check` to confirm nothing shifted, then review the diff.\n'
    fi
fi
