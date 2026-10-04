//! Releases (#8): versions of the app published on GitHub Releases (ADR
//! 0007). Each carries a manifest naming its installers with their SHA-256
//! and every risky migration the app has brought so far, so an update that
//! skips versions still sees the risky ones in between (ADR 0010).

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{MigrationId, ModuleMigrations};

/// The manifest every Release carries among its files.
pub const MANIFEST_NAME: &str = "mascate-update.json";

const REPOSITORY: &str = "fernandolisboa/mascate";
/// The API answer and the manifest are small; anything bigger is not them.
const MAX_JSON_BYTES: u64 = 1024 * 1024;
const MAX_INSTALLER_BYTES: u64 = 1024 * 1024 * 1024;

/// An app version, `major.minor.patch`. Pre-releases are not published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// Reads `1.2.3`, or a tag `v1.2.3`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.').map(|part| {
            let digits = !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
            digits.then(|| part.parse().ok()).flatten()
        });
        let version = Self {
            major: parts.next()??,
            minor: parts.next()??,
            patch: parts.next()??,
        };
        parts.next().is_none().then_some(version)
    }

    /// The git tag a Release of this version is published under.
    pub fn tag(self) -> String {
        format!("v{self}")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The kind of installer a copy of the app updates itself with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    WindowsInstaller,
    LinuxAppImage,
}

impl Target {
    /// Its key in a manifest's `installers`.
    pub fn key(self) -> &'static str {
        match self {
            Target::WindowsInstaller => "windows-x86_64",
            Target::LinuxAppImage => "linux-x86_64-appimage",
        }
    }
}

/// A published version of the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The changelog, in Markdown.
    pub notes: String,
    /// The Release's page on GitHub.
    pub page: String,
    /// Every risky migration published up to this version.
    pub risky_migrations: Vec<MigrationId>,
    /// The installer for the target the channel was asked about.
    pub installer: Option<Installer>,
}

impl Release {
    /// Whether installing it runs a risky migration on a database that this
    /// app's `modules` keep.
    pub fn is_risky_for(&self, modules: &[ModuleMigrations]) -> bool {
        self.risky_migrations.iter().any(|risky| {
            let known = modules
                .iter()
                .find(|module| module.module == risky.module)
                .map_or(0, ModuleMigrations::latest_version);
            risky.version > known
        })
    }
}

/// One installer file of a Release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installer {
    pub file_name: String,
    pub size: u64,
    url: String,
    sha256: [u8; 32],
}

#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("GitHub asked to wait before checking again")]
    RateLimited,
    #[error("could not reach GitHub: {0}")]
    Network(String),
    #[error("GitHub answered {status} for {url}")]
    Status { status: u16, url: String },
    #[error("release {release} is not valid: {problem}")]
    Invalid { release: String, problem: String },
    #[error("the download of {file} does not match the SHA-256 its release lists")]
    ChecksumMismatch { file: String },
    #[error("the download of {file} has {got} bytes, not the {expected} its release lists")]
    SizeMismatch {
        file: String,
        expected: u64,
        got: u64,
    },
    #[error("could not write {path}: {source}")]
    File { path: PathBuf, source: io::Error },
}

/// Where Releases are read from: the app's repository on GitHub, or a fake
/// server in tests.
pub struct ReleaseChannel {
    agent: ureq::Agent,
    /// The repository's REST API URL.
    api: String,
    /// What every download URL of the repository starts with.
    downloads: String,
    target: Option<Target>,
}

impl ReleaseChannel {
    /// The app's own Releases. `target` picks the installer each Release
    /// reports; `None` reports none.
    pub fn github(user_agent: &str, target: Option<Target>) -> Self {
        Self::new(
            &format!("https://api.github.com/repos/{REPOSITORY}"),
            &format!("https://github.com/{REPOSITORY}/releases/download/"),
            user_agent,
            target,
        )
    }

    /// Releases under the REST API URL `api`, whose files download from
    /// URLs starting with `downloads`. Plain HTTP only when `api` is.
    pub fn new(api: &str, downloads: &str, user_agent: &str, target: Option<Target>) -> Self {
        let agent = ureq::Agent::config_builder()
            .https_only(api.starts_with("https://"))
            .http_status_as_error(false)
            .user_agent(user_agent)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_recv_body(Some(Duration::from_secs(30 * 60)))
            .build()
            .into();
        Self {
            agent,
            api: api.trim_end_matches('/').to_owned(),
            downloads: downloads.to_owned(),
            target,
        }
    }

