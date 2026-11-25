//! Bagian inti yang DIKUNCI dari agen AI: konfigurasi, kriptografi, autentikasi,
//! sesi/CSRF, dan (nanti) kebijakan izin alat agen, blocklist RouterOS, serta rollback.
//! Agen tidak boleh mengubah isi crate ini.

pub mod auth;
pub mod config;
pub mod crypto;
pub mod policy;
