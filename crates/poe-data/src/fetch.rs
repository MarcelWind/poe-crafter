//! Fetch the pinned Path of Building data snapshot and build the cache.

use std::fs;
use std::io::Read;
use std::path::{Component, Path};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::import::{cache_is_current, from_pob_dir, save_cache, ImportError};

const USER_AGENT: &str = "poe-crafter-data-fetch/1";
const HTTP_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Error)]
pub enum DataError {
    #[error("reading data lock {path}: {source}")]
    LockRead {
        path: String,
        source: std::io::Error,
    },
    #[error("parsing data lock {path}: {source}")]
    LockParse {
        path: String,
        source: serde_json::Error,
    },
    #[error("unsupported data lock schema {0}")]
    LockSchema(u32),
    #[error("data lock has no source URL or commit")]
    LockSource,
    #[error("invalid data path `{0}`")]
    InvalidPath(String),
    #[error("downloading {url}: {message}")]
    Http { url: String, message: String },
    #[error("reading {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("size mismatch for {path}: expected {expected}, got {actual}")]
    SizeMismatch {
        path: String,
        expected: u64,
        actual: u64,
    },
    #[error("sha256 mismatch for {path}: expected {expected}, got {actual}")]
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("importing Path of Building data: {0}")]
    Import(#[from] ImportError),
}

#[derive(Debug, Clone)]
pub struct DataProgress {
    pub completed: usize,
    pub total: usize,
    pub path: String,
    pub bytes: u64,
    pub downloaded: bool,
}

#[derive(Debug, Clone)]
pub struct DataOutcome {
    pub cache_built: bool,
    pub files: usize,
    pub downloaded: usize,
    pub skipped: usize,
}

#[derive(Debug, Deserialize)]
struct LockManifest {
    schema: u32,
    source: LockSource,
    files: Vec<LockedFile>,
}

#[derive(Debug, Deserialize)]
struct LockSource {
    repo: String,
    commit: String,
    commit_date: String,
    raw_base: String,
}

#[derive(Debug, Deserialize)]
struct LockedFile {
    path: String,
    size: u64,
    sha256: String,
}

/// Ensure the MessagePack cache exists. When it does not, download any missing
/// pinned data files, verify them, import them, and build the cache.
///
/// Existing valid data files are reused, so an interrupted download resumes.
pub fn ensure_cache(
    lock_path: &Path,
    data_dir: &Path,
    cache_path: &Path,
    mut progress: Option<&mut dyn FnMut(DataProgress)>,
) -> Result<DataOutcome, DataError> {
    if cache_is_current(cache_path) {
        return Ok(DataOutcome {
            cache_built: false,
            files: 0,
            downloaded: 0,
            skipped: 0,
        });
    }

    let manifest = read_lock(lock_path)?;
    let total = manifest.files.len();
    let mut downloaded = 0usize;
    let mut skipped = 0usize;

    fs::create_dir_all(data_dir).map_err(|source| DataError::Io {
        path: data_dir.display().to_string(),
        source,
    })?;

    for (index, file) in manifest.files.iter().enumerate() {
        if !is_safe_relative(&file.path) {
            return Err(DataError::InvalidPath(file.path.clone()));
        }
        let destination = data_dir.join(&file.path);
        let (bytes, was_downloaded) = if valid_file(&destination, file)? {
            skipped += 1;
            (read_file(&destination)?, false)
        } else {
            let url = format!(
                "{}/{}",
                manifest.source.raw_base.trim_end_matches('/'),
                file.path.trim_start_matches('/')
            );
            let bytes = download(&url, &file.path)?;
            if bytes.len() as u64 != file.size {
                return Err(DataError::SizeMismatch {
                    path: file.path.clone(),
                    expected: file.size,
                    actual: bytes.len() as u64,
                });
            }
            let actual = sha256_hex(&bytes);
            if !actual.eq_ignore_ascii_case(&file.sha256) {
                return Err(DataError::HashMismatch {
                    path: file.path.clone(),
                    expected: file.sha256.clone(),
                    actual,
                });
            }
            write_file(&destination, &bytes)?;
            downloaded += 1;
            (bytes, true)
        };

        if let Some(callback) = progress.as_mut() {
            callback(DataProgress {
                completed: index + 1,
                total,
                path: file.path.clone(),
                bytes: bytes.len() as u64,
                downloaded: was_downloaded,
            });
        }
    }

    let mut data = from_pob_dir(data_dir)?;
    data.version.source = manifest.source.repo;
    data.version.commit = manifest.source.commit;
    data.version.commit_date = manifest.source.commit_date;
    save_cache(&data, cache_path)?;

    Ok(DataOutcome {
        cache_built: true,
        files: total,
        downloaded,
        skipped,
    })
}

