//! Alat baca router yang butuh logika agen: pohon perintah, cek perintah,
//! diagnosa jaringan, topologi, dan audit. Semuanya hanya membaca.
//! `RouterApi` diimplementasikan app di atas klien RouterOS API.
use std::collections::BTreeMap;

use panel_core::policy::command::Command;
use serde_json::{json, Value as Json};

pub type Row = BTreeMap<String, String>;

pub trait RouterApi: Send + Sync {
    /// Panggil `path` (mis. `/ip/address/print`) dengan argumen `=k=v` dan filter `?k=v`.
    fn call(&self, path: &str, args: &[(&str, String)], filters: &[(&str, String)]) -> impl Future<Output = Result<Vec<Row>, String>> + Send;
}

fn get<'a>(row: &'a Row, key: &str) -> &'a str {
    row.get(key).map(String::as_str).unwrap_or("")
}

/// Anak dari sebuah node pohon perintah (`/console/inspect request=child`).
pub async fn command_tree<R: RouterApi>(api: &R, path: &[String]) -> Result<Json, String> {
    let rows = api.call("/console/inspect", &[("request", "child".into()), ("path", path.join(","))], &[]).await?;
    let children: Vec<Json> = rows.iter()
        .filter(|r| get(r, "type") == "child" || r.contains_key("node-type"))
        .map(|r| json!({"name": get(r, "name"), "type": get(r, "node-type")}))
        .collect();
    Ok(json!({"path": path, "children": children}))
}

async fn child_names<R: RouterApi>(api: &R, path: &[String]) -> Result<Vec<(String, String)>, String> {
    let rows = api.call("/console/inspect", &[("request", "child".into()), ("path", path.join(","))], &[]).await?;
    Ok(rows.iter().map(|r| (get(r, "name").to_string(), get(r, "node-type").to_string())).collect())
}

/// Pastikan menu, perintah, dan nama parameter ada di versi RouterOS router ini.
pub async fn check_command<R: RouterApi>(api: &R, command: &Command) -> Result<Json, String> {
    command.validate().map_err(|e| e.0)?;
    let mut problems = Vec::new();
    let mut walked: Vec<String> = Vec::new();
    for part in &command.path {
        let children = child_names(api, &walked).await?;
        if !children.iter().any(|(n, t)| n == part && t == "dir") {
            problems.push(format!("Menu `{part}` tidak ada di /{}.", walked.join(" ")));
            return Ok(json!({"ok": false, "problems": problems}));
        }
        walked.push(part.clone());
    }
    let children = child_names(api, &walked).await?;
    if !children.iter().any(|(n, t)| n == &command.verb && t == "cmd") {
        let cmds: Vec<&str> = children.iter().filter(|(_, t)| t == "cmd").map(|(n, _)| n.as_str()).collect();
        problems.push(format!("Perintah `{}` tidak ada. Tersedia: {}.", command.verb, cmds.join(", ")));
        return Ok(json!({"ok": false, "problems": problems}));
    }
    walked.push(command.verb.clone());
    let args = child_names(api, &walked).await?;
    let known: Vec<&str> = args.iter().filter(|(_, t)| t == "arg").map(|(n, _)| n.as_str()).collect();
    for key in command.args.keys() {
        if !known.contains(&key.as_str()) {
            problems.push(format!("Parameter `{key}` tidak dikenal untuk {}.", command.verb));
        }
    }
    if !command.find.is_empty() {
        // Properti `find` dicek terhadap argumen `set`/`print` menu yang sama.
        let mut print_path = command.path.clone();
        print_path.push("print".into());
        let props = child_names(api, &print_path).await.unwrap_or_default();
        let mut set_path = command.path.clone();
        set_path.push("set".into());
        let set_props = child_names(api, &set_path).await.unwrap_or_default();
        for key in command.find.keys() {
            let exists = props.iter().chain(&set_props).any(|(n, _)| n == key) || matches!(key.as_str(), "name" | ".id" | "comment" | "disabled");
            if !exists {
                problems.push(format!("Properti `{key}` untuk find tidak dikenal di /{}.", command.path.join(" ")));
            }
        }
    }
    Ok(json!({"ok": problems.is_empty(), "problems": problems, "known_args": known}))
}

fn valid_host(address: &str) -> bool {
    !address.is_empty() && address.len() <= 253
        && address.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
}

