use reqwest::Url;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE, USER_AGENT};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};
use thiserror::Error;
use uuid::Uuid;

const CACHE_DIRECTORY: &str = "images";
const MAX_CACHE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
const CLIENT_USER_AGENT: &str = "Metagene/0.1 (local MTG deck evolution workbench)";

static ACCESS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();
static CACHE_EPOCH: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum ArtCacheError {
    #[error("card-art cache file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("card-art download error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("card-art URL is invalid: {0}")]
    Url(String),
    #[error("card-art response was not an image")]
    NotImage,
    #[error("card-art image exceeds the 8 MiB limit")]
    TooLarge,
    #[error("card-art cache lock is unavailable")]
    Lock,
}

#[derive(Clone, Debug, Serialize)]
pub struct ArtCacheStatus {
    pub entries: usize,
    pub bytes: u64,
    pub max_bytes: u64,
}

pub fn get(data_dir: &Path, image_uri: &str) -> Result<Vec<u8>, ArtCacheError> {
    validate_uri(image_uri)?;
    let path = image_path(data_dir, image_uri);
    let cache_epoch = CACHE_EPOCH.load(Ordering::Acquire);

    if let Some(bytes) = read_cached(&path)? {
        return Ok(bytes);
    }

    let client = http_client()?;
    let mut response = client
        .get(image_uri)
        .header(USER_AGENT, CLIENT_USER_AGENT)
        .header(ACCEPT, "image/jpeg,image/*;q=0.8")
        .send()?
        .error_for_status()?;

    if response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_none_or(|value| !value.to_ascii_lowercase().starts_with("image/"))
    {
        return Err(ArtCacheError::NotImage);
    }
    if response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > MAX_IMAGE_BYTES)
    {
        return Err(ArtCacheError::TooLarge);
    }

    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() {
        return Err(ArtCacheError::NotImage);
    }
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(ArtCacheError::TooLarge);
    }

    let _guard = access_lock().lock().map_err(|_| ArtCacheError::Lock)?;
    if CACHE_EPOCH.load(Ordering::Acquire) != cache_epoch {
        return Ok(bytes);
    }
    if path.is_file() {
        return read_and_touch(&path).map_err(Into::into);
    }
    let directory = cache_directory(data_dir);
    fs::create_dir_all(&directory)?;
    let pending = directory.join(format!("{}.pending", Uuid::new_v4()));
    if let Err(error) = write_atomically(&pending, &path, &bytes) {
        let _ = fs::remove_file(&pending);
        if path.is_file() {
            return read_and_touch(&path).map_err(Into::into);
        }
        return Err(error.into());
    }
    prune_to_limit(&directory, MAX_CACHE_BYTES)?;
    Ok(bytes)
}

pub fn status(data_dir: &Path) -> Result<ArtCacheStatus, ArtCacheError> {
    let _guard = access_lock().lock().map_err(|_| ArtCacheError::Lock)?;
    let (entries, bytes) = cache_usage(&cache_directory(data_dir))?;
    Ok(ArtCacheStatus {
        entries,
        bytes,
        max_bytes: MAX_CACHE_BYTES,
    })
}

pub fn clear(data_dir: &Path) -> Result<ArtCacheStatus, ArtCacheError> {
    let _guard = access_lock().lock().map_err(|_| ArtCacheError::Lock)?;
    CACHE_EPOCH.fetch_add(1, Ordering::AcqRel);
    let directory = cache_directory(data_dir);
    if directory.is_dir() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(ArtCacheStatus {
        entries: 0,
        bytes: 0,
        max_bytes: MAX_CACHE_BYTES,
    })
}

fn validate_uri(image_uri: &str) -> Result<(), ArtCacheError> {
    let url = Url::parse(image_uri).map_err(|error| ArtCacheError::Url(error.to_string()))?;
    if url.scheme() != "https" || url.host_str() != Some("cards.scryfall.io") {
        return Err(ArtCacheError::Url(
            "only HTTPS cards.scryfall.io URLs are allowed".into(),
        ));
    }
    Ok(())
}

