//! Encrypted password vault.
//!
//! Passwords are kept in `passwords.vault`, encrypted with ChaCha20-Poly1305 under a random
//! 256-bit key. On Windows that key is itself protected with DPAPI (the same OS facility
//! Chrome uses), so it can only be unlocked by your Windows account. On other platforms the
//! key file is stored with owner-only permissions.

use crate::storage::{now, write_atomic};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Credential {
    pub id: u64,
    /// `scheme://host[:port]`, e.g. `https://accounts.google.com`
    pub origin: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub last_used: u64,
}

pub struct Vault {
    path: PathBuf,
    cipher: ChaCha20Poly1305,
    creds: Vec<Credential>,
    next_id: u64,
}

pub enum Upsert {
    Unchanged,
    Added,
    Updated,
}

/// Normalise any URL to the origin we index credentials by.
pub fn origin_of(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    match u.scheme() {
        "http" | "https" => {}
        _ => return None,
    }
    Some(u.origin().ascii_serialization())
}

/// Matching key for an origin: identical scheme/port, host with a leading `www.` dropped.
/// We deliberately do NOT match across subdomains (no public-suffix list = unsafe).
fn site_key(origin: &str) -> String {
    origin.replacen("://www.", "://", 1)
}

impl Vault {
    pub fn open(dir: &Path) -> Result<Self, String> {
        let key = load_or_create_key(&dir.join("vault.key"))?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
        let path = dir.join("passwords.vault");
        let creds: Vec<Credential> = match std::fs::read(&path) {
            Ok(blob) if blob.len() > 12 => {
                let (nonce, ct) = blob.split_at(12);
                let plain = cipher.decrypt(Nonce::from_slice(nonce), ct).map_err(|_| {
                    "password vault could not be decrypted (wrong key or corrupt file)"
                })?;
                serde_json::from_slice(&plain).map_err(|e| e.to_string())?
            }
            _ => Vec::new(),
        };
        let next_id = creds.iter().map(|c| c.id).max().unwrap_or(0) + 1;
        Ok(Self {
            path,
            cipher,
            creds,
            next_id,
        })
    }

    fn save(&self) {
        let plain = match serde_json::to_vec(&self.creds) {
            Ok(p) => p,
            Err(_) => return,
        };
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        let Ok(ct) = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), plain.as_ref())
        else {
            return;
        };
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&ct);
        if let Err(e) = write_atomic(&self.path, &blob) {
            eprintln!("rustium: failed to save password vault: {e}");
        }
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.creds.len()
    }

    pub fn all(&self) -> &[Credential] {
        &self.creds
    }

    pub fn get(&self, id: u64) -> Option<&Credential> {
        self.creds.iter().find(|c| c.id == id)
    }

    pub fn for_origin(&self, origin: &str) -> Vec<&Credential> {
        let key = site_key(origin);
        self.creds
            .iter()
            .filter(|c| site_key(&c.origin) == key)
            .collect()
    }

    pub fn mark_used(&mut self, origin: &str) {
        let key = site_key(origin);
        let t = now();
        for c in self.creds.iter_mut().filter(|c| site_key(&c.origin) == key) {
            c.last_used = t;
        }
    }

    /// What saving this login would do, without saving it.
    pub fn classify(&self, origin: &str, username: &str, password: &str) -> Upsert {
        let key = site_key(origin);
        match self
            .creds
            .iter()
            .find(|c| site_key(&c.origin) == key && c.username == username)
        {
            Some(c) if c.password == password => Upsert::Unchanged,
            Some(_) => Upsert::Updated,
            None => Upsert::Added,
        }
    }

    pub fn upsert(&mut self, origin: &str, username: &str, password: &str) -> Upsert {
        let key = site_key(origin);
        let t = now();
        let result = if let Some(c) = self
            .creds
            .iter_mut()
            .find(|c| site_key(&c.origin) == key && c.username == username)
        {
            if c.password == password {
                return Upsert::Unchanged;
            }
            c.password = password.to_string();
            c.last_used = t;
            Upsert::Updated
        } else {
            self.creds.push(Credential {
                id: self.next_id,
                origin: origin.to_string(),
                username: username.to_string(),
                password: password.to_string(),
                note: String::new(),
                created: t,
                last_used: t,
            });
            self.next_id += 1;
            Upsert::Added
        };
        self.save();
        result
    }

    pub fn delete(&mut self, id: u64) {
        self.creds.retain(|c| c.id != id);
        self.save();
    }

    /// Import a password CSV exported from Chrome (also accepts Edge, Brave, Firefox,
    /// Bitwarden/1Password-style CSVs that have url/username/password columns).
    /// Returns (added, updated, skipped).
    pub fn import_csv(&mut self, data: &str) -> Result<(usize, usize, usize), String> {
        let mut rdr = csv::ReaderBuilder::new()
            .flexible(true)
            .trim(csv::Trim::Headers)
            .from_reader(data.as_bytes());
        let headers: Vec<String> = rdr
            .headers()
            .map_err(|e| format!("not a valid CSV file: {e}"))?
            .iter()
            .map(|h| h.trim_start_matches('\u{feff}').to_lowercase())
            .collect();
        let col = |names: &[&str]| headers.iter().position(|h| names.contains(&h.as_str()));
        let url_i = col(&["url", "origin", "login_uri", "website", "web site"])
            .ok_or("CSV has no 'url' column — export it from chrome://password-manager/settings")?;
        let user_i = col(&["username", "login", "login_username", "user", "email"]);
        let pass_i = col(&["password", "login_password"]).ok_or("CSV has no 'password' column")?;
        let note_i = col(&["note", "notes", "extra"]);

        let (mut added, mut updated, mut skipped) = (0, 0, 0);
        let t = now();
        for rec in rdr.records() {
            let Ok(rec) = rec else {
                skipped += 1;
                continue;
            };
            let get = |i: Option<usize>| i.and_then(|i| rec.get(i)).unwrap_or("").to_string();
            let password = get(Some(pass_i));
            let Some(origin) = origin_of(&get(Some(url_i))) else {
                skipped += 1; // android:// app entries, blank rows, etc.
                continue;
            };
            if password.is_empty() {
                skipped += 1;
                continue;
            }
            let username = get(user_i);
            let key = site_key(&origin);
            if let Some(c) = self
                .creds
                .iter_mut()
                .find(|c| site_key(&c.origin) == key && c.username == username)
            {
                if c.password != password {
                    c.password = password;
                    updated += 1;
                } else {
                    skipped += 1;
                }
                continue;
            }
            self.creds.push(Credential {
                id: self.next_id,
                origin,
                username,
                password,
                note: get(note_i),
                created: t,
                last_used: 0,
            });
            self.next_id += 1;
            added += 1;
        }
        self.save();
        Ok((added, updated, skipped))
    }

    /// Export in Chrome's CSV format (name,url,username,password,note).
    pub fn export_csv(&self) -> String {
        let mut w = csv::Writer::from_writer(Vec::new());
        let _ = w.write_record(["name", "url", "username", "password", "note"]);
        for c in &self.creds {
            let name = url::Url::parse(&c.origin)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_default();
            let _ = w.write_record([&name, &c.origin, &c.username, &c.password, &c.note]);
        }
        String::from_utf8(w.into_inner().unwrap_or_default()).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------------------
// Key storage
// ---------------------------------------------------------------------------------------

fn load_or_create_key(path: &Path) -> Result<[u8; 32], String> {
    if let Ok(stored) = std::fs::read(path) {
        let raw = unprotect(&stored)?;
        return raw
            .try_into()
            .map_err(|_| "vault key file is corrupt".to_string());
    }
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    let stored = protect(&key)?;
    write_atomic(path, &stored).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(key)
}

#[cfg(windows)]
fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
    dpapi(data, true)
}
#[cfg(windows)]
fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    dpapi(data, false)
}