pub async fn net_diagnose<R: RouterApi>(api: &R, kind: &str, address: &str, count: Option<u64>) -> Result<Json, String> {
    if !valid_host(address) {
        return Err("Alamat harus IP atau nama host yang valid.".into());
    }
    let (path, max) = match kind {
        "ping" => ("/ping", 10),
        "traceroute" => ("/tool/traceroute", 3),
        _ => return Err("kind harus ping atau traceroute.".into()),
    };
    let count = count.unwrap_or(if kind == "ping" { 4 } else { 1 }).clamp(1, max);
    let rows = api.call(path, &[("address", address.into()), ("count", count.to_string())], &[]).await?;
    Ok(json!({"kind": kind, "address": address, "rows": rows.into_iter().take(60).collect::<Vec<_>>()}))
}

/// Graf topologi sederhana dari data router.
pub async fn topology<R: RouterApi>(api: &R) -> Result<Json, String> {
    let identity = api.call("/system/identity/print", &[], &[]).await?;
    let interfaces = api.call("/interface/print", &[(".proplist", "name,type,running,disabled,mac-address,comment".into())], &[]).await?;
    let ports = api.call("/interface/bridge/port/print", &[(".proplist", "bridge,interface,pvid,disabled".into())], &[]).await.unwrap_or_default();
    let vlans = api.call("/interface/vlan/print", &[(".proplist", "name,vlan-id,interface".into())], &[]).await.unwrap_or_default();
    let addresses = api.call("/ip/address/print", &[(".proplist", "address,interface,disabled".into())], &[]).await.unwrap_or_default();
    let neighbors = api.call("/ip/neighbor/print", &[(".proplist", "interface,address,mac-address,identity,platform,board,version".into())], &[]).await.unwrap_or_default();
    let arp = api.call("/ip/arp/print", &[(".proplist", "interface".into())], &[]).await.unwrap_or_default();
    let mut arp_count: BTreeMap<String, u32> = BTreeMap::new();
    for row in &arp {
        *arp_count.entry(get(row, "interface").to_string()).or_default() += 1;
    }
    let mut bridges: BTreeMap<String, Vec<Json>> = BTreeMap::new();
    for p in &ports {
        bridges.entry(get(p, "bridge").to_string()).or_default()
            .push(json!({"interface": get(p, "interface"), "pvid": get(p, "pvid"), "disabled": get(p, "disabled") == "true"}));
    }
    Ok(json!({
        "router": identity.first().map(|r| get(r, "name")).unwrap_or(""),
        "interfaces": interfaces.iter().map(|i| json!({
            "name": get(i, "name"), "type": get(i, "type"), "running": get(i, "running") == "true",
            "disabled": get(i, "disabled") == "true", "comment": get(i, "comment"),
            "arp_hosts": arp_count.get(get(i, "name")).copied().unwrap_or(0),
        })).collect::<Vec<_>>(),
        "bridges": bridges,
        "vlans": vlans,
        "addresses": addresses,
        "neighbors": neighbors,
    }))
}

fn finding(kind: &str, severity: &str, title: &str, evidence: String, suggestion: &str) -> Json {
    json!({"kind": kind, "severity": severity, "title": title, "evidence": evidence, "suggestion": suggestion})
}

