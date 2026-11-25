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

## Status tahap 2 (telemetri, kuota, agen)

- Poller tiap `poll_seconds`: pelanggan User Manager, sesi PPPoE aktif, resource, interface.
  Sesi di-stream (tanpa batas 8 MB); trafik dihitung per sesi dari counter terakhir, jadi jeda
  sinkronisasi dan tengah malam tidak menghilangkan trafik. Reset kuota tanggal 29–31 didukung.
- Tambah kuota: `POST /api/customers/{akun}/quota/preview` lalu `/apply`; dicek ulang di router dan dicatat di audit.
- API: `/api/dashboard`, `/api/customers[/{akun}[/logs]]`, `/api/router`, `/api/router/refresh`, `/api/audit`,
  `/api/settings/ai`, serta endpoint desain dan agen (lihat `web/src/agent/CONTRACT.md`).
- API-SSL: `mikrotik-panel router-fingerprint`, lalu isi `tls = true` dan `tls_fingerprint` di config.
  Router harus punya sertifikat di `/ip service set api-ssl certificate=...`.
- Pindah dari panel lama: `mikrotik-panel migrate --from panel.sqlite3 --old-config private.json`.

## Status tahap 3 (tampilan web)

- `web/`: Preact + signals, ~25 KB JS gzip, tertanam di binary (tidak perlu Node di server).
- Halaman Ringkasan, Pelanggan (+ detail, tambah kuota dengan pratinjau), Audit, Pengaturan AI, Memori agen.
- Panel chat agen di samping (Enter kirim, Shift+Enter baris baru); tampilan berubah langsung lewat SSE tanpa reload.
- Darurat: buka `/?design=default` bila desain buatan agen membuat panel tak terpakai.
- RSS terukur ~5,7 MB (binary rilis x86_64, router tiruan 16 pelanggan).

## Pasang di VPS (Ubuntu/Debian)

```sh
curl -fsSL https://raw.githubusercontent.com/nirvagold/mikrotik-panel/main/deploy/install.sh | sudo bash
```

Pemasang mengunduh rilis terbaru dari GitHub (hash SHA256 dicek), menanyakan port, IP/user/kata sandi
router, dan kata sandi admin, lalu memasang layanan `mikrotik-panel` dan pembaru otomatis.
Port bawaan 8780 supaya bisa berjalan berdampingan dengan panel lama (8765–8767).
Menjalankan ulang aman: config yang sudah ada tidak diubah.

## Update otomatis

`mikrotik-panel-update.timer` mengecek rilis GitHub tiap jam. Bila ada tag baru:
unduh + cek hash → hentikan panel → cadangkan database → ganti symlink `/opt/mikrotik-panel/current`
→ nyalakan → tunggu `/api/health` melaporkan versi baru (maks. 45 detik). Gagal = database dan rilis
lama dikembalikan otomatis dan rilis itu tidak dicoba lagi oleh timer. Disimpan 3 rilis + 3 cadangan.
Setelah update, admin perlu login ulang (sesi disimpan di memori).

```sh
sudo mikrotik-panel-update --status          # versi terpasang, hasil update terakhir, jadwal berikutnya
sudo mikrotik-panel-update --check           # cek rilis terbaru tanpa memasang
sudo mikrotik-panel-update                   # update sekarang
sudo mikrotik-panel-update --rollback        # kembali ke rilis sebelumnya (beserta database)
sudo mikrotik-panel-update --version v0.1.0  # pasang versi tertentu
sudo systemctl disable --now mikrotik-panel-update.timer   # matikan update otomatis
```

Membuat rilis: `git tag v0.1.1 && git push origin v0.1.1`. GitHub Actions membangun binary dengan
versi dari tag dan mengunggah binary, `SHA256SUMS`, skrip, dan unit systemd ke halaman rilis.

## Build

```sh
(cd web && npm ci && npm run build)   # dulu, hasilnya ditanam ke binary
cargo test
cargo build --release                                          # mesin ini
cargo zigbuild --release --target x86_64-unknown-linux-musl    # VPS x86_64, binary statis
cargo zigbuild --release --target aarch64-unknown-linux-musl   # ARM 64-bit (STB/RPi 4)
```
