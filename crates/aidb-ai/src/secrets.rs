//! Provider keys: environment first, then an optional store outside `app.db`.
//! The model catalog may store a key *name*. Never the secret.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aidb_core::{Error, Result};

const DEFAULT_SERVICE: &str = "aidb";

/// `AIDB_SECRET_STORE=keychain` or `file:/path/outside/the/db`.
pub fn secret_store_uri() -> String {
    std::env::var("AIDB_SECRET_STORE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "env".into())
}

pub fn default_key_name(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "ANTHROPIC_API_KEY",
        "openai" => "OPENAI_API_KEY",
        _ => "AIDB_API_KEY",
    }
}

pub fn validate_key_name(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::usage("key_name is a name, never the secret"));
    }
    if name.len() > 64
        || name.contains('=')
        || name.contains(' ')
        || name.to_ascii_lowercase().starts_with("sk-")
        || !name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(Error::usage("key_name is a name, never the secret"));
    }
    Ok(())
}

pub fn resolve_secret(name: &str) -> Result<String> {
    validate_key_name(name)?;
    if let Some(value) = env_value(name) {
        return Ok(value);
    }
    match configured_store()? {
        Some(store) => match store.get(name)? {
            Some(value) if !value.is_empty() => Ok(value),
            _ => Err(missing(name)),
        },
        None => Err(missing(name)),
    }
}

pub fn resolve_provider_key(provider: &str, key_name: Option<&str>) -> Result<String> {
    let name = match key_name.map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => name,
        None => default_key_name(provider),
    };
    resolve_secret(name)
}

fn missing(name: &str) -> Error {
    Error::ai(format!("{name} is not set"))
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn configured_store() -> Result<Option<SecretStore>> {
    match std::env::var("AIDB_SECRET_STORE") {
        Ok(spec) if !spec.trim().is_empty() => Ok(Some(SecretStore::parse(spec.trim())?)),
        _ => Ok(None),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretStore {
    File(PathBuf),
    Keychain { service: String },
}

impl SecretStore {
    pub fn parse(spec: &str) -> Result<Self> {
        let spec = spec.trim();
        if spec.eq_ignore_ascii_case("keychain") {
            return Ok(Self::Keychain {
                service: DEFAULT_SERVICE.into(),
            });
        }
        if let Some(service) = spec.strip_prefix("keychain:") {
            let service = service.trim();
            if service.is_empty() {
                return Err(Error::usage(
                    "AIDB_SECRET_STORE keychain: requires a service name",
                ));
            }
            return Ok(Self::Keychain {
                service: service.to_string(),
            });
        }
        if let Some(path) = spec.strip_prefix("file:") {
            let path = strip_file_path(path);
            if path.as_os_str().is_empty() {
                return Err(Error::usage(
                    "AIDB_SECRET_STORE file: requires a path outside the db",
                ));
            }
            return Ok(Self::File(path));
        }
        Err(Error::usage(
            "AIDB_SECRET_STORE must be keychain, keychain:<service>, or file:<path>",
        ))
    }

    pub fn get(&self, name: &str) -> Result<Option<String>> {
        match self {
            Self::File(path) => file_get(path, name),
            Self::Keychain { service } => keychain_get(service, name),
        }
    }
}

fn strip_file_path(raw: &str) -> PathBuf {
    let raw = raw.trim();
    let path = if let Some(rest) = raw.strip_prefix("//") {
        if rest.starts_with('/') {
            rest
        } else if let Some((_, rest)) = rest.split_once('/') {
            rest
        } else {
            rest
        }
    } else {
        raw
    };
    PathBuf::from(path)
}

fn file_get(path: &Path, name: &str) -> Result<Option<String>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    Ok(parse_env_file(&text).remove(name))
}

fn parse_env_file(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || validate_key_name(key).is_err() {
            continue;
        }
        let value = unquote(value.trim());
        if !value.is_empty() {
            out.insert(key.to_string(), value);
        }
    }
    out
}

fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn keychain_get(service: &str, name: &str) -> Result<Option<String>> {
    match keyring::Entry::new(service, name) {
        Ok(entry) => match entry.get_password() {
            Ok(value) => {
                let value = value.trim().to_string();
                if value.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(value))
                }
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Ok(None),
        },
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        test_env_lock()
    }

    fn set_var(key: &str, value: &str) {
        unsafe { std::env::set_var(key, value) }
    }

    fn remove_var(key: &str) {
        unsafe { std::env::remove_var(key) }
    }

    #[test]
    fn validates_names_and_rejects_secrets() {
        assert!(validate_key_name("OPENAI_API_KEY").is_ok());
        assert!(validate_key_name("prod-openai").is_ok());
        assert!(validate_key_name("sk-live-secret").is_err());
        assert!(validate_key_name("OPENAI_API_KEY=sk").is_err());
        assert!(validate_key_name(
            "too long name that is definitely not a key name because it exceeds sixty four"
        )
        .is_err());
    }

    #[test]
    fn parses_store_specs() {
        assert_eq!(
            SecretStore::parse("keychain").unwrap(),
            SecretStore::Keychain {
                service: "aidb".into()
            }
        );
        assert_eq!(
            SecretStore::parse("keychain:team").unwrap(),
            SecretStore::Keychain {
                service: "team".into()
            }
        );
        assert_eq!(
            SecretStore::parse("file:/tmp/keys.env").unwrap(),
            SecretStore::File(PathBuf::from("/tmp/keys.env"))
        );
        assert_eq!(
            SecretStore::parse("file:///tmp/keys.env").unwrap(),
            SecretStore::File(PathBuf::from("/tmp/keys.env"))
        );
        assert!(SecretStore::parse("vault:prod").is_err());
    }

    #[test]
    fn env_wins_then_file_then_missing() {
        let _guard = env_lock();
        let dir = std::env::temp_dir().join(format!(
            "aidb-secrets-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keys.env");
        std::fs::write(&path, "AIDB_PHASE25_KEY=from-file\n# comment\nOTHER=x\n").unwrap();

        remove_var("AIDB_PHASE25_KEY");
        remove_var("AIDB_SECRET_STORE");
        let err = resolve_secret("AIDB_PHASE25_KEY").unwrap_err();
        assert!(
            err.to_string().contains("AIDB_PHASE25_KEY is not set"),
            "{err}"
        );

        set_var("AIDB_SECRET_STORE", &format!("file:{}", path.display()));
        assert_eq!(resolve_secret("AIDB_PHASE25_KEY").unwrap(), "from-file");
        assert_eq!(secret_store_uri(), format!("file:{}", path.display()));

        set_var("AIDB_PHASE25_KEY", "from-env");
        assert_eq!(resolve_secret("AIDB_PHASE25_KEY").unwrap(), "from-env");

        remove_var("AIDB_PHASE25_KEY");
        remove_var("AIDB_SECRET_STORE");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_store_is_the_same_as_missing_env() {
        let _guard = env_lock();
        remove_var("AIDB_PHASE25_MISSING");
        set_var(
            "AIDB_SECRET_STORE",
            "file:/tmp/aidb-does-not-exist-phase25.env",
        );
        let err = resolve_secret("AIDB_PHASE25_MISSING").unwrap_err();
        assert_eq!(err.to_string(), "AIDB_PHASE25_MISSING is not set");
        remove_var("AIDB_SECRET_STORE");
    }
}