#[cfg(windows)]
fn dpapi(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        let res = if encrypt {
            CryptProtectData(
                &input,
                windows::core::w!("Rustium vault key"),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        } else {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        res.map_err(|e| format!("Windows DPAPI failed: {e}"))?;
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as _)));
        Ok(bytes)
    }
}

#[cfg(not(windows))]
fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
    Ok(data.to_vec())
}
#[cfg(not(windows))]
fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    Ok(data.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rustium-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn roundtrip_and_chrome_csv_import() {
        let dir = tmpdir("vault");
        let mut v = Vault::open(&dir).unwrap();
        let csv = "name,url,username,password,note\n\
                   github.com,https://github.com/login,octo,hunter2,\n\
                   app,android://abc@com.example/,x,y,\n\
                   ex,https://www.example.com/a?b=c,me@x.com,\"p,w\"\"q\",hi\n";
        let (a, u, s) = v.import_csv(csv).unwrap();
        assert_eq!((a, u, s), (2, 0, 1));
        drop(v);

        let v = Vault::open(&dir).unwrap();
        assert_eq!(v.len(), 2);
        let gh = v.for_origin("https://github.com");
        assert_eq!(gh[0].password, "hunter2");
        // www. is normalised, path/query ignored, quoting preserved
        let ex = v.for_origin("https://example.com");
        assert_eq!(ex[0].password, "p,w\"q");
        // never match a different host or scheme
        assert!(v.for_origin("https://evil-github.com").is_empty());
        assert!(v.for_origin("http://github.com").is_empty());
        // ciphertext must not contain the plaintext password
        let raw = std::fs::read(dir.join("passwords.vault")).unwrap();
        assert!(!raw.windows(7).any(|w| w == b"hunter2"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_updates_existing() {
        let dir = tmpdir("upsert");
        let mut v = Vault::open(&dir).unwrap();
        assert!(matches!(v.upsert("https://a.com", "u", "1"), Upsert::Added));
        assert!(matches!(
            v.upsert("https://a.com", "u", "1"),
            Upsert::Unchanged
        ));
        assert!(matches!(
            v.upsert("https://www.a.com", "u", "2"),
            Upsert::Updated
        ));
        assert_eq!(v.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
