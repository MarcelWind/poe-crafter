//! GitHub release checks and the Windows self-replacement helper.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use semver::Version;
use serde::Deserialize;
use thiserror::Error;

pub const REPO: &str = "MarcelWind/poe-crafter";
pub const ASSET_NAME: &str = "poe-crafter-windows-x64.zip";
pub const ALLOWED_FILES: &[&str] = &["poe-ui.exe", "poe.exe"];
const API_URL: &str = "https://api.github.com/repos";
const USER_AGENT: &str = "poe-crafter-updater/1";
const UPDATE_DIR_ENV: &str = "POE_CRAFTER_UPDATE_DIR";
const MAX_STAGED_BYTES: u64 = 300 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("GitHub request failed: {0}")]
    Http(String),
    #[error("GitHub response was invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("release tag `{0}` is not a valid version")]
    Version(String),
    #[error("release has no {ASSET_NAME} asset")]
    AssetMissing,
    #[error("reading update archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("updating {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("update archive is missing poe-ui.exe")]
    MissingUi,
    #[error("update archive entry `{0}` is too large")]
    TooLarge(String),
    #[error("could not start the update helper: {0}")]
    Spawn(String),
    #[error("update helper failed: {0}")]
    Apply(String),
}

impl UpdateError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub version: Version,
    pub tag: String,
    pub name: String,
    pub body: String,
    pub asset_name: String,
    pub asset_url: String,
    pub asset_size: u64,
}

#[derive(Debug, Clone)]
pub struct UpdateProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

pub struct StagedUpdate {
    pub staging_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

pub fn check_for_update(current_version: &str) -> Result<Option<ReleaseInfo>, UpdateError> {
    let current = Version::parse(current_version)
        .map_err(|_| UpdateError::Version(current_version.to_string()))?;
    let url = format!("{API_URL}/{REPO}/releases/latest");
    let response = match ureq::get(&url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(10))
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(error) => return Err(UpdateError::Http(error.to_string())),
    };
    let release: GithubRelease = serde_json::from_reader(response.into_reader())?;
    if release.prerelease {
        return Ok(None);
    }
    let version = release
        .tag_name
        .trim_start_matches('v')
        .parse::<Version>()
        .map_err(|_| UpdateError::Version(release.tag_name.clone()))?;
    if version <= current {
        return Ok(None);
    }
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name.eq_ignore_ascii_case(ASSET_NAME))
        .or_else(|| {
            release.assets.iter().find(|asset| {
                asset
                    .name
                    .to_ascii_lowercase()
                    .ends_with("-windows-x64.zip")
            })
        })
        .ok_or(UpdateError::AssetMissing)?;
    Ok(Some(ReleaseInfo {
        version,
        tag: release.tag_name,
        name: release.name.unwrap_or_default(),
        body: release.body.unwrap_or_default(),
        asset_name: asset.name.clone(),
        asset_url: asset.browser_download_url.clone(),
        asset_size: asset.size,
    }))
}

pub fn download_and_stage(
    release: &ReleaseInfo,
    staging_dir: &Path,
    progress: Option<&mut dyn FnMut(UpdateProgress)>,
) -> Result<StagedUpdate, UpdateError> {
    if staging_dir.exists() {
        fs::remove_dir_all(staging_dir).map_err(|error| UpdateError::io(staging_dir, error))?;
    }
    fs::create_dir_all(staging_dir).map_err(|error| UpdateError::io(staging_dir, error))?;
    let archive_path = staging_dir.join("update.zip");
    download(
        &release.asset_url,
        &archive_path,
        release.asset_size,
        progress,
    )?;
    let result = stage_archive(&archive_path, staging_dir);
    let _ = fs::remove_file(&archive_path);
    result?;
    Ok(StagedUpdate {
        staging_dir: staging_dir.to_path_buf(),
    })
}

fn download(
    url: &str,
    path: &Path,
    expected: u64,
    mut progress: Option<&mut dyn FnMut(UpdateProgress)>,
) -> Result<(), UpdateError> {
    let response = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .timeout(std::time::Duration::from_secs(120))
        .call()
        .map_err(|error| UpdateError::Http(error.to_string()))?;
    let total = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .or((expected > 0).then_some(expected));
    let mut reader = response.into_reader();
    let mut file = File::create(path).map_err(|error| UpdateError::io(path, error))?;
    let mut buffer = [0u8; 64 * 1024];
    let mut downloaded = 0u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| UpdateError::io(path, error))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|error| UpdateError::io(path, error))?;
        downloaded += read as u64;
        if let Some(callback) = progress.as_mut() {
            callback(UpdateProgress { downloaded, total });
        }
    }
    file.flush().map_err(|error| UpdateError::io(path, error))?;
    Ok(())
}

fn stage_archive(archive_path: &Path, staging_dir: &Path) -> Result<(), UpdateError> {
    let file = File::open(archive_path).map_err(|error| UpdateError::io(archive_path, error))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))?;
    let mut found_ui = false;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = Path::new(entry.name())
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string);
        let Some(name) = name else {
            continue;
        };
        if !ALLOWED_FILES
            .iter()
            .any(|allowed| name.eq_ignore_ascii_case(allowed))
        {
            continue;
        }
        if entry.size() > MAX_STAGED_BYTES {
            return Err(UpdateError::TooLarge(name));
        }
        let destination = staging_dir.join(&name);
        let mut output =
            File::create(&destination).map_err(|error| UpdateError::io(&destination, error))?;
        io::copy(&mut entry, &mut output).map_err(|error| UpdateError::io(&destination, error))?;
        output
            .flush()
            .map_err(|error| UpdateError::io(&destination, error))?;
        if name.eq_ignore_ascii_case("poe-ui.exe") {
            found_ui = true;
        }
    }
    if !found_ui {
        return Err(UpdateError::MissingUi);
    }
    Ok(())
}

