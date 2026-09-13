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
            echo "  -h, --help     Show this help message"
            exit 0
            ;;
    esac
done

echo "=========================================================================="
echo "          Installing PRISM (Enterprise AI Token Optimizer)               "
echo "=========================================================================="
echo "  Source repository: $REPO_ROOT"
echo "  Target binary dir: $BIN_DIR"
echo ""

mkdir -p "$BIN_DIR"

# 1. Build release binary with Cargo LTO
echo "[1/4] Compiling PRISM release binary with Link-Time Optimization (LTO)..."
cd "$REPO_ROOT"
cargo build --release --quiet
install -m 0755 "$REPO_ROOT/target/release/prism" "$BIN_DIR/prism"
chmod +x "$BIN_DIR/prism"
echo "  ✓ Installed binary: $BIN_DIR/prism"

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
echo "[2/4] Initializing XDG storage and CA certificates..."
"$BIN_DIR/prism" init --global < /dev/null

# 3. Install toggle helper scripts
echo "[3/4] Installing system control helpers (prism-enable, prism-disable, prism-env)..."

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

echo "[4/4] Installation completed successfully!"
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