    /// The newest Release, or `None` before the first one is published.
    pub fn latest(&self) -> Result<Option<Release>, ReleaseError> {
        self.release(&format!("{}/releases/latest", self.api))
    }

    /// The Release of `version`, or `None` when it was not published.
    pub fn by_version(&self, version: Version) -> Result<Option<Release>, ReleaseError> {
        self.release(&format!("{}/releases/tags/{}", self.api, version.tag()))
    }

    /// Downloads `installer` into `folder` under its own name, checking its
    /// size and SHA-256 on the way. A file that fails the check is removed.
    pub fn download(&self, installer: &Installer, folder: &Path) -> Result<PathBuf, ReleaseError> {
        create_folder(folder)?;
        let path = folder.join(&installer.file_name);
        let partial = folder.join(format!(".{}.download", installer.file_name));
        let downloaded = self.download_to(installer, &partial);
        if let Err(error) = downloaded {
            let _ = std::fs::remove_file(&partial);
            return Err(error);
        }
        std::fs::rename(&partial, &path).map_err(|source| {
            let _ = std::fs::remove_file(&partial);
            ReleaseError::File {
                path: path.clone(),
                source,
            }
        })?;
        Ok(path)
    }

    fn download_to(&self, installer: &Installer, partial: &Path) -> Result<(), ReleaseError> {
        let file_error = |source| ReleaseError::File {
            path: partial.to_path_buf(),
            source,
        };
        let response = self.get(&installer.url, "application/octet-stream")?;
        let mut body = response
            .into_body()
            .into_with_config()
            .limit(installer.size.saturating_add(1))
            .reader();
        let mut file = std::fs::File::create(partial).map_err(file_error)?;
        let mut hasher = Sha256::new();
        let mut got = 0u64;
        let mut buffer = vec![0; 64 * 1024];
        loop {
            let read = match body.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                // Past the size the release lists.
                Err(_) if got >= installer.size => {
                    got += 1;
                    break;
                }
                Err(error) => return Err(ReleaseError::Network(error.to_string())),
            };
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read]).map_err(file_error)?;
            got += read as u64;
        }
        file.sync_all().map_err(file_error)?;
        if got != installer.size {
            return Err(ReleaseError::SizeMismatch {
                file: installer.file_name.clone(),
                expected: installer.size,
                got,
            });
        }
        if hasher.finalize().as_slice() != installer.sha256 {
            return Err(ReleaseError::ChecksumMismatch {
                file: installer.file_name.clone(),
            });
        }
        Ok(())
    }

    fn release(&self, url: &str) -> Result<Option<Release>, ReleaseError> {
        let Some(found) = self.json::<GitHubRelease>(url, "application/vnd.github+json")? else {
            return Ok(None);
        };
        let invalid = |problem: String| ReleaseError::Invalid {
            release: found.tag_name.clone(),
            problem,
        };
        let version = Version::parse(&found.tag_name)
            .ok_or_else(|| invalid("its tag is not a version".into()))?;
        let files = format!("{}{}/", self.downloads, found.tag_name);
        let asset = |name: &str| {
            let asset = found
                .assets
                .iter()
                .find(|asset| asset.name == name)
                .ok_or_else(|| invalid(format!("it has no file {name}")))?;
            if asset.browser_download_url != format!("{files}{name}") {
                return Err(invalid(format!(
                    "{name} downloads from outside the app's repository"
                )));
            }
            Ok(asset)
        };

        let manifest_url = &asset(MANIFEST_NAME)?.browser_download_url;
        let manifest = self
            .json::<Manifest>(manifest_url, "application/octet-stream")?
            .ok_or_else(|| invalid(format!("{MANIFEST_NAME} is missing")))?;
        if Version::parse(&manifest.version) != Some(version) {
            return Err(invalid(format!(
                "its manifest is for version {}",
                manifest.version
            )));
        }
        let installer = match self
            .target
            .and_then(|target| manifest.installers.get(target.key()))
        {
            None => None,
            Some(listed) => {
                if !is_plain_file_name(&listed.file) {
                    return Err(invalid(format!("{:?} is not a file name", listed.file)));
                }
                let sha256 = parse_sha256(&listed.sha256)
                    .ok_or_else(|| invalid(format!("{} has no valid SHA-256", listed.file)))?;
                let file = asset(&listed.file)?;
                if file.size > MAX_INSTALLER_BYTES {
                    return Err(invalid(format!("{} is too big", listed.file)));
                }
                Some(Installer {
                    file_name: listed.file.clone(),
                    size: file.size,
                    url: file.browser_download_url.clone(),
                    sha256,
                })
            }
        };
        let mut risky_migrations: Vec<MigrationId> = manifest
            .risky_migrations
            .into_iter()
            .map(|risky| MigrationId {
                module: risky.module,
                version: risky.version,
            })
            .collect();
        risky_migrations.sort();
        Ok(Some(Release {
            version,
            notes: found.body.unwrap_or_default(),
            page: found.html_url,
            risky_migrations,
            installer,
        }))
    }

    /// The JSON at `url`, or `None` when it is not there.
    fn json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        accept: &str,
    ) -> Result<Option<T>, ReleaseError> {
        let response = match self.get(url, accept) {
            Err(ReleaseError::Status { status: 404, .. }) => return Ok(None),
            other => other?,
        };
        let invalid = |problem: String| ReleaseError::Invalid {
            release: url.to_owned(),
            problem,
        };
        let bytes = response
            .into_body()
            .into_with_config()
            .limit(MAX_JSON_BYTES)
            .read_to_vec()
            .map_err(|error| invalid(error.to_string()))?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| invalid(error.to_string()))
    }

    fn get(
        &self,
        url: &str,
        accept: &str,
    ) -> Result<ureq::http::Response<ureq::Body>, ReleaseError> {
        let response = self
            .agent
            .get(url)
            .header("Accept", accept)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .call()
            .map_err(|error| ReleaseError::Network(error.to_string()))?;
        match response.status().as_u16() {
            200 => Ok(response),
            // GitHub's rate limits answer 403 or 429.
            403 | 429 => Err(ReleaseError::RateLimited),
            status => Err(ReleaseError::Status {
                status,
                url: url.to_owned(),
            }),
        }
    }
}

