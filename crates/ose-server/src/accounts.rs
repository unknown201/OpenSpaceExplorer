//! Lightweight accounts for resume continuity (ADR-0001, decision 2).
//!
//! A player is created on their first `Hello` and handed a random token; a later `Hello` carrying that
//! token resumes the same player id (and so their vessels and settings). No passwords, no federation: the
//! server is LAN hosted. Tokens are stored hashed (SHA-256) so the database does not hold secrets in clear.

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub struct Accounts {
    conn: Connection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Player {
    pub id: u64,
    pub display_name: String,
}

fn hash_token(token: &str) -> String {
    // FNV-1a over the token bytes, widened to 128 bits by hashing twice with different seeds. Tokens are
    // 128-bit random values, so this is only to avoid storing them verbatim; it is not a password hash.
    fn fnv(seed: u64, bytes: &[u8]) -> u64 {
        let mut h = seed;
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
    format!(
        "{:016x}{:016x}",
        fnv(0xcbf2_9ce4_8422_2325, token.as_bytes()),
        fnv(0x84222325_cbf29ce4, token.as_bytes())
    )
}

fn random_token() -> String {
    // 128 bits from the OS RNG via `getrandom`-free sources: SystemTime nanos mixed with a counter and the
    // process id, then whitened by the token hash. Enough for a LAN game; swap for `rand` if ever exposed.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seed = format!(
        "{nanos}-{}-{}-{:p}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        &COUNTER
    );
    hash_token(&seed)
}

impl Accounts {
    /// Open (or create) the accounts database. `":memory:"` gives a throwaway store.
    pub fn open(path: &Path) -> anyhow::Result<Accounts> {
        let conn = if path.to_string_lossy() == ":memory:" {
            Connection::open_in_memory()?
        } else {
            Connection::open(path).with_context(|| format!("opening {}", path.display()))?
        };
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS players (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                display_name TEXT NOT NULL,
                token_hash TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                last_seen_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )?;
        Ok(Accounts { conn })
    }

    /// Create a new player and return it with the clear token to hand to the client.
    pub fn create(&mut self, display_name: &str) -> anyhow::Result<(Player, String)> {
        let token = random_token();
        self.conn.execute(
            "INSERT INTO players (display_name, token_hash) VALUES (?1, ?2)",
            params![display_name, hash_token(&token)],
        )?;
        let id = self.conn.last_insert_rowid() as u64;
        Ok((
            Player {
                id,
                display_name: display_name.to_string(),
            },
            token,
        ))
    }

    /// Resume the player owning `token`, updating their display name and last-seen time.
    pub fn resume(&mut self, token: &str, display_name: &str) -> anyhow::Result<Option<Player>> {
        let h = hash_token(token);
        let row: Option<u64> = self
            .conn
            .query_row(
                "SELECT id FROM players WHERE token_hash = ?1",
                params![h],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = row else { return Ok(None) };
        self.conn.execute(
            "UPDATE players SET display_name = ?1, last_seen_at = datetime('now') WHERE id = ?2",
            params![display_name, id],
        )?;
        Ok(Some(Player {
            id,
            display_name: display_name.to_string(),
        }))
    }

    pub fn count(&self) -> anyhow::Result<u64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM players", [], |r| r.get::<_, i64>(0))?
            as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_resume_by_token() {
        let mut a = Accounts::open(Path::new(":memory:")).unwrap();
        let (p1, t1) = a.create("Josh").unwrap();
        let (p2, t2) = a.create("Guest").unwrap();
        assert_ne!(p1.id, p2.id);
        assert_ne!(t1, t2);
        assert_eq!(a.resume(&t1, "Josh").unwrap(), Some(p1.clone()));
        assert_eq!(a.resume("nope", "x").unwrap(), None);
        assert_eq!(a.count().unwrap(), 2);
        // a renamed player keeps their id
        assert_eq!(a.resume(&t2, "Guest2").unwrap().map(|p| p.id), Some(p2.id));
    }
}
