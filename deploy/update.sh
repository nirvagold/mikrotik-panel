#!/usr/bin/env bash
# mikrotik-panel-update: pasang rilis terbaru dari GitHub dengan aman.
#
#   mikrotik-panel-update            cek dan pasang bila ada rilis baru (dipakai timer)
#   mikrotik-panel-update --check    hanya cek, tidak memasang
#   mikrotik-panel-update --version TAG   pasang rilis tertentu (mis. v0.1.2), termasuk turun versi
#   mikrotik-panel-update --rollback kembali ke rilis sebelumnya (database terbaru dipertahankan; cadangan dipakai hanya bila perlu)
#   mikrotik-panel-update --status   tampilkan versi terpasang dan hasil update terakhir
#   mikrotik-panel-update --request  jalankan permintaan dari tombol panel (dipicu unit .path)
#
# Alur: unduh binary + SHA256SUMS → cocokkan hash → hentikan panel → cadangkan database →
# ganti symlink current → nyalakan → cek /api/health sampai 45 detik. Gagal = database dan
# rilis lama dikembalikan otomatis, rilis itu ditandai rusak dan tidak dicoba lagi oleh timer.
set -euo pipefail

CONF=/etc/mikrotik-panel/update.conf
REPO="nirvagold/mikrotik-panel"
BASE=/opt/mikrotik-panel
PANEL_CONFIG=/etc/mikrotik-panel/config.toml
SERVICE=mikrotik-panel
KEEP=3
HEALTH_TIMEOUT=45
STATE=/var/lib/mikrotik-panel-update
REQUEST=/var/lib/mikrotik-panel/update-request
UNITS="mikrotik-panel.service mikrotik-panel-update.service mikrotik-panel-update.timer mikrotik-panel-update-request.service mikrotik-panel-update-request.path"
# shellcheck source=/dev/null
[ -r "$CONF" ] && . "$CONF"

log() { echo "[$(date '+%F %T')] $*" | tee -a "$STATE/update.log" >&2; }
die() { log "GAGAL: $*"; exit 1; }

[ "$(id -u)" = 0 ] || { echo "Jalankan sebagai root (sudo)." >&2; exit 1; }
mkdir -p "$STATE" "$BASE/releases" "$BASE/backups"
exec 9>"$STATE/lock"
if [ "${1:-}" = --request ]; then
  # Tombol di panel: tunggu giliran (mis. update otomatis sedang jalan), jangan keluar diam-diam.
  # Kalau keluar, file permintaan tetap ada dan systemd memicu ulang terus sampai unitnya macet.
  flock -w 900 9 || { rm -f "$REQUEST"; echo "Update lain terlalu lama berjalan." >&2; exit 0; }
else
  flock -n 9 || { echo "Update lain sedang berjalan." >&2; exit 0; }
fi

case "$(uname -m)" in
  x86_64|amd64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
  *) die "arsitektur $(uname -m) belum didukung" ;;
esac

# Status untuk ditampilkan panel (panel berjalan sebagai user dinamis, jadi file ini dibuat 644).
write_status() { # $1 = tag terbaru di GitHub (boleh kosong), $2 = hasil cek: ok|gagal
  local last_tag="" last_at="" last_result="" f
  read -r last_tag last_at last_result < "$STATE/last" 2>/dev/null || true
  chmod 755 "$STATE"
  f="$STATE/status.json"
  local latest="${1:-}"
  [ -n "$latest" ] || latest=$(sed -n 's/.*"latest":"\([^"]*\)".*/\1/p' "$f" 2>/dev/null || true)
  printf '{"installed":"%s","latest":"%s","checked_at":"%s","check":"%s","last_tag":"%s","last_at":"%s","last_result":"%s","auto":%s,"on_demand":%s}\n' \
    "$(current_tag)" "$latest" "$(date -Is)" "${2:-ok}" "$last_tag" "$last_at" "$last_result" \
    "$(systemctl is-enabled mikrotik-panel-update.timer 9>&- >/dev/null 2>&1 && echo true || echo false)" \
    "$(systemctl is-active mikrotik-panel-update-request.path 9>&- >/dev/null 2>&1 && echo true || echo false)" > "$f.tmp"
  chmod 644 "$f.tmp" && mv -f "$f.tmp" "$f"
}

current_tag() { [ -L "$BASE/current" ] && basename "$(readlink "$BASE/current")" || true; }

