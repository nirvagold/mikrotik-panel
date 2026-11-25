# Uji end-to-end agen (router tiruan + AI tiruan)

1. Build: `(cd web && npm ci && npm run build)` lalu `cargo build -p mikrotik-panel --features test-endpoint`
   (tambahkan di crates/app/Cargo.toml: `[features] test-endpoint = ["panel-agent/test-endpoint"]`; fitur ini mengizinkan endpoint AI http://127.0.0.1, jangan dipakai di rilis).
2. `python3 fake_router.py 18728` (RouterOS API tiruan, 3 pelanggan) dan `python3 fake_ai.py 18900` (OpenAI-compatible, skrip: ubah desain / "putus" = usulan perintah / "lambat" = jawaban panjang untuk uji Stop).
3. Config: router 127.0.0.1:18728, port 8799, cookie_secure=false. Login, simpan Pengaturan AI (https placeholder), lalu ubah url di tabel settings ke `http://127.0.0.1:18900/v1`.
4. `node e2e.cjs` (password admin di skrip: rahasia-admin-123). Memeriksa: Shift+Enter, desain berubah live tanpa reload, /undo, /redo, kartu persetujuan + eksekusi di router, Stop menyimpan teks parsial, riwayat setelah reload, lebar HP.
