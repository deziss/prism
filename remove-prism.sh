#!/usr/bin/env bash

set -euo pipefail

echo "========================================"
echo "       PRISM COMPLETE REMOVAL"
echo "========================================"

# ------------------------------------------------------------
# Safety
# ------------------------------------------------------------

if [[ "${EUID}" -eq 0 ]]; then
    echo
    echo "ERROR: Do not run this script with sudo."
    echo "Run:"
    echo "  ./remove-prism.sh"
    echo
    exit 1
fi

USER_HOME="$HOME"

echo
echo "User : $(whoami)"
echo "Home : $USER_HOME"

# ------------------------------------------------------------
# Paths
# ------------------------------------------------------------

SYSTEMD_USER="$USER_HOME/.config/systemd/user"

PRISM_BIN="$USER_HOME/.local/bin/prism"
PRISM_BRIDGE="$USER_HOME/.local/bin/prism-bridge"

PRISM_CONFIG="$USER_HOME/.config/prism"
PRISM_DATA="$USER_HOME/.local/share/prism"

PROXY_SERVICE="$SYSTEMD_USER/prism-proxy.service"
BRIDGE_SERVICE="$SYSTEMD_USER/prism-bridge.service"

PROXY_WANTS="$SYSTEMD_USER/default.target.wants/prism-proxy.service"
BRIDGE_WANTS="$SYSTEMD_USER/default.target.wants/prism-bridge.service"

ANTIGRAVITY_SETTINGS="$USER_HOME/.config/Antigravity IDE/User/settings.json"

# ------------------------------------------------------------
# 1. Stop services
# ------------------------------------------------------------

echo
echo "[1/11] Stopping PRISM services..."

systemctl --user stop prism-proxy.service 2>/dev/null || true
systemctl --user stop prism-bridge.service 2>/dev/null || true

# ------------------------------------------------------------
# 2. Disable services
# ------------------------------------------------------------

echo
echo "[2/11] Disabling PRISM services..."

systemctl --user disable prism-proxy.service 2>/dev/null || true
systemctl --user disable prism-bridge.service 2>/dev/null || true

# ------------------------------------------------------------
# 3. Remove systemd service files
# ------------------------------------------------------------

echo
echo "[3/11] Removing PRISM systemd services..."

rm -f "$PROXY_SERVICE"
rm -f "$BRIDGE_SERVICE"
rm -f "$PROXY_WANTS"
rm -f "$BRIDGE_WANTS"

systemctl --user daemon-reload 2>/dev/null || true
systemctl --user reset-failed 2>/dev/null || true

# ------------------------------------------------------------
# 4. Clear systemd environment
# ------------------------------------------------------------

echo
echo "[4/11] Clearing systemd PRISM environment..."

systemctl --user unset-environment \
    HTTP_PROXY HTTPS_PROXY \
    http_proxy https_proxy \
    ALL_PROXY all_proxy \
    PRISM_HUB_URL \
    NODE_EXTRA_CA_CERTS \
    CURL_CA_BUNDLE \
    REQUESTS_CA_BUNDLE \
    SSL_CERT_FILE \
    2>/dev/null || true

# ------------------------------------------------------------
# 5. Remove PRISM shell configuration
# ------------------------------------------------------------

echo
echo "[5/11] Removing PRISM shell configuration..."

for file in \
    "$USER_HOME/.bashrc" \
    "$USER_HOME/.profile" \
    "$USER_HOME/.zshrc"
