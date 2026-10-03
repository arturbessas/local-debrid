#!/data/data/com.termux/files/usr/bin/bash
# Installs Local Debrid inside Termux (alternative to the APK).
# Usage: tar xzf localdebrid-termux-arm64-*.tar.gz && cd localdebrid-termux-arm64-* && ./install.sh
set -euo pipefail
: "${PREFIX:?run this inside Termux}"

install -m 755 localdebrid "$PREFIX/bin/localdebrid"
cat > "$PREFIX/bin/localdebrid-start" <<'START'
#!/data/data/com.termux/files/usr/bin/bash
# Keeps the CPU awake while the server runs (needs the Termux:API app for notifications).
termux-wake-lock 2>/dev/null || true
trap 'termux-wake-unlock 2>/dev/null || true' EXIT
exec localdebrid --data-dir "$HOME/.localdebrid" "$@"
START
chmod 755 "$PREFIX/bin/localdebrid-start"
echo "Installed. Start with: localdebrid-start"
echo "Then open the dashboard printed in the log (http://<phone-ip>:8765/)."
