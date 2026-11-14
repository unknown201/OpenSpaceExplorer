//! Email-verified accounts with private session tokens for remembered devices.
//!
//! Email ownership is verified by expiring OTPs before sign-in or linking an existing player. Remembered
//! device tokens stay private in browser storage; signing in on another machine adds a session for the
//! same player id. Legacy tokens continue working so existing accounts can link an email safely.
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
    #[serde(default)]
    sessions: Vec<String>,
    #[serde(default)]
    email: Option<String>,
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

/// Independent 128-bit credentials from the operating system CSPRNG.
pub fn random_token() -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("random source: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
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
        let token = random_token()?;
        let id = self.store.next_id;
        self.store.next_id += 1;
        let now = now_unix();
        self.store.players.push(Record {
            id,
            display_name: display_name.to_string(),
            token_hash: hash_token(&token),
            sessions: Vec::new(),
            email: None,
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
        let Some(rec) = self
            .store
            .players
            .iter_mut()
            .find(|r| r.token_hash == h || r.sessions.contains(&h))
        else {
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

    pub fn email(&self, player: u64) -> Option<String> {
        self.store
            .players
            .iter()
            .find(|r| r.id == player)
            .and_then(|r| r.email.clone())
    }

    /// Attach only after ownership of the mailbox has been verified. Never merge two existing accounts.
    pub fn link_email(&mut self, player: u64, email: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self
                .store
                .players
                .iter()
                .any(|r| r.id != player && r.email.as_deref() == Some(email)),
            "this email is already linked to another account; sign out to use that account"
        );
        let rec = self
            .store
            .players
            .iter_mut()
            .find(|r| r.id == player)
            .context("account unavailable")?;
        rec.email = Some(email.into());
        self.flush()
    }

    /// A verified mailbox signs in to its existing account, or creates a new one. Other machines stay signed in.
    pub fn sign_in_email(&mut self, email: &str, name: &str) -> anyhow::Result<(Player, String)> {
        if let Some(rec) = self
            .store
            .players
            .iter_mut()
            .find(|r| r.email.as_deref() == Some(email))
        {
            let token = random_token()?;
            rec.sessions.push(hash_token(&token));
            if rec.sessions.len() > 32 {
                rec.sessions.remove(0);
            }
            rec.last_seen_unix = now_unix();
            let player = Player {
                id: rec.id,
                display_name: rec.display_name.clone(),
            };
            self.flush()?;
            return Ok((player, token));
        }
        let (player, token) = self.create(name)?;
        self.link_email(player.id, email)?;
        Ok((player, token))
    }

    /// Players not seen for `ttl_s` seconds whose id is not in `keep` (campaign members are never swept).
    pub fn stale(&self, ttl_s: u64, keep: &[u64]) -> Vec<u64> {
        let now = now_unix();
        self.store
            .players
            .iter()
            .filter(|r| {
                r.email.is_none()
                    && !keep.contains(&r.id)
                    && now.saturating_sub(r.last_seen_unix) >= ttl_s
            })
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
    fn linking_email_preserves_identity_and_cannot_merge_accounts() {
        let mut a = Accounts::open(Path::new(":memory:")).unwrap();
        let (legacy, token) = a.create("Pilot").unwrap();
        let (other, _) = a.create("Other").unwrap();
        a.link_email(legacy.id, "pilot@example.com").unwrap();
        assert!(a.link_email(other.id, "pilot@example.com").is_err());
        let (signed_in, new_token) = a.sign_in_email("pilot@example.com", "New machine").unwrap();
        assert_eq!(signed_in.id, legacy.id);
        assert_ne!(token, new_token);
        assert_eq!(a.resume(&token, "Pilot").unwrap().unwrap().id, legacy.id);
        assert_eq!(
            a.resume(&new_token, "Pilot").unwrap().unwrap().id,
            legacy.id
        );
        assert!(
            !a.stale(0, &[]).contains(&legacy.id),
            "verified accounts are never swept as guests"
        );
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