/// Spawn the newly downloaded binary in helper mode and let the caller exit.
///
/// `working_dir` is preserved across the restart so relative config and plan
/// paths stay in the same folder the user launched the app from.
pub fn apply_and_restart(
    staged: &StagedUpdate,
    target_dir: &Path,
    working_dir: Option<&Path>,
) -> Result<(), UpdateError> {
    let helper = staged.staging_dir.join("poe-ui.exe");
    if !helper.exists() {
        return Err(UpdateError::MissingUi);
    }
    let mut command = std::process::Command::new(&helper);
    command
        .arg("--apply-update")
        .arg(std::process::id().to_string())
        .arg(target_dir)
        .arg(&staged.staging_dir)
        .current_dir(target_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(working_dir) = working_dir {
        command.arg(working_dir);
    }
    command
        .spawn()
        .map_err(|error| UpdateError::Spawn(error.to_string()))?;
    Ok(())
}

/// Handle the hidden updater mode before starting the GUI.
///
/// Returns `Some(exit_code)` when this process was launched as the helper.
pub fn apply_from_args<I>(args: I) -> Option<i32>
where
    I: IntoIterator<Item = OsString>,
{
    let args: Vec<OsString> = args.into_iter().collect();
    if args.get(1).and_then(|arg| arg.to_str()) != Some("--apply-update") {
        return None;
    }
    let pid = args.get(2)?.to_str()?.parse::<u32>().ok()?;
    let target = PathBuf::from(args.get(3)?);
    let staging = PathBuf::from(args.get(4)?);
    let working_dir = args
        .get(5)
        .map(PathBuf::from)
        .unwrap_or_else(|| target.clone());
    Some(match apply_update(pid, &target, &staging, &working_dir) {
        Ok(()) => 0,
        Err(error) => {
            let _ = fs::write(target.join("update-error.log"), format!("{error}\n"));
            1
        }
    })
}

fn apply_update(
    pid: u32,
    target_dir: &Path,
    staging_dir: &Path,
    working_dir: &Path,
) -> Result<(), UpdateError> {
    wait_for_process(pid);
    for name in ALLOWED_FILES {
        let source = staging_dir.join(name);
        if source.exists() {
            install_file(&source, &target_dir.join(name))?;
        }
    }
    let ui = target_dir.join("poe-ui.exe");
    if !ui.exists() {
        return Err(UpdateError::MissingUi);
    }
    std::process::Command::new(&ui)
        .current_dir(working_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| UpdateError::Spawn(error.to_string()))?;
    Ok(())
}

fn install_file(source: &Path, target: &Path) -> Result<(), UpdateError> {
    let temporary = target.with_file_name(format!(
        "{}.new",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("poe-update")
    ));
    fs::copy(source, &temporary).map_err(|error| UpdateError::io(&temporary, error))?;
    for _ in 0..40 {
        match fs::remove_file(target) {
            Ok(()) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
    fs::rename(&temporary, target).map_err(|error| UpdateError::io(target, error))
}

#[cfg(windows)]
fn wait_for_process(pid: u32) {
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::processthreadsapi::OpenProcess;
    use winapi::um::synchapi::WaitForSingleObject;
    use winapi::um::winnt::SYNCHRONIZE;

    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
    if !handle.is_null() {
        unsafe {
            WaitForSingleObject(handle, 60_000);
            CloseHandle(handle);
        }
    }
}

#[cfg(not(windows))]
fn wait_for_process(_pid: u32) {}

/// The UI creates its staging directory outside the install folder.
pub fn staging_dir(version: &Version) -> PathBuf {
    if let Ok(path) = std::env::var(UPDATE_DIR_ENV) {
        return PathBuf::from(path);
    }
    std::env::temp_dir().join(format!("poe-crafter-update-{}", version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_release_asset_by_name() {
        let release: GithubRelease = serde_json::from_str(
            r#"{
                "tag_name": "v0.2.0",
                "name": "0.2.0",
                "body": "notes",
                "assets": [
                    {"name": "other.zip", "browser_download_url": "https://example/other.zip"},
                    {"name": "poe-crafter-windows-x64.zip", "browser_download_url": "https://example/app.zip", "size": 12}
                ]
            }"#,
        )
        .unwrap();
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name.eq_ignore_ascii_case(ASSET_NAME))
            .unwrap();
        assert_eq!(asset.browser_download_url, "https://example/app.zip");
    }

    #[test]
    fn versions_compare() {
        assert!(Version::parse("0.2.0").unwrap() > Version::parse("0.1.0").unwrap());
    }

    #[test]
    fn stages_only_binaries() {
        let dir = std::env::temp_dir().join("poe-crafter-update-stage-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let archive_path = dir.join("update.zip");
        {
            let file = File::create(&archive_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("poe-ui.exe", options).unwrap();
            writer.write_all(b"ui").unwrap();
            writer.start_file("poe.exe", options).unwrap();
            writer.write_all(b"cli").unwrap();
            writer.start_file("config.toml", options).unwrap();
            writer.write_all(b"config").unwrap();
            writer.finish().unwrap();
        }
        let staging = dir.join("stage");
        std::fs::create_dir_all(&staging).unwrap();
        stage_archive(&archive_path, &staging).unwrap();
        assert!(staging.join("poe-ui.exe").exists());
        assert!(staging.join("poe.exe").exists());
        assert!(!staging.join("config.toml").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn install_replaces_target() {
        let dir = std::env::temp_dir().join("poe-crafter-update-install-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.exe");
        let target = dir.join("poe-ui.exe");
        std::fs::write(&source, b"new").unwrap();
        std::fs::write(&target, b"old").unwrap();
        install_file(&source, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(dir);
    }
}