/// A name with no folder in it, safe to join to a folder path.
fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
}

fn parse_sha256(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut bytes = [0; 32];
    for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(bytes)
}

fn create_folder(folder: &Path) -> Result<(), ReleaseError> {
    std::fs::create_dir_all(folder).map_err(|source| ReleaseError::File {
        path: folder.to_path_buf(),
        source,
    })
}

/// The fields read from GitHub's "get a release" answer.
#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// `mascate-update.json`, written by the release workflow.
#[derive(Deserialize)]
struct Manifest {
    version: String,
    risky_migrations: Vec<ManifestMigration>,
    installers: BTreeMap<String, ManifestInstaller>,
}

#[derive(Deserialize)]
struct ManifestMigration {
    module: String,
    version: u32,
}

#[derive(Deserialize)]
struct ManifestInstaller {
    file: String,
    sha256: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_read_from_numbers_and_tags_and_order_by_number() {
        let v = |text| Version::parse(text).unwrap();
        assert_eq!(
            v("v0.10.2"),
            Version {
                major: 0,
                minor: 10,
                patch: 2
            }
        );
        assert!(v("0.9.9") < v("0.10.0"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert_eq!(v("1.2.3").tag(), "v1.2.3");
        for bad in [
            "",
            "1.2",
            "1.2.3.4",
            "v1.2.x",
            "1.2.3-beta",
            "1..3",
            "+1.2.3",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn installer_names_never_leave_their_folder() {
        assert!(is_plain_file_name("Mascate_0.2.0_x64-setup.exe"));
        for bad in [
            "",
            "../evil.exe",
            "a/b.exe",
            r"a\b.exe",
            ".hidden",
            "C:evil.exe",
        ] {
            assert!(!is_plain_file_name(bad), "{bad}");
        }
    }

    #[test]
    fn sha256_reads_only_64_hex_digits() {
        let hex = "00".repeat(31) + "ff";
        assert_eq!(parse_sha256(&hex).unwrap()[31], 0xff);
        assert_eq!(parse_sha256(&hex[2..]), None);
        assert_eq!(parse_sha256(&("zz".to_owned() + &hex[2..])), None);
    }
}
