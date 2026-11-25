#!/usr/bin/env bash
# Pemasang Panel MikroTik untuk Ubuntu/Debian (systemd).
#
#   curl -fsSL https://raw.githubusercontent.com/nirvagold/mikrotik-panel/main/deploy/install.sh | sudo bash
#
# Yang dilakukan:
#  1. unduh rilis terbaru dari GitHub dan cocokkan SHA256
#  2. bila belum ada /etc/mikrotik-panel/config.toml: tanya port, akses, router, kata sandi, lalu buat config
#  3. pasang layanan systemd mikrotik-panel + pembaru otomatis mikrotik-panel-update (timer tiap jam)
#  4. nyalakan panel dan cek kesehatannya
# Aman dijalankan ulang: config yang sudah ada tidak disentuh.
set -euo pipefail

REPO="${REPO:-nirvagold/mikrotik-panel}"
ETC=/etc/mikrotik-panel
CONFIG=$ETC/config.toml

say() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31mGagal:\033[0m %s\n' "$*" >&2; exit 1; }
ask() { # ask VAR "pertanyaan" "bawaan"
  local __v; read -r -p "$2 [${3}]: " __v </dev/tty || true; printf -v "$1" '%s' "${__v:-$3}"
}
ask_secret() { # ask_secret VAR "pertanyaan"
  local __v=""; while [ -z "$__v" ]; do read -r -s -p "$2: " __v </dev/tty || true; echo >&2; done; printf -v "$1" '%s' "$__v"
}

[ "$(id -u)" = 0 ] || die "jalankan dengan sudo"
command -v systemctl >/dev/null || die "butuh systemd"
command -v curl >/dev/null || { apt-get update -qq && apt-get install -y -qq curl; }
case "$(uname -m)" in x86_64|amd64) ARCH=x86_64 ;; aarch64|arm64) ARCH=aarch64 ;; *) die "arsitektur $(uname -m) belum didukung" ;; esac

say "Mencari rilis terbaru $REPO"
TAG=$(curl -fsSL "${API_URL:-https://api.github.com}/repos/$REPO/releases/latest" | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*"\([^"]*\)"$/\1/')
[ -n "$TAG" ] || die "belum ada rilis di GitHub ($REPO)"
say "Rilis: $TAG ($ARCH)"

TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
URL="${GITHUB_URL:-https://github.com}/$REPO/releases/download/$TAG"
FILES="mikrotik-panel-$ARCH mikrotik-panel-update mikrotik-panel.service mikrotik-panel-update.service mikrotik-panel-update.timer config.example.toml"
curl -fsSL -o "$TMP/SHA256SUMS" "$URL/SHA256SUMS"
for f in $FILES; do
  curl -fsSL -o "$TMP/$f" "$URL/$f" || die "unduh $f gagal"
  (cd "$TMP" && grep " $f\$" SHA256SUMS | sha256sum -c --quiet -) || die "hash $f tidak cocok"
done
chmod 755 "$TMP/mikrotik-panel-$ARCH" "$TMP/mikrotik-panel-update"
BIN="$TMP/mikrotik-panel-$ARCH"
"$BIN" version >/dev/null || die "binary tidak bisa dijalankan di server ini"

install -d -m700 "$ETC"
if [ -f "$CONFIG" ]; then
  say "Config $CONFIG sudah ada, tidak diubah"
else
  say "Membuat config baru (Enter = pakai nilai dalam kurung)"
  ask PORT "Port panel (panel lama memakai 8765-8767)" "8780"
  ask DIRECT "Buka panel langsung lewat http://IP-VPS:$PORT? (y = ya, n = hanya lewat reverse proxy/HTTPS)" "y"
  ask RHOST "IP router MikroTik" "10.10.10.2"
  ask RPORT "Port API router (8728 biasa, 8729 API-SSL)" "8728"
  ask RUSER "User API router" "panel"
  ask_secret RPASS "Kata sandi user API router"
  APASS=""; while [ "${#APASS}" -lt 10 ]; do ask_secret APASS "Kata sandi admin panel (min. 10 karakter)"; done

  KEY=$("$BIN" gen-key)
  HASH=$(printf '%s\n' "$APASS" | "$BIN" hash-password 2>/dev/null)
  if [ "$DIRECT" = y ] || [ "$DIRECT" = Y ]; then BIND=0.0.0.0; SECURE=false; else BIND=127.0.0.1; SECURE=true; fi
  sed -e "s|^bind = .*|bind = \"$BIND\"|" \
      -e "s|^port = .*|port = $PORT|" \
      -e "s|^secret_key = .*|secret_key = \"$KEY\"|" \
      -e "s|^admin_password_hash = .*|admin_password_hash = \"$HASH\"|" \
      -e "s|^cookie_secure = .*|cookie_secure = $SECURE|" \
      -e "s|^host = .*|host = \"$RHOST\"|" \
      -e "s|^user = .*|user = \"$RUSER\"|" \
      "$TMP/config.example.toml" > "$CONFIG"
  # port router: baris "port" kedua (di bagian [router])
  awk -v rp="$RPORT" '/^\[router\]/{r=1} r && /^port = /{$0="port = " rp; r=0} {print}' "$CONFIG" > "$CONFIG.tmp" && mv "$CONFIG.tmp" "$CONFIG"
  ENC=$(printf '%s\n' "$RPASS" | "$BIN" encrypt --config "$CONFIG" 2>/dev/null) || die "gagal mengenkripsi kata sandi router"
  sed -i "s|^password_encrypted = .*|password_encrypted = \"$ENC\"|" "$CONFIG"
  chmod 600 "$CONFIG"
  unset RPASS APASS
  if [ "$RPORT" = 8729 ]; then
    say "API-SSL dipilih: isi tls = true dan tls_fingerprint (mikrotik-panel router-fingerprint) di $CONFIG"
  fi
  [ "$BIND" = 0.0.0.0 ] && say "Catatan: panel lewat HTTP biasa. Sebaiknya batasi port $PORT di firewall hanya untuk IP kamu."
fi

say "Memasang layanan systemd"
install -m755 "$TMP/mikrotik-panel-update" /usr/local/sbin/mikrotik-panel-update
install -m644 "$TMP/mikrotik-panel.service" "$TMP/mikrotik-panel-update.service" "$TMP/mikrotik-panel-update.timer" /etc/systemd/system/
[ -f "$ETC/update.conf" ] || cat > "$ETC/update.conf" <<EOF
# Pengaturan pembaru otomatis. Matikan update otomatis: systemctl disable --now mikrotik-panel-update.timer
REPO="$REPO"
KEEP=3
HEALTH_TIMEOUT=45
EOF
systemctl daemon-reload
systemctl enable "mikrotik-panel.service" >/dev/null

say "Memasang $TAG"
/usr/local/sbin/mikrotik-panel-update --version "$TAG"
systemctl enable --now mikrotik-panel-update.timer >/dev/null

PORT=$(sed -n 's/^port = \([0-9]*\).*/\1/p' "$CONFIG" | head -1)
say "Selesai. Panel $TAG berjalan di port $PORT."
say "Update otomatis aktif (cek tiap jam). Status: sudo mikrotik-panel-update --status"