do
    [[ -f "$file" ]] || continue

    # Backup before modification
    cp "$file" "$file.before-prism-removal"

    # Remove PRISM proxy block
    sed -i \
        '/# PRISM — transparent LLM proxy/,/export NO_PROXY=localhost,127.0.0.1,::1/d' \
        "$file"

    # Remove proxy variables
    sed -i \
        '/^[[:space:]]*export[[:space:]]\+\(HTTP_PROXY\|HTTPS_PROXY\|http_proxy\|https_proxy\|ALL_PROXY\|all_proxy\)=.*27181/d' \
        "$file"

    # Remove PRISM Hub URL
    sed -i \
        '/^[[:space:]]*export[[:space:]]\+PRISM_HUB_URL=/d' \
        "$file"

    # Remove PRISM CA variables
    sed -i \
        '/^[[:space:]]*export[[:space:]]\+\(NODE_EXTRA_CA_CERTS\|CURL_CA_BUNDLE\|REQUESTS_CA_BUNDLE\|SSL_CERT_FILE\)=.*prism/d' \
        "$file"

    # Remove PRISM markers
    sed -i \
        '/^[[:space:]]*# end PRISM[[:space:]]*$/d' \
        "$file"

    # Remove PRISM shim block
    sed -i \
        '/# >>> PRISM shims >>>/,/# <<< PRISM shims <<</d' \
        "$file"

    # Remove standalone PRISM shim PATH
    sed -i \
        '\|\.local/share/prism/shims|d' \
        "$file"
done

# ------------------------------------------------------------
# 6. Clean current shell environment
# ------------------------------------------------------------

echo
echo "[6/11] Cleaning current shell environment..."

unset HTTP_PROXY HTTPS_PROXY
unset http_proxy https_proxy
unset ALL_PROXY all_proxy
unset PRISM_HUB_URL
unset NODE_EXTRA_CA_CERTS
unset CURL_CA_BUNDLE
unset REQUESTS_CA_BUNDLE
unset SSL_CERT_FILE

# ------------------------------------------------------------
# 7. Remove PRISM from Antigravity settings
# ------------------------------------------------------------

echo
echo "[7/11] Cleaning Antigravity PRISM configuration..."

if [[ -f "$ANTIGRAVITY_SETTINGS" ]]; then

    BACKUP="${ANTIGRAVITY_SETTINGS}.before-prism-removal"

    cp "$ANTIGRAVITY_SETTINGS" "$BACKUP"

    python3 - "$ANTIGRAVITY_SETTINGS" <<'PY'
import json
import sys

path = sys.argv[1]

with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)

# Remove the IDE-level proxy settings PRISM's setup adds.
#
# The previous guard was `"27181" in str(data[key])`, which is never true for a
# boolean, so `http.proxyStrictSSL: false` survived every removal. That is not a
# cosmetic leftover: it leaves the IDE skipping TLS certificate verification for
# all of its traffic, permanently, long after PRISM is gone. `http.proxySupport`
# was not listed at all.
#
# `http.proxy` is deleted unconditionally (it is the hard dependency on a local
# daemon that makes the IDE lose networking whenever PRISM is down). The other two
# are deleted only when they hold the value PRISM sets, so a user who deliberately
# chose them keeps their setting.
if "http.proxy" in data:
    del data["http.proxy"]

if data.get("http.proxyStrictSSL") is False:
    del data["http.proxyStrictSSL"]

if data.get("http.proxySupport") == "override":
    del data["http.proxySupport"]

# Known PRISM-injected environment values.
def clean_environment(obj):
    if not isinstance(obj, dict):
        return

    for key in list(obj.keys()):
        value = obj[key]

        if key in {
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "http_proxy",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
            "PRISM_HUB_URL",
            "NODE_EXTRA_CA_CERTS",
            "CURL_CA_BUNDLE",
            "REQUESTS_CA_BUNDLE",
            "SSL_CERT_FILE",
        }:
            if (
                "27181" in str(value)
                or "27183" in str(value)
                or "prism" in str(value).lower()
            ):
                del obj[key]
                continue

        if key == "PATH" and "prism/shims" in str(value):
            del obj[key]
            continue

        if isinstance(value, dict):
            clean_environment(value)
        elif isinstance(value, list):
            for item in value:
                clean_environment(item)

clean_environment(data)

with open(path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=4, ensure_ascii=False)
    f.write("\n")

print("Antigravity settings cleaned.")
PY

else
    echo "Antigravity settings.json not found; skipping."
fi

# ------------------------------------------------------------
# 8. Kill leftover PRISM processes
# ------------------------------------------------------------

echo
echo "[8/11] Stopping leftover PRISM processes..."

pkill -f "$USER_HOME/.local/bin/prism-bridge" 2>/dev/null || true
pkill -f "$USER_HOME/.local/bin/prism" 2>/dev/null || true

# ------------------------------------------------------------
# 9. Remove PRISM installation
# ------------------------------------------------------------

