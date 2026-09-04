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
cp "$REPO_ROOT/target/release/prism" "$BIN_DIR/prism"
chmod +x "$BIN_DIR/prism"
echo "  ✓ Installed binary: $BIN_DIR/prism"

# 2. Run PRISM initialization (sets up CA certificates & XDG directories)
echo "[2/4] Initializing XDG storage and CA certificates..."
"$BIN_DIR/prism" init --global < /dev/null

# 3. Install toggle helper scripts
echo "[3/4] Installing system control helpers (prism-enable, prism-disable)..."
ln -sf "$BIN_DIR/prism-enable" "$BIN_DIR/prism-on" 2>/dev/null || true
ln -sf "$BIN_DIR/prism-disable" "$BIN_DIR/prism-off" 2>/dev/null || true
echo "  ✓ Helpers active: prism-enable (alias: prism-on), prism-disable (alias: prism-off)"

echo "[4/4] Installation completed successfully!"
echo "=========================================================================="
echo "  PRISM is installed at: $BIN_DIR/prism"
echo "  Run 'prism --help' or 'prism guide' at any time."
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
