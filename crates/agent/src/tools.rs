//! Katalog alat agen. Skema dikirim ke model; validasi sebenarnya dilakukan
//! oleh modul terkait (design, css, routeros_cmd) sebelum apa pun dijalankan.
use panel_core::policy::Permission;
use serde::Serialize;
use serde_json::{json, Value as Json};

#[derive(Clone, Debug, Serialize)]
pub struct ToolDef {
    pub name: &'static str,
    pub group: Group,
    pub description: &'static str,
    /// Izin dasar. Alat perintah RouterOS memakai izin dari perintahnya sendiri.
    pub permission: Permission,
    pub input_schema: Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Design,
    RouterRead,
    RouterOsKnowledge,
    RouterWrite,
    Analysis,
    Memory,
    Conversation,
    Code,
}

fn obj(props: Json, required: &[&str]) -> Json {
    json!({"type": "object", "properties": props, "required": required, "additionalProperties": false})
}

fn s(desc: &str) -> Json {
    json!({"type": "string", "description": desc})
}

fn int(min: i64, max: i64, desc: &str) -> Json {
    json!({"type": "integer", "minimum": min, "maximum": max, "description": desc})
}

const BLOCK_DOC: &str = "Blok: {id, span, kind, props}. kind: metrics{items:[customers_online|customers_total|low_quota|cpu|memory|uptime|traffic_today|temperature]}, traffic_chart{days:7|14|30, account?}, customer_table{title, filter:all|online|offline|low_quota, columns:[name|status|address|profile|used|remaining|limit|uptime|actions], row_limit}, low_quota{limit}, router_status{}, topology{}, logs{topics, limit}, recommendations{limit}, action_button{action_id, label?}, text{markdown}, builtin{name:customer_detail|audit|agent|agent_memory|ai_settings|actions}.";

fn command_schema() -> Json {
    json!({"type": "object", "description": "Perintah RouterOS terstruktur. Contoh: {\"path\":[\"ppp\",\"active\"],\"verb\":\"remove\",\"find\":{\"name\":\"budi\"}}. Placeholder tombol: {\"param\":\"nama\"} atau {\"param\":\"gib\",\"multiply\":1073741824}.",
        "properties": {
            "path": {"type": "array", "items": {"type": "string"}},
            "verb": {"type": "string"},
            "find": {"type": "object"},
            "args": {"type": "object"}
        }, "required": ["path", "verb"], "additionalProperties": false})
}

fn params_schema() -> Json {
    json!({"type": "array", "maxItems": 8, "items": {"type": "object",
        "description": "{name, label, type: text{max_len}|integer{min,max}|choice{values}|customer|profile}",
        "properties": {"name": {"type": "string"}, "label": {"type": "string"},
            "type": {"enum": ["text", "integer", "choice", "customer", "profile"]},
            "max_len": {"type": "integer"}, "min": {"type": "integer"}, "max": {"type": "integer"},
            "values": {"type": "array", "items": {"type": "string"}}},
        "required": ["name", "label", "type"]}})
}

