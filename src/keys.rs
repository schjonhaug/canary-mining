use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use rand::rngs::OsRng;
use secp256k1::SecretKey;
use serde::{Deserialize, Serialize};
use stratum_apps::key_utils::{Secp256k1PublicKey, Secp256k1SecretKey};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StoredAuthorityKeys {
    authority_public_key: Secp256k1PublicKey,
    authority_secret_key: Secp256k1SecretKey,
}

#[derive(Copy, Clone, Debug)]
pub struct AuthorityKeys {
    pub public_key: Secp256k1PublicKey,
    pub secret_key: Secp256k1SecretKey,
}

#[derive(Debug, Error)]
pub enum KeyError {
    #[error("failed to read authority keys {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse authority keys {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("failed to encode authority keys: {0}")]
    Encode(toml::ser::Error),
    #[error("failed to create data directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write authority keys {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl AuthorityKeys {
    pub fn load_or_create(data_dir: &Path) -> Result<Self, KeyError> {
        let path = authority_key_path(data_dir);

        match fs::read_to_string(&path) {
            Ok(raw) => {
                let stored = toml::from_str::<StoredAuthorityKeys>(&raw).map_err(|source| {
                    KeyError::Parse {
                        path: path.clone(),
                        source,
                    }
                })?;
                Ok(Self {
                    public_key: stored.authority_public_key,
                    secret_key: stored.authority_secret_key,
                })
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let keys = Self::generate();
                keys.persist(data_dir)?;
                Ok(keys)
            }
            Err(source) => Err(KeyError::Read { path, source }),
        }
    }

    pub fn generate() -> Self {
        let secret_key = SecretKey::new(&mut OsRng);
        let secret_key = Secp256k1SecretKey(secret_key);
        let public_key = Secp256k1PublicKey::from(secret_key);
        Self {
            public_key,
            secret_key,
        }
    }

    fn persist(&self, data_dir: &Path) -> Result<(), KeyError> {
        fs::create_dir_all(data_dir).map_err(|source| KeyError::CreateDir {
            path: data_dir.to_owned(),
            source,
        })?;

        let path = authority_key_path(data_dir);
        let stored = StoredAuthorityKeys {
            authority_public_key: self.public_key,
            authority_secret_key: self.secret_key,
        };
        let raw = toml::to_string_pretty(&stored).map_err(KeyError::Encode)?;
        fs::write(&path, raw).map_err(|source| KeyError::Write { path, source })
    }
}

pub fn authority_key_path(data_dir: &Path) -> PathBuf {
    data_dir.join("authority-keys.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_reuses_authority_keys() {
        let temp = tempfile::tempdir().unwrap();
        let first = AuthorityKeys::load_or_create(temp.path()).unwrap();
        let second = AuthorityKeys::load_or_create(temp.path()).unwrap();

        assert_eq!(first.public_key.to_string(), second.public_key.to_string());
        assert_eq!(first.secret_key.to_string(), second.secret_key.to_string());
        assert!(authority_key_path(temp.path()).exists());
    }
}
