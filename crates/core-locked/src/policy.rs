//! Tingkat izin alat agen. Ditegakkan di server: frontend hanya menampilkan.
use serde::{Deserialize, Serialize};

/// Seberapa jauh sebuah alat boleh berjalan tanpa klik admin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Hanya membaca, atau mengubah draft desain yang selalu bisa di-revert.
    Free,
    /// Tampil sebagai kartu; berjalan setelah admin klik setuju.
    Approve,
    /// Kartu dengan konfirmasi ketik nama tindakan.
    Critical,
}

impl Permission {
    /// Teks yang harus diketik admin untuk tindakan kritis.
    pub fn confirmation_phrase(self, action_label: &str) -> Option<String> {
        (self == Permission::Critical).then(|| action_label.trim().to_lowercase())
    }

    /// Apakah konfirmasi admin cukup untuk menjalankan tindakan ini.
    pub fn is_satisfied(self, approved: bool, typed: Option<&str>, action_label: &str) -> bool {
        match self {
            Permission::Free => true,
            Permission::Approve => approved,
            Permission::Critical => {
                approved
                    && typed.map(|t| t.trim().to_lowercase())
                        == self.confirmation_phrase(action_label)
            }
        }
    }
}

/// Batas loop agen per giliran.
pub const MAX_AGENT_STEPS: usize = 12;
/// Batas jumlah alat yang dipanggil dalam satu langkah.
pub const MAX_TOOL_CALLS_PER_STEP: usize = 8;

/// Path kode sumber yang tidak boleh diubah alat edit kode agen.
pub const LOCKED_PATHS: &[&str] = &[
    "crates/core-locked/",
    "Cargo.toml",
    "Cargo.lock",
    ".github/",
    "deploy/",
    "rust-toolchain.toml",
];

/// Apakah path relatif (dari akar repo) boleh diedit agen.
pub fn code_path_editable(path: &str) -> bool {
    let path = path.trim_start_matches("./");
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.split('/').any(|part| part == ".." || part.is_empty() || part == ".git")
    {
        return false;
    }
    !LOCKED_PATHS
        .iter()
        .any(|locked| path == locked.trim_end_matches('/') || path.starts_with(locked))
}

/// Perintah RouterOS terstruktur: agen mengirim JSON (path, verb, find, args),
/// bukan string CLI mentah, lalu divalidasi dan di-escape di sini.
pub mod command {
    // Perintah RouterOS terstruktur. Agen tidak pernah mengirim string CLI mentah:
    // ia mengirim JSON (path, verb, find, args) yang divalidasi di sini lalu
    // dirender dengan escape yang benar. Ini menutup celah injeksi seperti
    // `"; /system reset-configuration` di dalam nilai parameter.
    use std::collections::{BTreeMap, BTreeSet};

    use serde::{Deserialize, Serialize};

