# Panel MikroTik (Rust)

Satu binary Rust (axum + tokio + SQLite) pengganti panel Flask + Next.js lama.
Rencana lengkap: https://claude.ai/artifact/2nXcHyXLDPFcwf8ZD75tFH

## Struktur

- `crates/core-locked/` — konfigurasi, kriptografi, autentikasi, sesi/CSRF. **Dikunci dari agen AI.**
- `crates/app/` — server HTTP, kanal SSE, klien RouterOS API, database. Boleh diedit agen.
- `deploy/` — contoh config dan unit systemd.

## Status tahap 1 (fondasi)

- Config TOML tervalidasi, rahasia terenkripsi XChaCha20-Poly1305, password admin Argon2.
- SQLite WAL + migrasi berversi (semua tabel rencana, termasuk design_versions, chat, FTS5 memori agen).
- Login dengan cookie HttpOnly/SameSite=Strict/Secure, token CSRF (header `x-csrf-token`),
  batas 5 gagal per IP / 15 menit memakai IP asli dari proxy tepercaya.
- `GET /api/health` (DB + router), `GET /api/events` (SSE), `--port` untuk instance percobaan.
- Klien RouterOS API (8728): koneksi persisten, sambung ulang otomatis, baris di-stream tanpa batas 8 MB,
  parameter dikirim sebagai kata terpisah sehingga tidak bisa disisipi perintah.

## Pasang

```sh
install -Dm755 mikrotik-panel /opt/mikrotik-panel/releases/0.1.0/mikrotik-panel
ln -sfn /opt/mikrotik-panel/releases/0.1.0 /opt/mikrotik-panel/current
install -Dm600 deploy/config.example.toml /etc/mikrotik-panel/config.toml
/opt/mikrotik-panel/current/mikrotik-panel gen-key          # isi secret_key
/opt/mikrotik-panel/current/mikrotik-panel hash-password    # isi admin_password_hash
/opt/mikrotik-panel/current/mikrotik-panel encrypt --config /etc/mikrotik-panel/config.toml  # isi router.password_encrypted
install -m644 deploy/mikrotik-panel.service /etc/systemd/system/
systemctl daemon-reload && systemctl enable --now mikrotik-panel
curl -s http://127.0.0.1:8765/api/health
```

## Build

```sh
cargo test
cargo build --release                                          # mesin ini
cargo zigbuild --release --target x86_64-unknown-linux-musl    # VPS x86_64, binary statis
cargo zigbuild --release --target aarch64-unknown-linux-musl   # ARM 64-bit (STB/RPi 4)
```
