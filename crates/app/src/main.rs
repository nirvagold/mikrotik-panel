mod events;
mod http;
mod routeros;
mod store;

use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use panel_core::auth::{LoginLimiter, Sessions};
use panel_core::config::Config;
use panel_core::crypto::{self, Cipher};

const USAGE: &str = "Pemakaian:
  mikrotik-panel serve [--config PATH] [--port PORT]   jalankan panel
  mikrotik-panel gen-key                               buat secret_key baru
  mikrotik-panel hash-password                         hash kata sandi admin (dibaca dari stdin)
  mikrotik-panel encrypt [--config PATH]               enkripsi kata sandi router (dibaca dari stdin)";

const DEFAULT_CONFIG: &str = "/etc/mikrotik-panel/config.toml";

struct Args {
    command: String,
    config: PathBuf,
    port: Option<u16>,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut it = std::env::args().skip(1);
    let command = it.next().unwrap_or_else(|| "serve".into());
    let mut args = Args {
        command,
        config: DEFAULT_CONFIG.into(),
        port: None,
    };
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--config" => args.config = it.next().context("--config butuh path")?.into(),
            "--port" => {
                args.port = Some(
                    it.next()
                        .context("--port butuh angka")?
                        .parse()
                        .context("port tidak valid")?,
                )
            }
            _ => bail!("argumen tidak dikenal: {flag}\n\n{USAGE}"),
        }
    }
    Ok(args)
}

fn read_secret(prompt: &str) -> anyhow::Result<String> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let value = line.trim_end_matches(['\r', '\n']).to_string();
    if value.is_empty() {
        bail!("input kosong");
    }
    Ok(value)
}

fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    match args.command.as_str() {
        "serve" => serve(args),
        "gen-key" => {
            println!("{}", Cipher::generate_key());
            Ok(())
        }
        "hash-password" => {
            let pw = read_secret("Kata sandi admin baru: ")?;
            if pw.len() < 10 {
                bail!("kata sandi minimal 10 karakter");
            }
            println!("{}", crypto::hash_password(&pw)?);
            Ok(())
        }
        "encrypt" => {
            let config = Config::load(&args.config)?;
            let cipher = Cipher::from_base64(&config.secret_key)?;
            println!("{}", cipher.encrypt(&read_secret("Kata sandi router: ")?));
            Ok(())
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => bail!("perintah tidak dikenal: {other}\n\n{USAGE}"),
    }
}

fn serve(args: Args) -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let mut config = Config::load(&args.config)?;
    if let Some(port) = args.port {
        config.port = port;
    }
    // Runtime kecil: 2 worker cukup untuk satu admin dan menjaga RAM tetap rendah.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .thread_stack_size(256 * 1024)
        .enable_all()
        .build()?;
    runtime.block_on(run(config))
}

async fn run(config: Config) -> anyhow::Result<()> {
    let cipher = Cipher::from_base64(&config.secret_key)?;
    let router_password = cipher.decrypt(&config.router.password_encrypted).context(
        "router.password_encrypted tidak bisa dibuka; buat ulang dengan `mikrotik-panel encrypt`",
    )?;
    let store = store::Store::open(&config.database).context("gagal membuka database")?;
    let router = Arc::new(routeros::RouterOs::new(routeros::RouterTarget {
        host: config.router.host.clone(),
        port: config.router.port,
        user: config.router.user.clone(),
        password: router_password,
    }));
    let addr = SocketAddr::new(config.bind, config.port);
    let state = Arc::new(http::AppState {
        sessions: Sessions::new(Duration::from_secs(config.session_hours * 3600)),
        limiter: LoginLimiter::new(5, Duration::from_secs(15 * 60)),
        store,
        hub: events::Hub::default(),
        router,
        config,
    });
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("gagal bind {addr}"))?;
    tracing::info!("panel berjalan di http://{addr}");
    axum::serve(
        listener,
        http::app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