fn read_lock(path: &Path) -> Result<LockManifest, DataError> {
    let text = fs::read_to_string(path).map_err(|source| DataError::LockRead {
        path: path.display().to_string(),
        source,
    })?;
    let manifest: LockManifest =
        serde_json::from_str(&text).map_err(|source| DataError::LockParse {
            path: path.display().to_string(),
            source,
        })?;
    if manifest.schema != 1 {
        return Err(DataError::LockSchema(manifest.schema));
    }
    if manifest.source.raw_base.trim().is_empty() || manifest.source.commit.trim().is_empty() {
        return Err(DataError::LockSource);
    }
    Ok(manifest)
}

fn is_safe_relative(path: &str) -> bool {
    let path = Path::new(path);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn valid_file(path: &Path, expected: &LockedFile) -> Result<bool, DataError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(DataError::Io {
                path: path.display().to_string(),
                source,
            })
        }
    };
    if !metadata.is_file() || metadata.len() != expected.size {
        return Ok(false);
    }
    let bytes = read_file(path)?;
    Ok(sha256_hex(&bytes).eq_ignore_ascii_case(&expected.sha256))
}

fn read_file(path: &Path) -> Result<Vec<u8>, DataError> {
    fs::read(path).map_err(|source| DataError::Io {
        path: path.display().to_string(),
        source,
    })
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), DataError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| DataError::Io {
            path: parent.display().to_string(),
            source,
        })?;
    }
    let temp = path.with_file_name(format!(
        ".{}.part",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("data")
    ));
    fs::write(&temp, bytes).map_err(|source| DataError::Io {
        path: temp.display().to_string(),
        source,
    })?;
    if path.exists() {
        fs::remove_file(path).map_err(|source| DataError::Io {
            path: path.display().to_string(),
            source,
        })?;
    }
    fs::rename(&temp, path).map_err(|source| DataError::Io {
        path: path.display().to_string(),
        source,
    })
}

fn download(url: &str, path: &str) -> Result<Vec<u8>, DataError> {
    let response = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(|error| DataError::Http {
            url: url.to_string(),
            message: error.to_string(),
        })?;
    let mut reader = response.into_reader();
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|source| DataError::Io {
            path: path.to_string(),
            source,
        })?;
    Ok(bytes)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_paths() {
        assert!(is_safe_relative("src/Data/Bases/amulet.lua"));
        assert!(!is_safe_relative("../escape.lua"));
        assert!(!is_safe_relative("src/../escape.lua"));
        assert!(!is_safe_relative("C:/absolute.lua"));
        assert!(!is_safe_relative("/absolute.lua"));
    }

    #[test]
    fn parses_committed_lock() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/pob-lock.json");
        let manifest = read_lock(&path).unwrap();
        assert_eq!(manifest.schema, 1);
        assert!(!manifest.files.is_empty());
        assert!(manifest
            .files
            .iter()
            .all(|file| is_safe_relative(&file.path)));
    }

    #[test]
    fn existing_cache_short_circuits() {
        let dir = std::env::temp_dir().join("poe-crafter-fetch-cache-test");
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("data.msgpack");
        let mut header = Vec::new();
        header.extend_from_slice(crate::import::CACHE_MAGIC);
        header.extend_from_slice(&crate::import::CACHE_SCHEMA.to_le_bytes());
        header.push(0);
        std::fs::write(&cache, header).unwrap();
        let outcome = ensure_cache(
            &dir.join("missing-lock.json"),
            &dir.join("data"),
            &cache,
            None,
        )
        .unwrap();
        assert!(!outcome.cache_built);
        assert_eq!(outcome.files, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn verifies_existing_file() {
        let dir = std::env::temp_dir().join("poe-crafter-fetch-test");
        let path = dir.join("file.txt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, b"payload").unwrap();
        let expected = LockedFile {
            path: "file.txt".to_string(),
            size: 7,
            sha256: sha256_hex(b"payload"),
        };
        assert!(valid_file(&path, &expected).unwrap());
        let wrong = LockedFile {
            sha256: "0".repeat(64),
            ..expected
        };
        assert!(!valid_file(&path, &wrong).unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }
}