pub fn catalog() -> Vec<ToolDef> {
    use Group::*;
    use Permission::*;
    let t = |name, group, permission, description, input_schema| ToolDef { name, group, description, permission, input_schema };
    vec![
        // A. Desain dan layout (semua masuk draft, selalu bisa di-revert)
        t("get_design", Design, Free, "Baca design spec draft saat ini (tokens, nav, pages, custom_css).", obj(json!({}), &[])),
        t("set_tokens", Design, Free, "Ubah token desain dengan merge patch: light/dark palet (hex), font (system|inter|ibm_plex_sans|source_sans|serif|mono), font_size 12-20, radius 0-24, density 70-150, shadow none|soft|strong. Kontras teks dicek otomatis.", obj(json!({"changes": {"type": "object"}}), &["changes"])),
        t("set_theme_mode", Design, Free, "Pilih mode tema.", obj(json!({"mode": {"enum": ["light", "dark", "system"]}}), &["mode"])),
        t("add_block", Design, Free, BLOCK_DOC, obj(json!({"page": s("ID halaman"), "index": int(0, 100, "posisi, kosong = paling bawah"), "block": {"type": "object"}}), &["page", "block"])),
        t("update_block", Design, Free, "Ubah lebar (span) dan/atau props blok dengan merge patch.", obj(json!({"id": s("ID blok"), "span": int(1, 4, "lebar kolom"), "props": {"type": "object"}}), &["id"])),
        t("duplicate_block", Design, Free, "Gandakan blok tepat di bawahnya.", obj(json!({"id": s("ID blok"), "new_id": s("ID baru")}), &["id", "new_id"])),
        t("move_block", Design, Free, "Pindahkan blok ke halaman/posisi lain.", obj(json!({"id": s("ID blok"), "page": s("ID halaman tujuan"), "index": int(0, 100, "posisi")}), &["id", "page"])),
        t("remove_block", Design, Free, "Hapus blok (blok Agen AI dan Pengaturan AI tidak bisa dihapus).", obj(json!({"id": s("ID blok")}), &["id"])),
        t("set_page_layout", Design, Free, "Ubah judul dan jumlah kolom grid halaman.", obj(json!({"page": s("ID halaman"), "title": s("judul"), "columns": int(1, 4, "kolom")}), &["page"])),
        t("add_page", Design, Free, "Tambah halaman baru berisi blok, opsional langsung masuk menu.", obj(json!({"page": {"type": "object", "description": "{id, title, columns, blocks:[...]}"}, "nav_label": s("label menu")}), &["page"])),
        t("remove_page", Design, Free, "Hapus halaman beserta item menunya.", obj(json!({"page": s("ID halaman")}), &["page"])),
        t("set_nav", Design, Free, "Atur menu: position left|top|compact, items [{page, label, icon (maks 2 karakter)}].", obj(json!({"nav": {"type": "object"}}), &["nav"])),
        t("set_custom_css", Design, Free, "Ganti CSS khusus (maks 8 KB). Disanitasi: tanpa url(), @import, position:fixed; semua selektor diberi awalan .panel-root. Pakai var(--accent) dst. untuk token.", obj(json!({"css": s("CSS")}), &["css"])),
        t("preview_viewport", Design, Free, "Minta panel menampilkan draft dalam ukuran tertentu.", obj(json!({"size": {"enum": ["desktop", "tablet", "phone"]}}), &["size"])),
        t("revert_design", Design, Free, "Kembalikan draft ke versi desain tertentu.", obj(json!({"version_id": s("ID versi")}), &["version_id"])),
        t("request_publish", Design, Approve, "Minta admin menjadikan draft sebagai desain aktif.", obj(json!({"summary": s("ringkasan perubahan")}), &["summary"])),
        // B. Membaca router
        t("router_snapshot", RouterRead, Free, "Ringkasan terbaru router: model, versi RouterOS, CPU, RAM, uptime, suhu, paket.", obj(json!({}), &[])),
        t("router_query", RouterRead, Free, "Jalankan perintah baca (print/get/export tanpa file) di menu yang diizinkan.", obj(json!({"command": command_schema()}), &["command"])),
        t("get_topology", RouterRead, Free, "Graf topologi dari neighbor, bridge, ARP, dan interface.", obj(json!({}), &[])),
        t("get_traffic", RouterRead, Free, "Trafik interface (sekali monitor-traffic) atau riwayat pelanggan dari database.", obj(json!({"interface": s("nama interface"), "account": s("nama pelanggan"), "days": int(1, 30, "hari riwayat")}), &[])),
        t("get_logs", RouterRead, Free, "Log router dengan filter topik dan teks.", obj(json!({"topics": {"type": "array", "items": {"type": "string"}}, "contains": s("teks"), "limit": int(1, 200, "baris")}), &[])),
        t("list_customers", RouterRead, Free, "Daftar pelanggan dengan status, profil, dan kuota.", obj(json!({"filter": {"enum": ["all", "online", "offline", "low_quota"]}, "limit": int(1, 500, "baris")}), &[])),
        t("customer_detail", RouterRead, Free, "Detail satu pelanggan: profil, kuota, sesi aktif, riwayat.", obj(json!({"account": s("nama pelanggan")}), &["account"])),
        t("snapshot_diff", RouterRead, Free, "Apa yang berubah di konfigurasi router sejak waktu tertentu.", obj(json!({"since_hours": int(1, 720, "jam ke belakang")}), &["since_hours"])),
        t("net_diagnose", RouterRead, Free, "Ping (count 1-10) atau traceroute (count 1-3) dari router.", obj(json!({"kind": {"enum": ["ping", "traceroute"]}, "address": s("IP/host"), "count": int(1, 10, "jumlah")}), &["kind", "address"])),
        // C. Pengetahuan RouterOS
        t("get_command_tree", RouterOsKnowledge, Free, "Menu, perintah, dan parameter yang ada di versi RouterOS router ini.", obj(json!({"path": {"type": "array", "items": {"type": "string"}}}), &["path"])),
        t("search_routeros_docs", RouterOsKnowledge, Free, "Cari referensi RouterOS v7 lokal.", obj(json!({"query": s("kata kunci")}), &["query"])),
        t("check_command", RouterOsKnowledge, Free, "Validasi perintah: blocklist, pohon perintah router, dan :parse. Tidak menjalankan apa pun.", obj(json!({"command": command_schema()}), &["command"])),
        // D. Mengubah router (selalu lewat kartu persetujuan)
        t("topup_quota", RouterWrite, Approve, "Tambah kuota pelanggan (GiB).", obj(json!({"account": s("pelanggan"), "gib": int(1, 1000, "tambahan GiB")}), &["account", "gib"])),
        t("disconnect_session", RouterWrite, Approve, "Putus/restart sesi PPPoE atau hotspot pelanggan.", obj(json!({"account": s("pelanggan")}), &["account"])),
        t("set_customer_profile", RouterWrite, Approve, "Ganti profil/paket pelanggan.", obj(json!({"account": s("pelanggan"), "profile": s("profil")}), &["account", "profile"])),
        t("set_customer_enabled", RouterWrite, Approve, "Aktifkan atau nonaktifkan pelanggan.", obj(json!({"account": s("pelanggan"), "enabled": {"type": "boolean"}}), &["account", "enabled"])),
        t("set_radius_user", RouterWrite, Approve, "Atur atribut User Manager/RADIUS untuk user tertentu.", obj(json!({"account": s("pelanggan"), "attributes": {"type": "object"}}), &["account", "attributes"])),
        t("propose_command", RouterWrite, Approve, "Usulkan satu perintah RouterOS sekali jalan. Izin mengikuti risiko perintah (Kritis untuk firewall, alamat, rute, interface, system).", obj(json!({"command": command_schema(), "reason": s("alasan")}), &["command", "reason"])),
        t("create_action_button", RouterWrite, Approve, "Simpan tombol aksi custom: template perintah + parameter, opsional langsung ditempatkan di halaman.", obj(json!({"id": s("ID tombol"), "label": s("label"), "description": s("penjelasan"), "command": command_schema(), "params": params_schema(), "place": {"type": "object", "description": "{page, index?}"}}), &["id", "label", "command", "params"])),
        t("update_action_button", RouterWrite, Approve, "Ubah tombol aksi.", obj(json!({"id": s("ID tombol"), "label": s("label"), "description": s("penjelasan"), "command": command_schema(), "params": params_schema()}), &["id"])),
        t("delete_action_button", RouterWrite, Approve, "Hapus tombol aksi.", obj(json!({"id": s("ID tombol")}), &["id"])),
        t("run_action_button", RouterWrite, Approve, "Jalankan tombol aksi tersimpan dengan nilai parameter. Izin mengikuti label tombol.", obj(json!({"id": s("ID tombol"), "values": {"type": "object"}}), &["id", "values"])),
        t("revert_action_run", RouterWrite, Approve, "Kembalikan nilai lama dari eksekusi sebelumnya.", obj(json!({"run_id": s("ID eksekusi")}), &["run_id"])),
        t("reboot_router", RouterWrite, Critical, "Reboot router.", obj(json!({"reason": s("alasan")}), &["reason"])),
        // E. Analisis dan saran
        t("audit_router", Analysis, Free, "Periksa performa, keamanan, topologi, dan kecocokan layout; hasilkan temuan.", obj(json!({"focus": {"type": "array", "items": {"enum": ["performance", "security", "topology", "layout"]}}}), &[])),
        t("create_recommendation", Analysis, Free, "Buat kartu saran: temuan, bukti, dampak, dan tombol/perintah perbaikan opsional.", obj(json!({"kind": {"enum": ["performance", "security", "topology", "layout"]}, "title": s("judul"), "finding": s("temuan"), "evidence": s("bukti dari data router"), "impact": s("dampak"), "fix": {"type": "object", "description": "{command} atau {design_ops:[...]}"}}), &["kind", "title", "finding", "evidence"])),
        t("traffic_report", Analysis, Free, "Ringkasan pemakaian: total, pelanggan terbesar, jam sibuk.", obj(json!({"days": int(1, 30, "hari")}), &["days"])),
        // F. Memori
        t("remember", Memory, Free, "Simpan fakta/preferensi baru. Selalu sebutkan di jawaban bahwa memori disimpan.", obj(json!({"kind": {"enum": ["user", "router", "outcome"]}, "text": s("isi memori, maks 500 karakter")}), &["kind", "text"])),
        t("recall", Memory, Free, "Cari memori yang relevan.", obj(json!({"query": s("kata kunci")}), &["query"])),
        t("update_memory", Memory, Free, "Ubah isi memori.", obj(json!({"id": s("ID memori"), "text": s("isi baru")}), &["id", "text"])),
        t("forget", Memory, Free, "Hapus memori.", obj(json!({"id": s("ID memori")}), &["id"])),
        // G. Percakapan
        t("ask_user", Conversation, Free, "Tanyakan pilihan ganda ke admin di chat; giliran berhenti sampai admin menjawab.", obj(json!({"question": s("pertanyaan"), "options": {"type": "array", "minItems": 2, "maxItems": 5, "items": {"type": "string"}}}), &["question", "options"])),
        t("show_card", Conversation, Free, "Tampilkan tabel, grafik, atau diagram topologi di chat.", obj(json!({"kind": {"enum": ["table", "chart", "topology"]}, "title": s("judul"), "data": {"type": "object"}}), &["kind", "data"])),
        t("set_conversation_title", Conversation, Free, "Beri judul percakapan.", obj(json!({"title": s("judul, maks 60 karakter")}), &["title"])),
        // H. Kode panel (lewat Git + GitHub Actions; path core-locked tidak bisa diubah)
        t("list_files", Code, Free, "Daftar file kode sumber panel.", obj(json!({"dir": s("folder relatif")}), &[])),
        t("read_file", Code, Free, "Baca file kode sumber.", obj(json!({"path": s("path relatif"), "start_line": int(1, 100000, "baris awal"), "end_line": int(1, 100000, "baris akhir")}), &["path"])),
        t("search_code", Code, Free, "Cari teks di kode sumber.", obj(json!({"query": s("teks/regex")}), &["query"])),
        t("edit_file", Code, Free, "Ganti potongan teks di file (harus muncul tepat sekali). Hanya di salinan kerja.", obj(json!({"path": s("path"), "old": s("teks lama"), "new": s("teks baru")}), &["path", "old", "new"])),
        t("create_file", Code, Free, "Buat file baru di salinan kerja.", obj(json!({"path": s("path"), "content": s("isi")}), &["path", "content"])),
        t("delete_file", Code, Free, "Hapus file di salinan kerja.", obj(json!({"path": s("path")}), &["path"])),
        t("show_diff", Code, Free, "Tampilkan diff salinan kerja terhadap versi terpasang.", obj(json!({}), &[])),
        t("build_and_test", Code, Free, "Commit ke branch agen, push, dan jalankan GitHub Actions (cargo test + build). Menunggu hasil.", obj(json!({"message": s("pesan commit")}), &["message"])),
        t("deploy_code", Code, Approve, "Pasang build yang lulus test: unduh binary, cek checksum, uji di port percobaan, swap, rollback otomatis jika gagal.", obj(json!({"build_id": s("ID build Actions")}), &["build_id"])),
        t("rollback_code", Code, Free, "Kembali ke build terpasang sebelumnya.", obj(json!({"release_id": s("ID rilis, kosong = sebelumnya")}), &[])),
    ]
}

pub fn find(name: &str) -> Option<ToolDef> {
    catalog().into_iter().find(|t| t.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn names_unique_and_schemas_are_objects() {
        let tools = catalog();
        let names: BTreeSet<_> = tools.iter().map(|t| t.name).collect();
        assert_eq!(names.len(), tools.len());
        for tool in &tools {
            assert_eq!(tool.input_schema["type"], "object", "{}", tool.name);
            assert!(tool.name.len() <= 64 && tool.name.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        }
        assert!(tools.len() >= 55, "{}", tools.len());
    }

    #[test]
    fn router_writes_never_free() {
        for tool in catalog().iter().filter(|t| t.group == Group::RouterWrite) {
            assert_ne!(tool.permission, Permission::Free, "{}", tool.name);
        }
    }
}