/// Audit berbasis aturan. Model memakai temuan ini untuk membuat kartu saran.
pub async fn audit<R: RouterApi>(api: &R, focus: &[String]) -> Result<Json, String> {
    let wants = |k: &str| focus.is_empty() || focus.iter().any(|f| f == k);
    let mut out = Vec::new();

    if wants("security") {
        let services = api.call("/ip/service/print", &[(".proplist", "name,port,disabled,address".into())], &[]).await.unwrap_or_default();
        for s in &services {
            let name = get(s, "name");
            if get(s, "disabled") == "true" {
                continue;
            }
            if matches!(name, "telnet" | "ftp" | "www") {
                out.push(finding("security", "high", &format!("Layanan {name} aktif"), format!("/ip service {name} port {} aktif", get(s, "port")),
                    "Nonaktifkan layanan tanpa enkripsi ini; pakai ssh/www-ssl/api-ssl."));
            } else if get(s, "address").is_empty() && matches!(name, "ssh" | "winbox" | "api" | "api-ssl" | "www-ssl") {
                out.push(finding("security", "medium", &format!("Layanan {name} terbuka dari semua alamat"), format!("/ip service {name} tanpa batas address"),
                    "Batasi kolom address ke subnet manajemen."));
            }
        }
        let dns = api.call("/ip/dns/print", &[], &[]).await.unwrap_or_default();
        let input = api.call("/ip/firewall/filter/print", &[(".proplist", "chain,action,protocol,dst-port,in-interface,in-interface-list,disabled".into())], &[("chain", "input".into())]).await.unwrap_or_default();
        let active_input: Vec<&Row> = input.iter().filter(|r| get(r, "disabled") != "true").collect();
        if dns.first().is_some_and(|d| get(d, "allow-remote-requests") == "true")
            && !active_input.iter().any(|r| get(r, "action") == "drop" && get(r, "dst-port").contains("53"))
        {
            out.push(finding("security", "high", "DNS router bisa dipakai dari internet", "allow-remote-requests=yes tanpa aturan drop port 53 di chain input".into(),
                "Tambahkan aturan drop udp/tcp 53 dari interface WAN, atau matikan allow-remote-requests."));
        }
        if !active_input.iter().any(|r| get(r, "action") == "drop") {
            out.push(finding("security", "high", "Chain input tidak punya aturan drop", format!("{} aturan input aktif, tidak ada yang drop", active_input.len()),
                "Tambahkan aturan accept established/related, accept dari LAN/manajemen, lalu drop sisanya dari WAN."));
        }
        let discovery = api.call("/ip/neighbor/discovery-settings/print", &[], &[]).await.unwrap_or_default();
        if discovery.first().is_some_and(|d| get(d, "discover-interface-list") == "all") {
            out.push(finding("security", "low", "Neighbor discovery aktif di semua interface", "discover-interface-list=all".into(),
                "Batasi ke interface list LAN."));
        }
    }

    if wants("performance") {
        let resource = api.call("/system/resource/print", &[], &[]).await?;
        if let Some(r) = resource.first() {
            let cpu: u32 = get(r, "cpu-load").parse().unwrap_or(0);
            let free: f64 = get(r, "free-memory").parse().unwrap_or(0.0);
            let total: f64 = get(r, "total-memory").parse().unwrap_or(0.0);
            if cpu >= 80 {
                out.push(finding("performance", "high", "CPU router tinggi", format!("cpu-load {cpu}%"), "Periksa /tool profile, aktifkan FastTrack, kurangi aturan firewall/queue yang berat."));
            }
            if total > 0.0 && free / total < 0.1 {
                out.push(finding("performance", "medium", "RAM router hampir penuh", format!("free {:.0}% dari {:.0} MiB", 100.0 * free / total, total / 1048576.0), "Kurangi log di memori, cek koneksi/connection tracking."));
            }
            out.push(json!({"kind": "info", "board": get(r, "board-name"), "version": get(r, "version"), "architecture": get(r, "architecture-name"), "cpu_count": get(r, "cpu-count")}));
        }
        let forward = api.call("/ip/firewall/filter/print", &[(".proplist", "action,disabled".into())], &[("chain", "forward".into())]).await.unwrap_or_default();
        if !forward.iter().any(|r| get(r, "action") == "fasttrack-connection" && get(r, "disabled") != "true") {
            out.push(finding("performance", "medium", "FastTrack belum aktif", "tidak ada aturan fasttrack-connection di chain forward".into(),
                "Tambahkan fasttrack untuk established/related (perhatikan: queue simple tidak berlaku untuk koneksi yang di-fasttrack)."));
        }
    }

    if wants("topology") {
        let vlans = api.call("/interface/vlan/print", &[(".proplist", "name".into())], &[]).await.unwrap_or_default();
        let ports = api.call("/interface/bridge/port/print", &[(".proplist", "bridge".into())], &[]).await.unwrap_or_default();
        if vlans.is_empty() && ports.len() >= 3 {
            out.push(finding("topology", "low", "Belum ada VLAN", format!("{} port bridge, 0 VLAN", ports.len()),
                "Pertimbangkan memisahkan VLAN manajemen dan pelanggan."));
        }
    }

    if wants("layout") {
        let wifi = api.call("/interface/wifi/print", &[(".proplist", "name".into())], &[]).await.unwrap_or_default();
        let wireless = api.call("/interface/wireless/print", &[(".proplist", "name".into())], &[]).await.unwrap_or_default();
        let ppp = api.call("/ppp/active/print", &[(".proplist", "name".into())], &[]).await.unwrap_or_default();
        out.push(json!({"kind": "layout_hint", "wireless_interfaces": wifi.len() + wireless.len(), "ppp_active": ppp.len()}));
    }
    Ok(json!({"findings": out}))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;
    fn rows(list: &[&[(&str, &str)]]) -> Vec<Row> {
        list.iter().map(|r| r.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()).collect()
    }
    impl RouterApi for Fake {
        async fn call(&self, path: &str, args: &[(&str, String)], _filters: &[(&str, String)]) -> Result<Vec<Row>, String> {
            let arg = |k: &str| args.iter().find(|(a, _)| *a == k).map(|(_, v)| v.as_str()).unwrap_or("");
            Ok(match (path, arg("path")) {
                ("/console/inspect", "") => rows(&[&[("name", "ip"), ("node-type", "dir")], &[("name", "ppp"), ("node-type", "dir")]]),
                ("/console/inspect", "ppp") => rows(&[&[("name", "active"), ("node-type", "dir")]]),
                ("/console/inspect", "ppp,active") => rows(&[&[("name", "remove"), ("node-type", "cmd")], &[("name", "print"), ("node-type", "cmd")]]),
                ("/console/inspect", "ppp,active,remove") => rows(&[&[("name", "numbers"), ("node-type", "arg")]]),
                ("/console/inspect", "ppp,active,print") => rows(&[&[("name", "where"), ("node-type", "arg")]]),
                ("/ip/service/print", _) => rows(&[&[("name", "telnet"), ("port", "23"), ("disabled", "false")], &[("name", "ssh"), ("port", "22"), ("disabled", "false"), ("address", "")]]),
                ("/ip/dns/print", _) => rows(&[&[("allow-remote-requests", "true")]]),
                ("/system/resource/print", _) => rows(&[&[("cpu-load", "91"), ("free-memory", "10000000"), ("total-memory", "268435456"), ("board-name", "hAP ax2")]]),
                ("/system/identity/print", _) => rows(&[&[("name", "core")]]),
                ("/interface/print", _) => rows(&[&[("name", "ether1"), ("type", "ether"), ("running", "true")]]),
                ("/ip/arp/print", _) => rows(&[&[("interface", "ether1")], &[("interface", "ether1")]]),
                _ => vec![],
            })
        }
    }

    #[tokio::test]
    async fn checks_command_against_tree() {
        let ok: Command = serde_json::from_value(json!({"path":["ppp","active"],"verb":"remove","find":{"name":"budi"}})).unwrap();
        assert_eq!(check_command(&Fake, &ok).await.unwrap()["ok"], true);
        let bad_verb: Command = serde_json::from_value(json!({"path":["ppp","active"],"verb":"kick"})).unwrap();
        let r = check_command(&Fake, &bad_verb).await.unwrap();
        assert_eq!(r["ok"], false);
        assert!(r["problems"][0].as_str().unwrap().contains("remove, print"));
        let bad_arg: Command = serde_json::from_value(json!({"path":["ppp","active"],"verb":"remove","args":{"force":true}})).unwrap();
        assert_eq!(check_command(&Fake, &bad_arg).await.unwrap()["ok"], false);
        let bad_menu: Command = serde_json::from_value(json!({"path":["hotspot"],"verb":"print"})).unwrap();
        assert_eq!(check_command(&Fake, &bad_menu).await.unwrap()["ok"], false);
    }

    #[tokio::test]
    async fn audit_finds_issues() {
        let r = audit(&Fake, &[]).await.unwrap();
        let titles: Vec<&str> = r["findings"].as_array().unwrap().iter().filter_map(|f| f["title"].as_str()).collect();
        for expected in ["Layanan telnet aktif", "Layanan ssh terbuka dari semua alamat", "DNS router bisa dipakai dari internet",
                         "Chain input tidak punya aturan drop", "CPU router tinggi", "RAM router hampir penuh", "FastTrack belum aktif"] {
            assert!(titles.contains(&expected), "{expected} not in {titles:?}");
        }
    }

    #[tokio::test]
    async fn topology_and_diagnose() {
        let t = topology(&Fake).await.unwrap();
        assert_eq!(t["router"], "core");
        assert_eq!(t["interfaces"][0]["arp_hosts"], 2);
        assert!(net_diagnose(&Fake, "ping", "1.1.1.1; /system reboot", None).await.is_err());
        assert!(net_diagnose(&Fake, "ping", "1.1.1.1", Some(50)).await.is_ok());
    }
}
