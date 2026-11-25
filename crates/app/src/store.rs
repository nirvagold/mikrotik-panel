//! Penyimpanan SQLite. Satu koneksi dibungkus Mutex: cukup untuk satu admin dan hemat RAM.
//! Semua akses dijalankan lewat `spawn_blocking` supaya runtime async tidak tertahan.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

/// Migrasi berurutan; indeks + 1 = nilai `PRAGMA user_version` setelah migrasi itu.
const MIGRATIONS: &[&str] = &[
    // v1: skema awal panel Rust.
    r#"
    CREATE TABLE daily (
        day TEXT NOT NULL, account TEXT NOT NULL,
        download INTEGER NOT NULL DEFAULT 0, upload INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (day, account)
    ) WITHOUT ROWID;
    CREATE TABLE coverage (day TEXT PRIMARY KEY, seconds INTEGER NOT NULL DEFAULT 0) WITHOUT ROWID;
    CREATE TABLE session_counters (
        session_id TEXT PRIMARY KEY, account TEXT NOT NULL,
        download INTEGER NOT NULL, upload INTEGER NOT NULL, sampled_at INTEGER NOT NULL
    ) WITHOUT ROWID;
    CREATE TABLE snapshot (id INTEGER PRIMARY KEY CHECK (id = 1), sampled_at INTEGER NOT NULL, data TEXT NOT NULL);
    CREATE TABLE audit (
        id TEXT PRIMARY KEY, at INTEGER NOT NULL, actor TEXT NOT NULL, account TEXT,
        action TEXT NOT NULL, before TEXT, after TEXT, status TEXT NOT NULL
    );
    CREATE INDEX audit_at ON audit(at);
    CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
    CREATE TABLE login_attempts (ip TEXT NOT NULL, at INTEGER NOT NULL, ok INTEGER NOT NULL);
    CREATE INDEX login_attempts_at ON login_attempts(at);

    CREATE TABLE design_versions (
        id TEXT PRIMARY KEY, parent_id TEXT REFERENCES design_versions(id),
        spec TEXT NOT NULL, custom_css TEXT NOT NULL DEFAULT '' CHECK (length(custom_css) <= 8192),
        source TEXT NOT NULL, status TEXT NOT NULL CHECK (status IN ('draft', 'active', 'archived')),
        created_at INTEGER NOT NULL
    );
    CREATE UNIQUE INDEX design_one_active ON design_versions(status) WHERE status = 'active';
    CREATE TABLE conversations (id TEXT PRIMARY KEY, title TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
    CREATE TABLE chat_messages (
        id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
        role TEXT NOT NULL, content TEXT NOT NULL, tool_calls TEXT,
        design_version_id TEXT REFERENCES design_versions(id), created_at INTEGER NOT NULL
    );
    CREATE INDEX chat_messages_conv ON chat_messages(conversation_id, created_at);

    CREATE TABLE custom_actions (
        id TEXT PRIMARY KEY, label TEXT NOT NULL, template TEXT NOT NULL, params_schema TEXT NOT NULL,
        risk TEXT NOT NULL CHECK (risk IN ('read', 'write', 'critical')), placement TEXT,
        created_at INTEGER NOT NULL, approved_at INTEGER
    );
    CREATE TABLE action_runs (
        id TEXT PRIMARY KEY, action_id TEXT REFERENCES custom_actions(id), params TEXT NOT NULL,
        command TEXT NOT NULL, before_snapshot TEXT, result TEXT, at INTEGER NOT NULL
    );
    CREATE TABLE recommendations (
        id TEXT PRIMARY KEY, kind TEXT NOT NULL, finding TEXT NOT NULL, evidence TEXT,
        status TEXT NOT NULL CHECK (status IN ('open', 'accepted', 'dismissed')),
        action_id TEXT REFERENCES custom_actions(id), created_at INTEGER NOT NULL
    );
    CREATE TABLE router_snapshots (
        id INTEGER PRIMARY KEY, taken_at INTEGER NOT NULL, section TEXT NOT NULL,
        diff TEXT NOT NULL, full_hash TEXT NOT NULL
    );
    CREATE INDEX router_snapshots_section ON router_snapshots(section, taken_at);
    CREATE TABLE agent_memories (
        id TEXT PRIMARY KEY, kind TEXT NOT NULL CHECK (kind IN ('user', 'router', 'outcome')),
        text TEXT NOT NULL, source TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
    );
    CREATE VIRTUAL TABLE agent_memories_fts USING fts5(text, content='agent_memories', content_rowid='rowid');
    CREATE TRIGGER agent_memories_ai AFTER INSERT ON agent_memories BEGIN
        INSERT INTO agent_memories_fts(rowid, text) VALUES (new.rowid, new.text);
    END;
    CREATE TRIGGER agent_memories_ad AFTER DELETE ON agent_memories BEGIN
        INSERT INTO agent_memories_fts(agent_memories_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
    END;
    CREATE TRIGGER agent_memories_au AFTER UPDATE ON agent_memories BEGIN
        INSERT INTO agent_memories_fts(agent_memories_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
        INSERT INTO agent_memories_fts(rowid, text) VALUES (new.rowid, new.text);
    END;
    CREATE VIRTUAL TABLE routeros_reference USING fts5(path, title, body);
    "#,
];

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000; PRAGMA cache_size=-2000; PRAGMA temp_store=FILE;",
        )?;
        migrate(&mut conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Menjalankan `f` dengan koneksi di thread blocking.
    pub async fn run<T, F>(&self, f: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        let out = tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut guard)
        })
        .await??;
        Ok(out)
    }
}

fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let current: usize =
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as usize;
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (index + 1) as i64)?;
        tx.commit()?;
        tracing::info!("migrasi database ke versi {}", index + 1);
    }
    Ok(())
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrates_and_fts_works() {
        let store = Store::open_in_memory().unwrap();
        let n = store
            .run(|c| {
                c.execute(
                    "INSERT INTO agent_memories VALUES ('m1', 'router', 'ether1 adalah uplink ISP', 'tes', 0, 0)",
                    [],
                )?;
                c.query_row("SELECT count(*) FROM agent_memories_fts WHERE agent_memories_fts MATCH 'uplink'", [], |r| {
                    r.get::<_, i64>(0)
                })
            })
            .await
            .unwrap();
        assert_eq!(n, 1);
        let version = store
            .run(|c| c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)))
            .await
            .unwrap();
        assert_eq!(version as usize, MIGRATIONS.len());
    }

    #[tokio::test]
    async fn only_one_active_design() {
        let store = Store::open_in_memory().unwrap();
        let second = store
            .run(|c| {
                c.execute("INSERT INTO design_versions VALUES ('a', NULL, '{}', '', 'default', 'active', 0)", [])?;
                c.execute("INSERT INTO design_versions VALUES ('b', 'a', '{}', '', 'agent', 'active', 1)", [])
            })
            .await;
        assert!(second.is_err());
    }
}