fn image_path(data_dir: &Path, image_uri: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(b"metagene-card-art-v1\0");
    hasher.update(image_uri.as_bytes());
    cache_directory(data_dir).join(format!("{:x}.img", hasher.finalize()))
}

fn cache_directory(data_dir: &Path) -> PathBuf {
    data_dir.join(CACHE_DIRECTORY)
}

fn access_lock() -> &'static Mutex<()> {
    ACCESS_LOCK.get_or_init(|| Mutex::new(()))
}

fn http_client() -> Result<&'static Client, reqwest::Error> {
    if let Some(client) = HTTP_CLIENT.get() {
        return Ok(client);
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let _ = HTTP_CLIENT.set(client);
    Ok(HTTP_CLIENT
        .get()
        .expect("HTTP client was initialized by this process"))
}

fn read_cached(path: &Path) -> Result<Option<Vec<u8>>, ArtCacheError> {
    let _guard = access_lock().lock().map_err(|_| ArtCacheError::Lock)?;
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(read_and_touch(path)?))
}

fn read_and_touch(path: &Path) -> Result<Vec<u8>, std::io::Error> {
    let bytes = fs::read(path)?;
    OpenOptions::new()
        .write(true)
        .open(path)?
        .set_modified(SystemTime::now())?;
    Ok(bytes)
}

fn write_atomically(
    pending: &Path,
    destination: &Path,
    bytes: &[u8],
) -> Result<(), std::io::Error> {
    let mut file = File::create(pending)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(pending, destination)
}

fn cache_usage(directory: &Path) -> Result<(usize, u64), std::io::Error> {
    if !directory.is_dir() {
        return Ok((0, 0));
    }
    let mut entries = 0;
    let mut bytes = 0u64;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                == Some("img")
        {
            entries += 1;
            bytes = bytes.saturating_add(entry.metadata()?.len());
        }
    }
    Ok((entries, bytes))
}

fn prune_to_limit(directory: &Path, limit: u64) -> Result<(), std::io::Error> {
    let mut files = Vec::new();
    let mut total = 0u64;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file()
            || entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("img")
        {
            continue;
        }
        let metadata = entry.metadata()?;
        let size = metadata.len();
        total = total.saturating_add(size);
        files.push((
            metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            entry.path(),
            size,
        ));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    for (_, path, size) in files {
        if total <= limit {
            break;
        }
        fs::remove_file(path)?;
        total = total.saturating_sub(size);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_directory() -> PathBuf {
        std::env::temp_dir().join(format!("metagene-art-cache-{}", Uuid::new_v4()))
    }

    #[test]
    fn keys_are_stable_and_uri_specific() {
        let data_dir = Path::new("cache-root");
        let first = image_path(
            data_dir,
            "https://cards.scryfall.io/normal/front/a/test.jpg",
        );
        assert_eq!(
            first,
            image_path(
                data_dir,
                "https://cards.scryfall.io/normal/front/a/test.jpg"
            )
        );
        assert_ne!(
            first,
            image_path(
                data_dir,
                "https://cards.scryfall.io/normal/front/b/test.jpg"
            )
        );
    }

    #[test]
    fn accepts_only_the_scryfall_image_cdn_over_https() {
        assert!(validate_uri("https://cards.scryfall.io/normal/front/a/test.jpg").is_ok());
        assert!(validate_uri("http://cards.scryfall.io/normal/front/a/test.jpg").is_err());
        assert!(validate_uri("https://example.com/test.jpg").is_err());
    }

    #[test]
    fn prunes_oldest_files_and_clears_cache() {
        let data_dir = temporary_directory();
        let directory = cache_directory(&data_dir);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("a.img"), [1u8; 6]).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(directory.join("b.img"), [2u8; 6]).unwrap();

        prune_to_limit(&directory, 6).unwrap();
        assert!(!directory.join("a.img").exists());
        assert!(directory.join("b.img").exists());
        let current = status(&data_dir).unwrap();
        assert_eq!(current.entries, 1);
        assert_eq!(current.bytes, 6);
        let cleared = clear(&data_dir).unwrap();
        assert_eq!(cleared.entries, 0);
        assert_eq!(cleared.bytes, 0);
        fs::remove_dir_all(data_dir).unwrap();
    }
}
