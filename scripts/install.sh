#!/usr/bin/env bash
set -e

# ==============================================================================
# PRISM Production Installer & Setup Script
# Builds release binary, initializes XDG storage, configures systemd daemons,
# and offers an interactive User Guide walkthrough.
# ==============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN_DIR="$HOME/.local/bin"
SHOW_GUIDE=false

# Parse command line flags
for arg in "$@"; do
    case $arg in
        -g|--guide)
            SHOW_GUIDE=true
            shift
            ;;
        -h|--help)
            echo "Usage: ./install.sh [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  -g, --guide    Open the interactive User Guide immediately after installation"
            echo ""
            echo "Re-run this script to update an existing installation: it rebuilds,"
            echo "replaces the binary, and restarts any PRISM service already running."
            echo "  -h, --help     Show this help message"
            exit 0
            ;;
    esac
done

# Is this an install or an upgrade? Asked before anything is written, because the
# answer changes what has to happen afterwards: an upgrade has running daemons holding
# the old binary open, and they keep serving it until they are restarted.
PREVIOUS_VERSION=""
if [ -x "$BIN_DIR/prism" ]; then
    PREVIOUS_VERSION="$("$BIN_DIR/prism" --version 2>/dev/null | awk '{print $2}')"
fi

echo "=========================================================================="
if [ -n "$PREVIOUS_VERSION" ]; then
    echo "          Updating PRISM (Enterprise AI Token Optimizer)                 "
else
    echo "          Installing PRISM (Enterprise AI Token Optimizer)               "
fi
echo "=========================================================================="
echo "  Source repository: $REPO_ROOT"
echo "  Target binary dir: $BIN_DIR"
if [ -n "$PREVIOUS_VERSION" ]; then
    echo "  Installed version: $PREVIOUS_VERSION"
fi
echo ""

mkdir -p "$BIN_DIR"

# 1. Build release binary with Cargo LTO
echo "[1/5] Compiling PRISM release binary with Link-Time Optimization (LTO)..."
cd "$REPO_ROOT"
cargo build --release --quiet
install -m 0755 "$REPO_ROOT/target/release/prism" "$BIN_DIR/prism"
chmod +x "$BIN_DIR/prism"
NEW_VERSION="$("$BIN_DIR/prism" --version 2>/dev/null | awk '{print $2}')"
if [ -n "$PREVIOUS_VERSION" ] && [ "$PREVIOUS_VERSION" != "$NEW_VERSION" ]; then
    echo "  ✓ Updated binary: $BIN_DIR/prism ($PREVIOUS_VERSION -> $NEW_VERSION)"
else
    echo "  ✓ Installed binary: $BIN_DIR/prism ($NEW_VERSION)"
fi

# 2. Run PRISM initialization (XDG directories + CA generation only)
#
# Deliberately WITHOUT --shims and --trust-ca. This used to be a bare
# `init --global < /dev/null`, and because init installed shims unconditionally, a
# plain install put prism in front of ~102 commands -- `ls`, `grep`, `find`, `env`,
# `ps`, `systemctl` among them -- rewrote every shell rc file, and injected a
# man-in-the-middle CA into every browser and Electron trust store. No prompt, and
# until now no CLI verb could undo the first two.
#
# The shims are the product's main value, so they are advertised loudly below rather
# than hidden; they are just no longer applied to someone's shell without consent.
echo "[2/5] Initializing XDG storage and CA certificates..."
"$BIN_DIR/prism" init --global < /dev/null

# 3. Install toggle helper scripts
echo "[3/5] Installing system control helpers (prism-enable, prism-disable, prism-env)..."

# The manifest goes in first: every helper sources it, and prism-disable needs to
# find it after a checkout is gone. It is the single list of everything PRISM
# writes outside its own tree, and the reason the enable and disable sides can no
# longer drift apart.
install -m 0644 "$SCRIPT_DIR/prism-manifest.sh" "$BIN_DIR/prism-manifest.sh"

for helper in prism-enable prism-disable prism-env; do
    install -m 0755 "$SCRIPT_DIR/$helper" "$BIN_DIR/$helper"
done
ln -sf "$BIN_DIR/prism-enable" "$BIN_DIR/prism-on" 2>/dev/null || true
ln -sf "$BIN_DIR/prism-disable" "$BIN_DIR/prism-off" 2>/dev/null || true
echo "  ✓ Helpers active: prism-enable (alias: prism-on), prism-disable (alias: prism-off), prism-env"

# certutil is what lets Chromium/Electron apps and Firefox trust the MITM CA
if ! command -v certutil > /dev/null 2>&1; then
    echo "  ! certutil not found. Install libnss3-tools, then re-run prism-enable,"
    echo "    or Electron IDEs and browsers will reject intercepted hosts:"
    echo "      sudo apt install libnss3-tools   # Debian/Ubuntu"
fi

# 4. Restart any daemon still holding the previous binary.
#
# `install` replaces the file, but a running `prism serve` / `prism mcp` keeps executing
# the inode it started with, so an upgrade silently left the proxy and the MCP server on
# the old build until someone restarted them by hand. That is the failure this step
# exists to prevent: `prism --version` reports the new one while the daemons serve the
# old one, and nothing says so.
#
# Only restarts what is already running: starting a daemon the operator had deliberately
# stopped would be an install deciding policy for them.
echo "[4/5] Restarting running PRISM services..."
RESTARTED=""
for unit in prism-proxy prism-mcp prism-shipper; do
    if systemctl --user is-active --quiet "$unit" 2>/dev/null; then
        systemctl --user restart "$unit" 2>/dev/null && RESTARTED="$RESTARTED $unit"
    fi
done
if [ -n "$RESTARTED" ]; then
    echo "  ✓ Restarted on the new binary:$RESTARTED"
else
    echo "  - No PRISM services were running; nothing to restart."
fi

echo "[5/5] Installation completed successfully!"
echo "=========================================================================="
echo "  PRISM is installed at: $BIN_DIR/prism"
echo "  Run 'prism --help' or 'prism guide' at any time."
echo ""
echo "  Nothing has been added to your shell. To turn on automatic filtering:"
echo ""
echo "    prism shim install --path    # put prism in front of git, ls, grep, ..."
echo "    prism shim uninstall         # and take it back out, rc block included"
echo ""
echo "  If you route traffic through the proxy, browsers and Electron IDEs need the CA:"
echo "    prism init --global --trust-ca"
echo ""
echo "  To see what PRISM has enabled, or to take it all back off:"
echo "    prism status"
echo "    prism uninstall              # audit only, changes nothing"
echo "    prism uninstall --remove"
echo "=========================================================================="

# 4. User Guide Option
if [ "$SHOW_GUIDE" = true ]; then
    "$BIN_DIR/prism" guide
else
    # Interactive prompt if connected to a terminal
    if [ -t 0 ]; then
        echo ""
        read -r -p "  [?] Would you like to explore the interactive PRISM User Guide now? [y/N]: " answer
        case "$answer" in
            [yY][eE][sS]|[yY])
                "$BIN_DIR/prism" guide
                ;;
            *)
                echo "  You can view the guide at any time by running: prism guide"
                ;;
        esac
    fi
fi