    use super::Permission;

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    #[serde(untagged)]
    pub enum Value {
        Int(i64),
        Bool(bool),
        Text(String),
        /// Placeholder di template tombol, diisi saat tombol dijalankan.
        Param {
            param: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            multiply: Option<i64>,
        },
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Command {
        /// Menu, misalnya `["ppp", "active"]`.
        pub path: Vec<String>,
        /// Perintah, misalnya `remove`.
        pub verb: String,
        /// Kondisi `[find where k=v ...]` (semua harus cocok).
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pub find: BTreeMap<String, Value>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pub args: BTreeMap<String, Value>,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum ParamKind {
        Text { max_len: usize },
        Integer { min: i64, max: i64 },
        Choice { values: Vec<String> },
        /// Nama pelanggan; harus ada di daftar pelanggan saat dijalankan.
        Customer,
        /// Nama profil; harus ada di router saat dijalankan.
        Profile,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    // `flatten` tidak bisa digabung dengan deny_unknown_fields; nama diperiksa di `validate_params`.
    pub struct ParamSpec {
        pub name: String,
        pub label: String,
        #[serde(flatten)]
        pub kind: ParamKind,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CommandError(pub String);

    impl std::fmt::Display for CommandError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl std::error::Error for CommandError {}

    fn err<T>(msg: impl Into<String>) -> Result<T, CommandError> {
        Err(CommandError(msg.into()))
    }

    const MAX_TEXT: usize = 512;
    const MAX_ARGS: usize = 24;

    /// Menu yang sama sekali tidak boleh disentuh agen (awalan path).
    const BLOCKED_PATHS: &[&[&str]] = &[
        &["system", "reset-configuration"],
        &["system", "package"],
        &["system", "script"],
        &["system", "scheduler"],
        &["system", "backup"],
        &["system", "license"],
        &["system", "device-mode"],
        &["system", "routerboard"],
        &["user"],
        &["ip", "service"],
        &["ip", "ssh"],
        &["ip", "socks"],
        &["file"],
        &["certificate"],
        &["tool", "fetch"],
        &["tool", "e-mail"],
        &["tool", "sniffer"],
        &["tool", "netwatch"],
        &["disk"],
        &["container"],
    ];

    /// Menu yang perubahannya bisa memutus akses panel ke router: selalu kritis.
    const CRITICAL_PATHS: &[&[&str]] = &[
        &["ip", "firewall"],
        &["ip", "address"],
        &["ip", "route"],
        &["ip", "dns"],
        &["ip", "dhcp-client"],
        &["interface"],
        &["routing"],
        &["radius"],
        &["system"],
    ];

    const READ_VERBS: &[&str] = &["print", "get", "monitor", "monitor-traffic", "export"];
    const CRITICAL_VERBS: &[&str] = &["reboot", "shutdown"];
    const ROOT_VERBS: &[&str] = &["ping"];

    fn valid_name(name: &str, allow_dot: bool) -> bool {
        let mut chars = name.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
            && name.len() <= 48
            && chars.all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || (allow_dot && c == '.')
            })
    }

    fn valid_text(text: &str) -> bool {
        text.chars().count() <= MAX_TEXT && !text.chars().any(char::is_control)
    }

    fn starts_with(path: &[String], prefix: &[&str]) -> bool {
        path.len() >= prefix.len() && path.iter().zip(prefix).all(|(a, b)| a == b)
    }

    impl Command {
        /// Validasi bentuk dan blocklist. Tidak mengecek keberadaan menu di router;
        /// itu tugas pohon perintah dari `/console/inspect`.
        pub fn validate(&self) -> Result<(), CommandError> {
            if self.path.len() > 6 || !self.path.iter().all(|p| valid_name(p, false)) {
                return err("Path menu RouterOS tidak valid.");
            }
            if !valid_name(&self.verb, false) {
                return err("Nama perintah RouterOS tidak valid.");
            }
            if self.path.is_empty() && !ROOT_VERBS.contains(&self.verb.as_str()) {
                return err(format!("Perintah /{} tidak diizinkan untuk agen.", self.verb));
            }
            let mut full = self.path.clone();
            full.push(self.verb.clone());
            if let Some(blocked) = BLOCKED_PATHS.iter().find(|b| starts_with(&full, b)) {
                return err(format!("Menu /{} diblokir untuk agen.", blocked.join(" ")));
            }
            if self.args.len() + self.find.len() > MAX_ARGS {
                return err("Terlalu banyak parameter.");
            }
            for (key, value) in self.find.iter().chain(&self.args) {
                if !valid_name(key, true) {
                    return err(format!("Nama parameter `{key}` tidak valid."));
                }
                if let Value::Text(text) = value {
                    if !valid_text(text) {
                        return err(format!("Nilai `{key}` terlalu panjang atau berisi karakter kontrol."));
                    }
                }
                if let Value::Param { param, multiply } = value {
                    if !valid_name(param, false) || matches!(multiply, Some(m) if *m <= 0) {
                        return err(format!("Placeholder `{key}` tidak valid."));
                    }
                }
            }
            if self.verb == "export" && self.args.contains_key("file") {
                return err("Export ke file tidak diizinkan.");
            }
            if self.verb == "ping" {
                match self.args.get("count") {
                    Some(Value::Int(n)) if (1..=10).contains(n) => {}
                    _ => return err("Ping wajib memakai count 1 sampai 10."),
                }
            }
            if self.path == ["tool", "traceroute"] {
                match self.args.get("count") {
                    Some(Value::Int(n)) if (1..=3).contains(n) => {}
                    _ => return err("Traceroute wajib memakai count 1 sampai 3."),
                }
            }
            if self.verb == "monitor-traffic" && !self.args.contains_key("once") {
                return err("monitor-traffic wajib memakai once.");
            }
            Ok(())
        }

        /// Tingkat izin minimum untuk perintah ini.
        pub fn permission(&self) -> Permission {
            let verb = self.verb.as_str();
            if READ_VERBS.contains(&verb) || verb == "ping" || self.path == ["tool", "traceroute"] {
                Permission::Free
            } else if CRITICAL_VERBS.contains(&verb)
                || CRITICAL_PATHS.iter().any(|p| starts_with(&self.path, p))
            {
                Permission::Critical
            } else {
                Permission::Approve
            }
        }

        pub fn placeholders(&self) -> BTreeSet<String> {
            self.find
                .values()
                .chain(self.args.values())
                .filter_map(|v| match v {
                    Value::Param { param, .. } => Some(param.clone()),
                    _ => None,
                })
                .collect()
        }

        /// Render ke satu baris CLI RouterOS. Gagal bila masih ada placeholder.
        pub fn render(&self) -> Result<String, CommandError> {
            self.validate()?;
            let mut out = String::from("/");
            out.push_str(&self.path.join(" "));
            if !self.path.is_empty() {
                out.push(' ');
            }
            out.push_str(&self.verb);
            if !self.find.is_empty() {
                out.push_str(" [find where");
                for (key, value) in &self.find {
                    out.push(' ');
                    out.push_str(key);
                    out.push('=');
                    out.push_str(&render_value(value)?);
                }
                out.push(']');
            }
            for (key, value) in &self.args {
                out.push(' ');
                out.push_str(key);
                out.push('=');
                out.push_str(&render_value(value)?);
            }
            Ok(out)
        }

        /// Path RouterOS API, mis. `/ppp/active/remove`.
        pub fn api_path(&self) -> String {
            let mut path = String::from("/");
            for part in &self.path {
                path.push_str(part);
                path.push('/');
            }
            path.push_str(&self.verb);
            path
        }

        /// Path `print` untuk mencari `.id` objek yang cocok dengan `find`.
        pub fn api_find_path(&self) -> String {
            format!("/{}/print", self.path.join("/"))
        }

        /// Argumen dan filter sebagai string (bool = yes/no). Gagal bila masih ada placeholder.
        pub fn api_pairs(map: &BTreeMap<String, Value>) -> Result<Vec<(String, String)>, CommandError> {
            map.iter().map(|(k, v)| Ok((k.clone(), match v {
                Value::Int(n) => n.to_string(),
                Value::Bool(b) => if *b { "yes" } else { "no" }.to_string(),
                Value::Text(t) => t.clone(),
                Value::Param { param, .. } => return err(format!("Parameter `{param}` belum diisi.")),
            }))).collect()
        }

        /// Isi placeholder dengan nilai yang diketik/dipilih admin.
        pub fn fill(
            &self,
            params: &[ParamSpec],
            values: &BTreeMap<String, Input>,
            ctx: &FillContext,
        ) -> Result<Command, CommandError> {
            validate_params(params)?;
            let declared: BTreeSet<String> = params.iter().map(|p| p.name.clone()).collect();
            if self.placeholders() != declared {
                return err("Placeholder template tidak cocok dengan daftar parameter.");
            }
            let fill_value = |value: &Value| -> Result<Value, CommandError> {
                let Value::Param { param, multiply } = value else {
                    return Ok(value.clone());
                };
                let spec = params.iter().find(|p| &p.name == param).expect("declared");
                let raw = values
                    .get(param)
                    .ok_or_else(|| CommandError(format!("Isi parameter {}.", spec.label)))?;
                let filled = spec.kind.check(&spec.label, raw, ctx)?;
                match (filled, multiply) {
                    (Value::Int(n), Some(m)) => n
                        .checked_mul(*m)
                        .map(Value::Int)
                        .ok_or_else(|| CommandError("Angka terlalu besar.".into())),
                    (filled, None) => Ok(filled),
                    _ => err("Pengali hanya untuk parameter angka."),
                }
            };
            let mut out = self.clone();
            for value in out.find.values_mut().chain(out.args.values_mut()) {
                *value = fill_value(value)?;
            }
            out.validate()?;
            Ok(out)
        }
    }

    /// Validasi daftar parameter tombol.
    pub fn validate_params(params: &[ParamSpec]) -> Result<(), CommandError> {
        let mut seen = BTreeSet::new();
        if params.len() > 8 {
            return err("Maksimal 8 parameter per tombol.");
        }
        for p in params {
            if !valid_name(&p.name, false) || !seen.insert(&p.name) {
                return err(format!("Nama parameter `{}` tidak valid atau ganda.", p.name));
            }
            if p.label.trim().is_empty() || p.label.chars().count() > 40 || !valid_text(&p.label) {
                return err("Label parameter harus 1 sampai 40 karakter.");
            }
            match &p.kind {
                ParamKind::Text { max_len } if !(1..=MAX_TEXT).contains(max_len) => return err("max_len tidak valid."),
                ParamKind::Integer { min, max } if min > max => return err("Rentang angka tidak valid."),
                ParamKind::Choice { values } if values.is_empty() || values.len() > 30 || values.iter().any(|v| !valid_text(v)) => {
                    return err("Pilihan harus 1 sampai 30 nilai.")
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Nilai parameter yang diisi admin saat menjalankan tombol.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    #[serde(untagged)]
    pub enum Input {
        Int(i64),
        Text(String),
    }

    impl Input {
        fn as_i64(&self) -> Option<i64> {
            match self { Input::Int(n) => Some(*n), Input::Text(_) => None }
        }
        fn as_str(&self) -> Option<&str> {
            match self { Input::Text(t) => Some(t), Input::Int(_) => None }
        }
    }

    /// Data router yang dipakai untuk memvalidasi parameter Customer/Profile.
    #[derive(Default)]
    pub struct FillContext {
        pub customers: BTreeSet<String>,
        pub profiles: BTreeSet<String>,
    }

    impl ParamKind {
        fn check(&self, label: &str, raw: &Input, ctx: &FillContext) -> Result<Value, CommandError> {
            let text = || raw.as_str().map(str::to_owned);
            match self {
                ParamKind::Integer { min, max } => match raw.as_i64() {
                    Some(n) if (*min..=*max).contains(&n) => Ok(Value::Int(n)),
                    _ => err(format!("{label} harus angka {min} sampai {max}.")),
                },
                ParamKind::Text { max_len } => match text() {
                    Some(t) if t.chars().count() <= *max_len && valid_text(&t) => Ok(Value::Text(t)),
                    _ => err(format!("{label} maksimal {max_len} karakter tanpa baris baru.")),
                },
                ParamKind::Choice { values } => match text() {
                    Some(t) if values.contains(&t) => Ok(Value::Text(t)),
                    _ => err(format!("Pilihan {label} tidak dikenal.")),
                },
                ParamKind::Customer => match text() {
                    Some(t) if ctx.customers.contains(&t) => Ok(Value::Text(t)),
                    _ => err(format!("{label}: pelanggan tidak ditemukan.")),
                },
                ParamKind::Profile => match text() {
                    Some(t) if ctx.profiles.contains(&t) => Ok(Value::Text(t)),
                    _ => err(format!("{label}: profil tidak ditemukan di router.")),
                },
            }
        }
    }

    fn render_value(value: &Value) -> Result<String, CommandError> {
        Ok(match value {
            Value::Int(n) => n.to_string(),
            Value::Bool(b) => if *b { "yes" } else { "no" }.to_string(),
            Value::Text(t) => quote(t),
            Value::Param { param, .. } => return err(format!("Parameter `{param}` belum diisi.")),
        })
    }

    /// Kutip string untuk CLI RouterOS: `\`, `"`, `$`, dan `?` di-escape.
    pub fn quote(text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 2);
        out.push('"');
        for c in text.chars() {
            if matches!(c, '\\' | '"' | '$' | '?') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('"');
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;

        fn cmd(v: serde_json::Value) -> Command {
            serde_json::from_value(v).unwrap()
        }

        #[test]
        fn renders_with_escaping() {
            let c = cmd(json!({"path":["ppp","active"],"verb":"remove","find":{"name":"a\"; /system reset-configuration $x"}}));
            assert_eq!(
                c.render().unwrap(),
                r#"/ppp active remove [find where name="a\"; /system reset-configuration \$x"]"#
            );
            assert_eq!(c.permission(), Permission::Approve);
        }

        #[test]
        fn blocks_dangerous_menus() {
            for v in [
                json!({"path":["system"],"verb":"reset-configuration"}),
                json!({"path":["system","script"],"verb":"add","args":{"source":"x"}}),
                json!({"path":["user"],"verb":"add"}),
                json!({"path":["ip","service"],"verb":"set"}),
                json!({"path":[],"verb":"import"}),
                json!({"path":["file"],"verb":"remove"}),
                json!({"path":[],"verb":"export","args":{"file":"x"}}),
                json!({"path":["ip","address"],"verb":"print","args":{"file":"x"}}),
            ] {
                let c = cmd(v.clone());
                if c.verb == "print" {
                    continue; // print file= diblok via export saja; print tetap aman tanpa file
                }
                assert!(c.validate().is_err(), "{v}");
            }
            assert!(cmd(json!({"path":["ip","address"],"verb":"print"})).validate().is_ok());
        }

        #[test]
        fn rejects_bad_names_and_control_chars() {
            assert!(cmd(json!({"path":["ip;"],"verb":"print"})).validate().is_err());
            assert!(cmd(json!({"path":["ip"],"verb":"Print"})).validate().is_err());
            assert!(cmd(json!({"path":["ppp","secret"],"verb":"set","args":{"comment":"a\nb"}})).validate().is_err());
            assert!(cmd(json!({"path":["ppp","secret"],"verb":"set","args":{"bad key":"x"}})).validate().is_err());
        }

        #[test]
        fn risk_levels() {
            assert_eq!(cmd(json!({"path":["system"],"verb":"reboot"})).permission(), Permission::Critical);
            assert_eq!(cmd(json!({"path":["ip","firewall","filter"],"verb":"add"})).permission(), Permission::Critical);
            assert_eq!(cmd(json!({"path":["interface"],"verb":"print"})).permission(), Permission::Free);
            assert_eq!(cmd(json!({"path":["user-manager","user"],"verb":"set"})).permission(), Permission::Approve);
            assert!(cmd(json!({"path":[],"verb":"ping","args":{"address":"1.1.1.1"}})).validate().is_err());
            assert!(cmd(json!({"path":[],"verb":"ping","args":{"address":"1.1.1.1","count":4}})).validate().is_ok());
        }

        #[test]
        fn fills_template() {
            let t = cmd(json!({"path":["user-manager","user"],"verb":"set",
                "find":{"name":{"param":"customer"}},
                "args":{"group":{"param":"profile"},"comment":{"param":"note"}}}));
            let params: Vec<ParamSpec> = serde_json::from_value(json!([
                {"name":"customer","label":"Pelanggan","type":"customer"},
                {"name":"profile","label":"Profil","type":"profile"},
                {"name":"note","label":"Catatan","type":"text","max_len":40}
            ])).unwrap();
            let ctx = FillContext {
                customers: ["budi".to_string()].into(),
                profiles: ["gold".to_string()].into(),
            };
            let mut values = BTreeMap::new();
            values.insert("customer".into(), Input::Text("budi".into()));
            values.insert("profile".into(), Input::Text("gold".into()));
            values.insert("note".into(), Input::Text("VIP".into()));
            let filled = t.fill(&params, &values, &ctx).unwrap();
            assert_eq!(filled.render().unwrap(),
                r#"/user-manager user set [find where name="budi"] comment="VIP" group="gold""#);
            values.insert("customer".into(), Input::Text("tidak-ada".into()));
            assert!(t.fill(&params, &values, &ctx).is_err());
            assert!(t.render().is_err());
            assert_eq!(filled.api_path(), "/user-manager/user/set");
            assert_eq!(filled.api_find_path(), "/user-manager/user/print");
            assert_eq!(Command::api_pairs(&filled.find).unwrap(), vec![("name".to_string(), "budi".to_string())]);
        }

        #[test]
        fn multiply_param() {
            let t = cmd(json!({"path":["queue","simple"],"verb":"set","find":{"name":"x"},
                "args":{"max-limit":{"param":"mbit","multiply":1000000}}}));
            let params: Vec<ParamSpec> = serde_json::from_value(json!([
                {"name":"mbit","label":"Mbps","type":"integer","min":1,"max":1000}])).unwrap();
            let values = [("mbit".to_string(), Input::Int(20))].into();
            let filled = t.fill(&params, &values, &FillContext::default()).unwrap();
            assert_eq!(filled.render().unwrap(), r#"/queue simple set [find where name="x"] max-limit=20000000"#);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn critical_needs_typed_phrase() {
        let p = Permission::Critical;
        assert!(!p.is_satisfied(true, None, "Reboot router"));
        assert!(!p.is_satisfied(true, Some("reboot"), "Reboot router"));
        assert!(p.is_satisfied(true, Some(" reboot ROUTER "), "Reboot router"));
        assert!(!p.is_satisfied(false, Some("reboot router"), "Reboot router"));
    }

    #[test]
    fn locked_paths() {
        assert!(code_path_editable("crates/app/src/main.rs"));
        assert!(code_path_editable("web/src/app.tsx"));
        assert!(!code_path_editable("crates/core-locked/src/policy.rs"));
        assert!(!code_path_editable("crates/app/../core-locked/src/lib.rs"));
        assert!(!code_path_editable("/etc/passwd"));
        assert!(!code_path_editable(".github/workflows/build.yml"));
        assert!(!code_path_editable(".git/config"));
        assert!(!code_path_editable("Cargo.toml"));
        assert!(code_path_editable("crates/app/Cargo.toml"));
    }
}