db_path() {
  local p
  p=$(sed -n 's/^[[:space:]]*database[[:space:]]*=[[:space:]]*"\(.*\)".*/\1/p' "$PANEL_CONFIG" 2>/dev/null | head -1)
  echo "${p:-/var/lib/mikrotik-panel/panel.sqlite3}"
}

health_url() {
  local port
  port=$(sed -n 's/^[[:space:]]*port[[:space:]]*=[[:space:]]*\([0-9]*\).*/\1/p' "$PANEL_CONFIG" 2>/dev/null | head -1)
  echo "http://127.0.0.1:${port:-8765}/api/health"
}

latest_tag() {
  curl -fsSL --max-time 30 -H 'Accept: application/vnd.github+json' \
    "${API_URL:-https://api.github.com}/repos/$REPO/releases/latest" |
    grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*"\([^"]*\)"$/\1/'
}

# Tunggu sampai panel tidak sedang mengerjakan sesuatu (top-up, agen, perintah router),
# paling lama IDLE_WAIT detik, supaya restart tidak memutus pekerjaan admin.
IDLE_WAIT=${IDLE_WAIT:-180}
wait_idle() {
  local url body i
  url=$(health_url)
  for ((i = 0; i < IDLE_WAIT; i += 5)); do
    body=$(curl -fsS --max-time 5 "$url" 2>/dev/null) || return 0
    echo "$body" | grep -q '"busy":true' || return 0
    [ "$i" = 0 ] && log "Panel sedang sibuk, menunggu sebelum restart…"
    sleep 5
  done
  log "Panel masih sibuk setelah ${IDLE_WAIT} detik; restart tetap dilanjutkan."
}

healthy() {
  local want="${1#v}" url body i
  url=$(health_url)
  for ((i = 0; i < HEALTH_TIMEOUT; i++)); do
    if body=$(curl -fsS --max-time 3 "$url" 2>/dev/null) && echo "$body" | grep -q "\"version\":\"$want\""; then
      return 0
    fi
    sleep 1
  done
  return 1
}

backup_db() { # $1 = nama folder cadangan
  local db dir
  db=$(db_path); dir="$BASE/backups/$1"
  mkdir -p "$dir"
  for f in "$db" "$db-wal" "$db-shm"; do [ -e "$f" ] && cp -a "$f" "$dir/"; done
  echo "$db" > "$dir/DB_PATH"
}

restore_db() { # $1 = nama folder cadangan
  local dir="$BASE/backups/$1" db
  [ -r "$dir/DB_PATH" ] || return 0
  db=$(cat "$dir/DB_PATH")
  rm -f "$db-wal" "$db-shm"
  for f in "$dir/$(basename "$db")" "$dir/$(basename "$db")-wal" "$dir/$(basename "$db")-shm"; do
    [ -e "$f" ] && cp -a "$f" "$(dirname "$db")/"
  done
  return 0
}

switch_to() { # atomik: symlink baru lalu rename
  ln -sfn "$BASE/releases/$1" "$BASE/current.new"
  mv -T "$BASE/current.new" "$BASE/current"
}

