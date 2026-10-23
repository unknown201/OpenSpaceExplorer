//! Lightweight accounts for resume continuity (ADR-0001, decision 2).
//!
//! A player is created on their first `Hello` and handed a random token; a later `Hello` carrying that
//! token resumes the same player id (and so their vessels and settings). No passwords, no federation: the
//! server is LAN hosted. Tokens are stored hashed so the file does not hold secrets in clear.
//!
//! Storage is a small JSON file rewritten on every change (a handful of players), chosen over SQLite so
//! the server cross-compiles to static Linux from any machine without a C toolchain.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    pub id: u64,
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    id: u64,
    display_name: String,
    token_hash: String,
    created_unix: u64,
    last_seen_unix: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    next_id: u64,
    players: Vec<Record>,
}

pub struct Accounts {
    path: Option<PathBuf>,
    store: Store,
}

fn hash_token(token: &str) -> String {
    // Two FNV-1a passes with different seeds, concatenated. Tokens are 128-bit random values, so this only
    // avoids storing them verbatim; it is not a password hash.
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
        fnv(0x8422_2325_cbf2_9ce4, token.as_bytes())
    )
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn random_token() -> String {
    // 128 bits from time, process id, a counter and an address, whitened by the token hash. Enough for a
    // LAN game; swap for `rand` if the server is ever exposed.
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
    /// Open (or create) the accounts file. `":memory:"` gives a throwaway store.
    pub fn open(path: &Path) -> anyhow::Result<Accounts> {
        if path.to_string_lossy() == ":memory:" {
            return Ok(Accounts {
                path: None,
                store: Store {
                    next_id: 1,
                    players: Vec::new(),
                },
            });
        }
        let store = if path.exists() {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?
        } else {
            Store {
                next_id: 1,
                players: Vec::new(),
            }
        };
        Ok(Accounts {
            path: Some(path.to_path_buf()),
            store,
        })
    }

    fn flush(&self) -> anyhow::Result<()> {
        let Some(p) = &self.path else { return Ok(()) };
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&self.store)?)?;
        std::fs::rename(&tmp, p)?;
        Ok(())
    }

    /// Create a new player and return it with the clear token to hand to the client.
    pub fn create(&mut self, display_name: &str) -> anyhow::Result<(Player, String)> {
        let token = random_token();
        let id = self.store.next_id;
        self.store.next_id += 1;
        let now = now_unix();
        self.store.players.push(Record {
            id,
            display_name: display_name.to_string(),
            token_hash: hash_token(&token),
            created_unix: now,
            last_seen_unix: now,
        });
        self.flush()?;
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
        let Some(rec) = self.store.players.iter_mut().find(|r| r.token_hash == h) else {
            return Ok(None);
        };
        rec.display_name = display_name.to_string();
        rec.last_seen_unix = now_unix();
        let p = Player {
            id: rec.id,
            display_name: display_name.to_string(),
        };
        self.flush()?;
        Ok(Some(p))
    }

    /// Players not seen for `ttl_s` seconds whose id is not in `keep` (campaign members are never swept).
    pub fn stale(&self, ttl_s: u64, keep: &[u64]) -> Vec<u64> {
        let now = now_unix();
        self.store
            .players
            .iter()
            .filter(|r| !keep.contains(&r.id) && now.saturating_sub(r.last_seen_unix) >= ttl_s)
            .map(|r| r.id)
            .collect()
    }

    /// Mark a player as seen now (on connect and disconnect).
    pub fn touch(&mut self, id: u64) -> anyhow::Result<()> {
        if let Some(r) = self.store.players.iter_mut().find(|r| r.id == id) {
            r.last_seen_unix = now_unix();
            self.flush()?;
        }
        Ok(())
    }

    pub fn remove(&mut self, id: u64) -> anyhow::Result<bool> {
        let before = self.store.players.len();
        self.store.players.retain(|r| r.id != id);
        let removed = self.store.players.len() != before;
        if removed {
            self.flush()?;
        }
        Ok(removed)
    }

    pub fn count(&self) -> anyhow::Result<u64> {
        Ok(self.store.players.len() as u64)
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
        assert_eq!(a.resume(&t2, "Guest2").unwrap().map(|p| p.id), Some(p2.id));
    }

    #[test]
    fn file_store_survives_reopen() {
        let dir = std::env::temp_dir().join(format!("ose-accounts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("accounts.json");
        let (p, t) = Accounts::open(&path).unwrap().create("Josh").unwrap();
        let mut again = Accounts::open(&path).unwrap();
        assert_eq!(again.resume(&t, "Josh").unwrap(), Some(p));
        let (p2, _) = again.create("Second").unwrap();
        assert_eq!(p2.id, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
