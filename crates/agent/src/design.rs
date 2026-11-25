//! Design spec: satu dokumen JSON yang menjadi sumber tampilan panel.
//! Agen mengubahnya lewat `DesignOp`; setiap op divalidasi sebelum disimpan
//! sebagai versi baru dan disiarkan ke browser.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesignError(pub String);
impl std::fmt::Display for DesignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for DesignError {}
fn err<T>(msg: impl Into<String>) -> Result<T, DesignError> {
    Err(DesignError(msg.into()))
}

pub const MAX_PAGES: usize = 20;
pub const MAX_BLOCKS_PER_PAGE: usize = 40;
pub const MAX_CUSTOM_CSS: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignSpec {
    pub tokens: Tokens,
    pub nav: Nav,
    pub pages: Vec<Page>,
    /// CSS bebas terbatas, sudah disanitasi (lihat `css`).
    #[serde(default)]
    pub custom_css: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Light,
    Dark,
    System,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Font {
    System,
    Inter,
    IbmPlexSans,
    SourceSans,
    Serif,
    Mono,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shadow {
    None,
    Soft,
    Strong,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Palette {
    pub bg: String,
    pub surface: String,
    pub surface_alt: String,
    pub text: String,
    pub muted: String,
    pub border: String,
    pub accent: String,
    pub on_accent: String,
    pub ok: String,
    pub warn: String,
    pub danger: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tokens {
    pub mode: ThemeMode,
    pub light: Palette,
    pub dark: Palette,
    pub font: Font,
    pub font_mono: Font,
    /// Ukuran teks dasar dalam px (12–20).
    pub font_size: u8,
    /// Radius sudut dalam px (0–24).
    pub radius: u8,
    /// Pengali jarak (persen, 70–150).
    pub density: u8,
    pub shadow: Shadow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavPosition {
    Left,
    Top,
    Compact,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavItem {
    pub page: String,
    pub label: String,
    #[serde(default)]
    pub icon: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Nav {
    pub position: NavPosition,
    pub items: Vec<NavItem>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub id: String,
    pub title: String,
    /// Jumlah kolom grid (1–4).
    pub columns: u8,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    pub id: String,
    /// Lebar dalam kolom (1–columns).
    pub span: u8,
    #[serde(flatten)]
    pub kind: BlockKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    CustomersOnline,
    CustomersTotal,
    LowQuota,
    Cpu,
    Memory,
    Uptime,
    TrafficToday,
    Temperature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomerFilter {
    All,
    Online,
    Offline,
    LowQuota,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomerColumn {
    Name,
    Status,
    Address,
    Profile,
    Used,
    Remaining,
    Limit,
    Uptime,
    Actions,
}

/// Katalog blok. Setiap varian punya props bertipe; field asing ditolak.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "props", rename_all = "snake_case", deny_unknown_fields)]
pub enum BlockKind {
    Metrics { items: Vec<Metric> },
    TrafficChart { days: u8, #[serde(default)] account: Option<String> },
    CustomerTable {
        title: String,
        filter: CustomerFilter,
        columns: Vec<CustomerColumn>,
        row_limit: u16,
    },
    LowQuota { limit: u8 },
    RouterStatus {},
    Topology {},
    Logs { #[serde(default)] topics: Vec<String>, limit: u8 },
    Recommendations { limit: u8 },
    ActionButton { action_id: String, #[serde(default)] label: Option<String> },
    Text { markdown: String },
    /// Halaman bawaan yang dirender aplikasi (detail pelanggan, riwayat, agen, pengaturan).
    Builtin { name: BuiltinView },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinView {
    CustomerDetail,
    Audit,
    Agent,
    AgentMemory,
    AiSettings,
    Actions,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn valid_label(text: &str, max: usize) -> bool {
    let n = text.trim().chars().count();
    (1..=max).contains(&n) && !text.chars().any(char::is_control)
}

/// Warna hanya hex (#rgb, #rrggbb) supaya kontras bisa dihitung.
pub fn parse_hex(color: &str) -> Option<[f64; 3]> {
    let hex = color.strip_prefix('#')?;
    let expand: String = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 => hex.to_string(),
        _ => return None,
    };
    let byte = |i: usize| u8::from_str_radix(&expand[i..i + 2], 16).ok().map(|v| v as f64 / 255.0);
    Some([byte(0)?, byte(2)?, byte(4)?])
}

fn luminance([r, g, b]: [f64; 3]) -> f64 {
    let lin = |c: f64| if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// Rasio kontras WCAG.
pub fn contrast(a: &str, b: &str) -> Option<f64> {
    let (la, lb) = (luminance(parse_hex(a)?), luminance(parse_hex(b)?));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    Some((hi + 0.05) / (lo + 0.05))
}

impl Palette {
    fn validate(&self, name: &str) -> Result<(), DesignError> {
        let fields = [
            ("bg", &self.bg), ("surface", &self.surface), ("surface_alt", &self.surface_alt),
            ("text", &self.text), ("muted", &self.muted), ("border", &self.border),
            ("accent", &self.accent), ("on_accent", &self.on_accent), ("ok", &self.ok),
            ("warn", &self.warn), ("danger", &self.danger),
        ];
        for (field, value) in fields {
            if parse_hex(value).is_none() {
                return err(format!("Warna {name}.{field} harus hex seperti #1a2b3c."));
            }
        }
        let checks = [
            ("text", &self.text, "bg", &self.bg, 4.5),
            ("text", &self.text, "surface", &self.surface, 4.5),
            ("text", &self.text, "surface_alt", &self.surface_alt, 4.5),
            ("muted", &self.muted, "surface", &self.surface, 3.0),
            ("on_accent", &self.on_accent, "accent", &self.accent, 4.5),
        ];
        for (fa, a, fb, b, min) in checks {
            let ratio = contrast(a, b).unwrap_or(0.0);
            if ratio < min {
                return err(format!(
                    "Kontras {name}.{fa} di atas {name}.{fb} hanya {ratio:.1}:1, minimal {min}:1 agar tetap terbaca."
                ));
            }
        }
        Ok(())
    }
}

impl Tokens {
    fn validate(&self) -> Result<(), DesignError> {
        self.light.validate("light")?;
        self.dark.validate("dark")?;
        if !(12..=20).contains(&self.font_size) {
            return err("Ukuran teks dasar harus 12 sampai 20 px.");
        }
        if self.radius > 24 {
            return err("Radius maksimal 24 px.");
        }
        if !(70..=150).contains(&self.density) {
            return err("Kepadatan harus 70 sampai 150 persen.");
        }
        Ok(())
    }
}

impl Block {
    fn validate(&self, columns: u8) -> Result<(), DesignError> {
        if !valid_id(&self.id) {
            return err(format!("ID blok `{}` tidak valid (huruf kecil, angka, tanda minus).", self.id));
        }
        if !(1..=columns).contains(&self.span) {
            return err(format!("Lebar blok `{}` harus 1 sampai {columns} kolom.", self.id));
        }
        match &self.kind {
            BlockKind::Metrics { items } => {
                let unique: BTreeSet<_> = items.iter().collect();
                if items.is_empty() || items.len() > 8 || unique.len() != items.len() {
                    return err("Blok metrik butuh 1 sampai 8 metrik berbeda.");
                }
            }
            BlockKind::TrafficChart { days, account } => {
                if ![7, 14, 30].contains(days) {
                    return err("Grafik trafik hanya 7, 14, atau 30 hari.");
                }
                if account.as_deref().is_some_and(|a| !valid_label(a, 64)) {
                    return err("Nama akun grafik tidak valid.");
                }
            }
            BlockKind::CustomerTable { title, columns, row_limit, .. } => {
                let unique: BTreeSet<_> = columns.iter().collect();
                if !valid_label(title, 80) || columns.is_empty() || unique.len() != columns.len()
                    || !(1..=200).contains(row_limit)
                {
                    return err("Tabel pelanggan butuh judul 1–80 karakter, kolom unik, dan 1–200 baris.");
                }
            }
            BlockKind::LowQuota { limit } | BlockKind::Recommendations { limit } => {
                if !(1..=50).contains(limit) {
                    return err("Batas daftar harus 1 sampai 50.");
                }
            }
            BlockKind::Logs { topics, limit } => {
                if !(1..=100).contains(limit) || topics.len() > 10
                    || topics.iter().any(|t| !valid_id(t))
                {
                    return err("Blok log: maksimal 10 topik dan 1–100 baris.");
                }
            }
            BlockKind::ActionButton { action_id, label } => {
                if !valid_id(action_id) || label.as_deref().is_some_and(|l| !valid_label(l, 40)) {
                    return err("Tombol aksi butuh action_id valid dan label maksimal 40 karakter.");
                }
            }
            BlockKind::Text { markdown } => {
                if markdown.chars().count() > 2000 {
                    return err("Teks maksimal 2000 karakter.");
                }
            }
            BlockKind::RouterStatus {} | BlockKind::Topology {} | BlockKind::Builtin { .. } => {}
        }
        Ok(())
    }
}

impl DesignSpec {
    pub fn validate(&self) -> Result<(), DesignError> {
        self.tokens.validate()?;
        if self.pages.is_empty() || self.pages.len() > MAX_PAGES {
            return err(format!("Jumlah halaman harus 1 sampai {MAX_PAGES}."));
        }
        let mut page_ids = BTreeSet::new();
        let mut block_ids = BTreeSet::new();
        for page in &self.pages {
            if !valid_id(&page.id) || !page_ids.insert(page.id.as_str()) {
                return err(format!("ID halaman `{}` tidak valid atau ganda.", page.id));
            }
            if !valid_label(&page.title, 60) {
                return err("Judul halaman harus 1 sampai 60 karakter.");
            }
            if !(1..=4).contains(&page.columns) {
                return err("Kolom halaman harus 1 sampai 4.");
            }
            if page.blocks.len() > MAX_BLOCKS_PER_PAGE {
                return err(format!("Maksimal {MAX_BLOCKS_PER_PAGE} blok per halaman."));
            }
            for block in &page.blocks {
                block.validate(page.columns)?;
                if !block_ids.insert(block.id.as_str()) {
                    return err(format!("ID blok `{}` dipakai lebih dari sekali.", block.id));
                }
            }
        }
        // Halaman agen dan pengaturan AI harus selalu bisa dicapai, apa pun desainnya.
        for required in [BuiltinView::Agent, BuiltinView::AiSettings] {
            if !self.pages.iter().flat_map(|p| &p.blocks)
                .any(|b| b.kind == BlockKind::Builtin { name: required })
            {
                return err("Halaman Agen AI dan Pengaturan AI tidak boleh dihapus.");
            }
        }
        if self.nav.items.is_empty() || self.nav.items.len() > MAX_PAGES {
            return err("Menu harus berisi 1 sampai 20 item.");
        }
        let mut nav_pages = BTreeSet::new();
        for item in &self.nav.items {
            if !page_ids.contains(item.page.as_str()) || !nav_pages.insert(item.page.as_str()) {
                return err(format!("Menu menunjuk halaman `{}` yang tidak ada atau ganda.", item.page));
            }
            if !valid_label(&item.label, 30) || item.icon.chars().count() > 2 {
                return err("Label menu 1–30 karakter, ikon maksimal 2 karakter.");
            }
        }
        if self.custom_css.len() > MAX_CUSTOM_CSS {
            return err("CSS khusus maksimal 8 KB.");
        }
        Ok(())
    }

    fn page_mut(&mut self, id: &str) -> Result<&mut Page, DesignError> {
        self.pages.iter_mut().find(|p| p.id == id)
            .ok_or_else(|| DesignError(format!("Halaman `{id}` tidak ada.")))
    }

    fn find_block(&self, id: &str) -> Result<(usize, usize), DesignError> {
        self.pages.iter().enumerate()
            .find_map(|(pi, p)| p.blocks.iter().position(|b| b.id == id).map(|bi| (pi, bi)))
            .ok_or_else(|| DesignError(format!("Blok `{id}` tidak ada.")))
    }

    /// Terapkan satu operasi; hasilnya selalu tervalidasi utuh.
    pub fn apply(&self, op: &DesignOp) -> Result<DesignSpec, DesignError> {
        let mut next = self.clone().normalized();
        match op {
            DesignOp::SetTokens { changes } => {
                if changes.get("light").is_some() {
                    return err("Panel selalu bertema gelap. Ubah palet `dark`, bukan `light`.");
                }
                if changes.get("mode").is_some_and(|m| m != "dark") {
                    return err("Panel selalu bertema gelap; mode tidak bisa diganti.");
                }
                let mut tokens = serde_json::to_value(&next.tokens).expect("tokens serialize");
                merge_patch(&mut tokens, changes);
                next.tokens = serde_json::from_value(tokens)
                    .map_err(|e| DesignError(format!("Perubahan token tidak valid: {e}")))?;
            }
            DesignOp::SetThemeMode { mode } => {
                if *mode != ThemeMode::Dark {
                    return err("Panel selalu bertema gelap; mode terang dan ikut sistem tidak tersedia.");
                }
            }
            DesignOp::AddBlock { page, index, block } => {
                let page = next.page_mut(page)?;
                let at = index.unwrap_or(page.blocks.len()).min(page.blocks.len());
                page.blocks.insert(at, block.clone());
            }
            DesignOp::UpdateBlock { id, span, props } => {
                let (pi, bi) = next.find_block(id)?;
                let block = &mut next.pages[pi].blocks[bi];
                if let Some(span) = span {
                    block.span = *span;
                }
                if let Some(props) = props {
                    let mut value = serde_json::to_value(&block.kind).expect("block serialize");
                    let current = value.get_mut("props").expect("adjacent tag");
                    if current.is_null() {
                        *current = Json::Object(Default::default());
                    }
                    merge_patch(current, props);
                    block.kind = serde_json::from_value(value)
                        .map_err(|e| DesignError(format!("Props blok `{id}` tidak valid: {e}")))?;
                }
            }
            DesignOp::DuplicateBlock { id, new_id } => {
                let (pi, bi) = next.find_block(id)?;
                let mut copy = next.pages[pi].blocks[bi].clone();
                copy.id = new_id.clone();
                next.pages[pi].blocks.insert(bi + 1, copy);
            }
            DesignOp::MoveBlock { id, page, index } => {
                let (pi, bi) = next.find_block(id)?;
                let mut block = next.pages[pi].blocks.remove(bi);
                let target = next.page_mut(page)?;
                block.span = block.span.min(target.columns);
                let at = index.unwrap_or(target.blocks.len()).min(target.blocks.len());
                target.blocks.insert(at, block);
            }
            DesignOp::RemoveBlock { id } => {
                let (pi, bi) = next.find_block(id)?;
                next.pages[pi].blocks.remove(bi);
            }
            DesignOp::SetPageLayout { page, title, columns } => {
                let page = next.page_mut(page)?;
                if let Some(title) = title {
                    page.title = title.trim().to_string();
                }
                if let Some(columns) = columns {
                    page.columns = *columns;
                    for block in &mut page.blocks {
                        block.span = block.span.min(*columns);
                    }
                }
            }
            DesignOp::AddPage { page, nav_label } => {
                next.pages.push(page.clone());
                if let Some(label) = nav_label {
                    next.nav.items.push(NavItem { page: page.id.clone(), label: label.clone(), icon: String::new() });
                }
            }
            DesignOp::RemovePage { page } => {
                let before = next.pages.len();
                next.pages.retain(|p| &p.id != page);
                if next.pages.len() == before {
                    return err(format!("Halaman `{page}` tidak ada."));
                }
                next.nav.items.retain(|i| &i.page != page);
            }
            DesignOp::SetNav { nav } => next.nav = nav.clone(),
            DesignOp::SetCustomCss { css } => next.custom_css = css.clone(),
        }
        next.validate()?;
        Ok(next)
    }
}

/// Operasi desain yang dipanggil agen (dan dikirim ke browser untuk animasi).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesignOp {
    /// RFC 7386 merge patch ke `tokens`.
    SetTokens { changes: Json },
    SetThemeMode { mode: ThemeMode },
    AddBlock { page: String, #[serde(default)] index: Option<usize>, block: Block },
    /// `props` adalah merge patch ke props blok.
    UpdateBlock { id: String, #[serde(default)] span: Option<u8>, #[serde(default)] props: Option<Json> },
    DuplicateBlock { id: String, new_id: String },
    MoveBlock { id: String, page: String, #[serde(default)] index: Option<usize> },
    RemoveBlock { id: String },
    SetPageLayout { page: String, #[serde(default)] title: Option<String>, #[serde(default)] columns: Option<u8> },
    AddPage { page: Page, #[serde(default)] nav_label: Option<String> },
    RemovePage { page: String },
    SetNav { nav: Nav },
    /// Diisi setelah sanitasi; agen memanggil alat `set_custom_css`.
    SetCustomCss { css: String },
}

/// RFC 7386 JSON Merge Patch.
pub fn merge_patch(target: &mut Json, patch: &Json) {
    if let Json::Object(patch) = patch {
        if !target.is_object() {
            *target = Json::Object(Default::default());
        }
        let map = target.as_object_mut().expect("object");
        for (key, value) in patch {
            if value.is_null() {
                map.remove(key);
            } else {
                merge_patch(map.entry(key.clone()).or_insert(Json::Null), value);
            }
        }
    } else {
        *target = patch.clone();
    }
}

/// Desain bawaan: Konsol NOC, selalu gelap.
pub fn default_spec() -> DesignSpec {
    let spec: DesignSpec = serde_json::from_str(include_str!("default_design.json")).expect("default design valid JSON");
    spec.normalized()
}

impl DesignSpec {
    /// Panel selalu gelap (pilihan Usep 2026-11-25). Draft lama bermode light/system
    /// dinormalisasi saat dibaca, jadi tersimpan sebagai dark pada checkpoint berikutnya.
    pub fn normalized(mut self) -> Self {
        self.tokens.mode = ThemeMode::Dark;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn op(v: Json) -> DesignOp {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn default_is_valid() {
        default_spec().validate().unwrap();
    }

    #[test]
    fn always_dark() {
        let spec = default_spec();
        assert_eq!(spec.tokens.mode, ThemeMode::Dark);
        assert!(spec.apply(&op(json!({"op": "set_theme_mode", "mode": "light"}))).is_err());
        assert!(spec.apply(&op(json!({"op": "set_tokens", "changes": {"light": {"accent": "#ff0000"}}}))).is_err());
        assert!(spec.apply(&op(json!({"op": "set_tokens", "changes": {"mode": "system"}}))).is_err());
        let mut old = spec.clone();
        old.tokens.mode = ThemeMode::Light;
        let next = old.apply(&op(json!({"op": "set_tokens", "changes": {"dark": {"accent": "#38bdf8"}}}))).unwrap();
        assert_eq!(next.tokens.mode, ThemeMode::Dark);
    }

    #[test]
    fn set_tokens_and_contrast_guard() {
        let spec = default_spec();
        let next = spec.apply(&op(json!({"op":"set_tokens","changes":{"dark":{"accent":"#22d3ee"},"radius":4}}))).unwrap();
        assert_eq!(next.tokens.dark.accent, "#22d3ee");
        assert_eq!(next.tokens.radius, 4);
        let bad = spec.apply(&op(json!({"op":"set_tokens","changes":{"dark":{"text":"#1a1d24"}}})));
        assert!(bad.unwrap_err().0.contains("Kontras"));
        assert!(spec.apply(&op(json!({"op":"set_tokens","changes":{"dark":{"bg":"red"}}}))).is_err());
        assert!(spec.apply(&op(json!({"op":"set_tokens","changes":{"unknown":1}}))).is_err());
    }

    #[test]
    fn block_ops() {
        let spec = default_spec();
        let next = spec.apply(&op(json!({"op":"add_block","page":"ringkasan","index":0,
            "block":{"id":"offline","span":3,"kind":"customer_table","props":{"title":"Perlu dicek","filter":"offline","columns":["name","status"],"row_limit":10}}}))).unwrap();
        assert_eq!(next.pages[0].blocks[0].id, "offline");
        let next = next.apply(&op(json!({"op":"update_block","id":"offline","props":{"row_limit":5}}))).unwrap();
        match &next.pages[0].blocks[0].kind {
            BlockKind::CustomerTable { row_limit, .. } => assert_eq!(*row_limit, 5),
            _ => panic!(),
        }
        assert!(next.apply(&op(json!({"op":"update_block","id":"offline","props":{"evil":"<script>"}}))).is_err());
        let next = next.apply(&op(json!({"op":"move_block","id":"offline","page":"router"}))).unwrap();
        assert_eq!(next.pages[2].blocks.last().unwrap().id, "offline");
        assert_eq!(next.pages[2].blocks.last().unwrap().span, 2);
        assert!(spec.apply(&op(json!({"op":"duplicate_block","id":"trafik","new_id":"trafik"}))).is_err());
        assert!(spec.apply(&op(json!({"op":"remove_block","id":"agen-chat"}))).is_err());
        assert!(spec.apply(&op(json!({"op":"remove_page","page":"pengaturan-ai"}))).is_err());
        let next = spec.apply(&op(json!({"op":"set_page_layout","page":"ringkasan","columns":1}))).unwrap();
        assert!(next.pages[0].blocks.iter().all(|b| b.span == 1));
    }

    #[test]
    fn move_to_narrow_page_clamps_span() {
        let spec = default_spec();
        let next = spec.apply(&op(json!({"op":"move_block","id":"metrik-utama","page":"router","index":0}))).unwrap();
        assert_eq!(next.pages[2].blocks[0].span, 2);
        // update_block dengan span melebihi kolom tetap ditolak
        assert!(spec.apply(&op(json!({"op":"update_block","id":"trafik","span":4}))).is_err());
    }

    #[test]
    fn pages_and_nav() {
        let spec = default_spec();
        let next = spec.apply(&op(json!({"op":"add_page","nav_label":"VIP","page":{"id":"vip","title":"Pelanggan VIP","columns":2,"blocks":[]}}))).unwrap();
        assert!(next.nav.items.iter().any(|i| i.page == "vip"));
        let next = next.apply(&op(json!({"op":"remove_page","page":"vip"}))).unwrap();
        assert!(!next.nav.items.iter().any(|i| i.page == "vip"));
    }
}