prune() {
  local cur keep_list
  cur=$(current_tag)
  # simpan $KEEP rilis terbaru (berdasarkan waktu pasang) + yang sedang aktif
  keep_list=$(ls -1t "$BASE/releases" | head -n "$KEEP")
  for r in "$BASE"/releases/*; do
    r=$(basename "$r")
    [ "$r" = "$cur" ] && continue
    echo "$keep_list" | grep -qx "$r" || rm -rf "${BASE:?}/releases/$r"
  done
  ls -1t "$BASE/backups" | tail -n +"$((KEEP + 1))" | while read -r b; do rm -rf "${BASE:?}/backups/$b"; done
}

install_tag() { # $1 = tag, $2 = auto|manual
  local tag="$1" mode="$2" prev tmp url sums bin
  prev=$(current_tag)
  [ "$tag" = "$prev" ] && { log "Sudah di $tag."; return 0; }
  if [ "$mode" = auto ] && grep -qx "$tag" "$STATE/bad" 2>/dev/null; then
    log "$tag pernah gagal dipasang, dilewati. Pasang manual dengan --version $tag bila sudah diperbaiki."
    return 0
  fi
  log "Memasang $tag (sebelumnya: ${prev:-belum ada})"
  tmp=$(mktemp -d); trap 'rm -rf "$tmp"' RETURN
  url="${GITHUB_URL:-https://github.com}/$REPO/releases/download/$tag"
  curl -fsSL --max-time 300 -o "$tmp/mikrotik-panel-$ARCH" "$url/mikrotik-panel-$ARCH" || die "unduh binary $tag gagal"
  curl -fsSL --max-time 60 -o "$tmp/SHA256SUMS" "$url/SHA256SUMS" || die "unduh SHA256SUMS gagal"
  sums=$(grep " mikrotik-panel-$ARCH\$" "$tmp/SHA256SUMS") || die "hash untuk $ARCH tidak ada di SHA256SUMS"
  (cd "$tmp" && echo "$sums" | sha256sum -c --quiet -) || die "hash binary tidak cocok, unduhan dibuang"
  chmod 755 "$tmp/mikrotik-panel-$ARCH"
  bin=$("$tmp/mikrotik-panel-$ARCH" version) || die "binary baru tidak bisa dijalankan di server ini"
  [ "$bin" = "${tag#v}" ] || die "versi binary ($bin) tidak sama dengan tag ($tag)"

  mkdir -p "$BASE/releases/$tag"
  install -m755 "$tmp/mikrotik-panel-$ARCH" "$BASE/releases/$tag/mikrotik-panel"
  echo "$tag" > "$BASE/releases/$tag/VERSION"

  local backup="${prev:-none}-ke-$tag-$(date +%Y%m%d%H%M%S)"
  wait_idle
  systemctl stop "$SERVICE" 9>&- 2>/dev/null || true
  backup_db "$backup"
  switch_to "$tag"
  systemctl start "$SERVICE" 9>&- || true

  if healthy "$tag"; then
    log "Berhasil: panel sekarang $tag. Cadangan database: $BASE/backups/$backup"
    echo "$tag $(date -Is) ok" > "$STATE/last"
    [ -n "$prev" ] && echo "$prev" > "$STATE/previous"
    sed -i "/^$tag\$/d" "$STATE/bad" 2>/dev/null || true
    self_update "$url" "$tmp"
    prune
    return 0
  fi

  log "Panel $tag tidak sehat dalam ${HEALTH_TIMEOUT} detik. Log terakhir:"
  journalctl -u "$SERVICE" -n 20 --no-pager 2>/dev/null | tee -a "$STATE/update.log" >&2 || true
  echo "$tag" >> "$STATE/bad"
  rm -rf "${BASE:?}/releases/$tag"
  systemctl stop "$SERVICE" 9>&- 2>/dev/null || true
  if [ -n "$prev" ] && [ -d "$BASE/releases/$prev" ]; then
    restore_db "$backup"
    switch_to "$prev"
    systemctl start "$SERVICE" 9>&- || true
    if healthy "$prev"; then log "Dikembalikan ke $prev."; else log "PERHATIAN: $prev juga tidak sehat, cek: journalctl -u $SERVICE"; fi
  fi
  echo "$tag $(date -Is) rollback" > "$STATE/last"
  die "update ke $tag dibatalkan"
}

self_update() { # perbarui skrip ini sendiri dari rilis yang sama (hash dicek)
  local url="$1" tmp="$2" sums
  curl -fsSL --max-time 60 -o "$tmp/mikrotik-panel-update" "$url/mikrotik-panel-update" 2>/dev/null || return 0
  sums=$(grep " mikrotik-panel-update\$" "$tmp/SHA256SUMS") || return 0
  (cd "$tmp" && echo "$sums" | sha256sum -c --quiet -) || return 0
  cmp -s "$tmp/mikrotik-panel-update" "$0" || install -m755 "$tmp/mikrotik-panel-update" /usr/local/sbin/mikrotik-panel-update
  update_units "$url" "$tmp"
}

update_units() { # pasang unit systemd baru/berubah dari rilis yang sama (hash dicek)
  local url="$1" tmp="$2" u sums changed=""
  for u in $UNITS; do
    curl -fsSL --max-time 60 -o "$tmp/$u" "$url/$u" 2>/dev/null || continue
    sums=$(grep " $u\$" "$tmp/SHA256SUMS") || continue
    (cd "$tmp" && echo "$sums" | sha256sum -c --quiet -) || continue
    cmp -s "$tmp/$u" "/etc/systemd/system/$u" && continue
    install -m644 "$tmp/$u" "/etc/systemd/system/$u"
    changed="$changed $u"
  done
  [ -n "$changed" ] || return 0
  log "Unit systemd diperbarui:$changed"
  systemctl daemon-reload 9>&- || true
  # tombol "Cek update" di panel butuh unit .path ini
  [ -f /etc/systemd/system/mikrotik-panel-update-request.path ] &&
    systemctl enable --now mikrotik-panel-update-request.path 9>&- >/dev/null 2>&1 || true
  # mikrotik-panel.service berubah: berlaku saat panel dinyalakan ulang berikutnya
  return 0
}

handle_request() { # isi file permintaan dari panel: hanya "check" atau "install"
  local req=""
  if [ -f "$REQUEST" ] && [ ! -L "$REQUEST" ]; then
    req=$(head -c 16 "$REQUEST" | tr -dc 'a-z')
  fi
  rm -f "$REQUEST"
  case "$req" in
    check)
      log "Permintaan dari panel: cek update"
      tag=$(latest_tag) || true
      write_status "${tag:-}" "$([ -n "${tag:-}" ] && echo ok || echo gagal)" ;;
    install)
      log "Permintaan dari panel: pasang update"
      tag=$(latest_tag) || true
      [ -n "${tag:-}" ] || { write_status "" gagal; die "tidak bisa membaca rilis terbaru dari GitHub"; }
      trap 'write_status "$tag" ok' EXIT
      install_tag "$tag" auto ;;
    *) [ -z "$req" ] || log "Permintaan panel tidak dikenal, diabaikan" ;;
  esac
}

rollback() {
  # Database TIDAK dikembalikan dulu: trafik, riwayat, dan chat sejak update tetap ada.
  # Perubahan skema hanya menambah tabel, jadi versi lama biasanya tetap bisa membacanya.
  # Bila versi lama tidak sehat dengan database sekarang, baru database cadangan dipakai.
  local cur prev backup
  cur=$(current_tag)
  prev=$(cat "$STATE/previous" 2>/dev/null || true)
  { [ -n "$prev" ] && [ "$prev" != "$cur" ] && [ -d "$BASE/releases/$prev" ]; } || prev=$(ls -1t "$BASE/releases" | grep -vx "$cur" | head -1 || true)
  [ -n "$prev" ] || die "tidak ada rilis sebelumnya"
  backup=$(ls -1t "$BASE/backups" | grep -- "^$prev-ke-$cur-" | head -1 || true)
  log "Rollback $cur → $prev"
  wait_idle
  systemctl stop "$SERVICE" 9>&- 2>/dev/null || true
  backup_db "$cur-rollback-$(date +%Y%m%d%H%M%S)"
  switch_to "$prev"
  touch "$BASE/releases/$prev"
  echo "$cur" >> "$STATE/bad"
  echo "$cur" > "$STATE/previous"
  systemctl start "$SERVICE" 9>&-
  if healthy "$prev"; then
    log "Rollback selesai, panel di $prev (database tetap yang terbaru)."
    return 0
  fi
  [ -n "$backup" ] || die "panel $prev tidak sehat setelah rollback dan tidak ada cadangan database"
  log "Panel $prev tidak sehat dengan database sekarang; memakai cadangan $backup."
  systemctl stop "$SERVICE" 9>&- 2>/dev/null || true
  restore_db "$backup"
  systemctl start "$SERVICE" 9>&-
  healthy "$prev" && log "Rollback selesai, panel di $prev (database dari cadangan)." || die "panel $prev tidak sehat setelah rollback"
}

case "${1:-}" in
  ""|--auto)
    tag=$(latest_tag) || true
    [ -n "${tag:-}" ] || { write_status "" gagal; die "tidak bisa membaca rilis terbaru dari GitHub"; }
    trap 'write_status "$tag" ok' EXIT
    install_tag "$tag" auto ;;
  --check)
    tag=$(latest_tag) || true
    write_status "${tag:-}" "$([ -n "${tag:-}" ] && echo ok || echo gagal)"
    echo "Terpasang: $(current_tag || echo -), terbaru di GitHub: ${tag:-tidak terbaca}" ;;
  --version)
    [ -n "${2:-}" ] || die "pakai: --version vX.Y.Z"
    trap 'write_status "" ok' EXIT
    install_tag "$2" manual ;;
  --rollback) trap 'write_status "" ok' EXIT; rollback ;;
  --request) handle_request ;;
  --status)
    echo "Terpasang: $(current_tag || echo -)"
    echo "Update terakhir: $(cat "$STATE/last" 2>/dev/null || echo belum pernah)"
    echo "Rilis tersimpan: $(ls -1t "$BASE/releases" | tr '\n' ' ')"
    systemctl list-timers mikrotik-panel-update.timer --no-pager 2>/dev/null | sed -n 2p ;;
  *) sed -n 2,12p "$0"; exit 2 ;;
esac