echo
echo "[9/11] Removing PRISM installation..."

rm -f "$PRISM_BIN"
rm -f "$PRISM_BRIDGE"

rm -rf "$PRISM_CONFIG"
rm -rf "$PRISM_DATA"

# ------------------------------------------------------------
# 10. Remove PRISM environment files
# ------------------------------------------------------------

echo
echo "[10/11] Checking environment.d..."

if [[ -d "$USER_HOME/.config/environment.d" ]]; then
    while IFS= read -r -d '' file; do
        if grep -qE \
            '27181|27183|PRISM_HUB_URL|\.local/share/prism|HTTP_PROXY|HTTPS_PROXY' \
            "$file" 2>/dev/null; then

            echo "Removing PRISM environment file:"
            echo "  $file"

            rm -f "$file"
        fi
    done < <(
        find "$USER_HOME/.config/environment.d" \
            -type f \
            -print0 2>/dev/null
    )
fi

# ------------------------------------------------------------
# 11. Final verification
# ------------------------------------------------------------

echo
echo "[11/11] Final verification..."

echo
echo "--- PRISM processes ---"

if pgrep -af 'prism|prism-bridge' 2>/dev/null; then
    echo "WARNING: PRISM process still exists."
else
    echo "OK: No PRISM processes."
fi

echo
echo "--- PRISM systemd services ---"

if systemctl --user list-units --all 2>/dev/null |
    grep -qi prism; then
    systemctl --user list-units --all | grep -i prism || true
else
    echo "OK: No PRISM systemd units."
fi

echo
echo "--- Shell environment ---"

if env | grep -iE \
    'HTTP_PROXY|HTTPS_PROXY|ALL_PROXY|PRISM_HUB_URL|NODE_EXTRA_CA_CERTS|CURL_CA_BUNDLE|REQUESTS_CA_BUNDLE|SSL_CERT_FILE' \
    2>/dev/null; then

    echo "WARNING: PRISM-related environment variable remains."
else
    echo "OK: Shell environment clean."
fi

echo
echo "--- Systemd environment ---"

if systemctl --user show-environment 2>/dev/null |
    grep -iE \
    'HTTP_PROXY|HTTPS_PROXY|ALL_PROXY|PRISM_HUB_URL|NODE_EXTRA_CA_CERTS|CURL_CA_BUNDLE|REQUESTS_CA_BUNDLE|SSL_CERT_FILE'; then

    echo "WARNING: PRISM variables remain in systemd."
else
    echo "OK: Systemd environment clean."
fi

echo
echo "--- PRISM ports ---"

if ss -ltnp 2>/dev/null | grep -E ':27181|:27183'; then
    echo "WARNING: PRISM port still listening."
else
    echo "OK: Ports 27181 and 27183 are closed."
fi

echo
echo "--- Antigravity settings ---"

if [[ -f "$ANTIGRAVITY_SETTINGS" ]]; then
    if grep -nEi \
        '27181|27183|PRISM_HUB_URL|prism/shims|NODE_EXTRA_CA_CERTS|CURL_CA_BUNDLE|REQUESTS_CA_BUNDLE|SSL_CERT_FILE' \
        "$ANTIGRAVITY_SETTINGS" 2>/dev/null; then

        echo "WARNING: PRISM configuration remains in Antigravity settings."
    else
        echo "OK: Antigravity settings are clean."
    fi
else
    echo "Antigravity settings.json not found."
fi

echo
echo "--- PRISM binaries ---"

if [[ -e "$PRISM_BIN" || -e "$PRISM_BRIDGE" ]]; then
    echo "WARNING: PRISM binary still exists."
else
    echo "OK: PRISM binaries removed."
fi

echo
echo "========================================"
echo "       PRISM REMOVAL COMPLETED"
echo "========================================"

echo
echo "Your source projects were NOT deleted:"
echo "  $USER_HOME/Desktop/learn/prism"
echo "  $USER_HOME/Desktop/learn/prism-hub"

echo
echo "Antigravity settings backup:"
echo "  $ANTIGRAVITY_SETTINGS.before-prism-removal"

echo
echo "Open a NEW terminal before testing."
echo
