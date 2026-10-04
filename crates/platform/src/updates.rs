//! Updates (#8, ADR 0007): finding a newer Release, putting it in place with
//! one click or silently on the next start, and going back to the version
//! the app updated from when the new one cannot migrate the database.
//!
//! The Windows installer replaces the app once the app has exited; an
//! AppImage is a single file, swapped for the new one while the app runs.
//! Any other copy (a Linux package, a development build) is updated by the
//! owner. A few small files in the updates folder carry what one version
//! leaves for the next: the version it updated from, a version that failed
//! to migrate, and an installer waiting for the next start.

use std::io;
use std::path::{Path, PathBuf};

use libsql::Value;
use mascate_kernel::{Clock, IdGenerator};

use crate::releases::{Release, ReleaseChannel, ReleaseError, Target, Version};
use crate::{Database, Migration, ModuleMigrations, single_row};

const PREVIOUS: &str = "previous-version";
const FAILED: &str = "failed-version";
const STAGED: &str = "staged";

/// The owner's choices about updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UpdateSettings {
    /// Download newer versions in the background and install them on the
    /// next start, except those with a risky migration. Off by default.
    pub silent: bool,
}

const TABLE: &str = "platform_update_settings";
const COLUMNS: &[&str] = &["silent"];

pub(crate) const CREATE_UPDATE_SETTINGS: Migration = Migration {
    version: 5,
    name: "create update settings",
    risky: false,
    sql: "CREATE TABLE platform_update_settings (
        id         TEXT PRIMARY KEY,
        silent     INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );",
};

/// The saved settings, or the defaults before any was saved.
pub async fn load_update_settings(database: &Database) -> Result<UpdateSettings, libsql::Error> {
    let Some(row) = single_row::load(database.connection(), TABLE, COLUMNS).await? else {
        return Ok(UpdateSettings::default());
    };
    Ok(UpdateSettings {
        silent: row.get::<i64>(0)? != 0,
    })
}

pub async fn save_update_settings(
    database: &Database,
    clock: &dyn Clock,
    ids: &dyn IdGenerator,
    settings: UpdateSettings,
) -> Result<(), libsql::Error> {
    single_row::save(
        database.connection(),
        clock,
        ids,
        TABLE,
        COLUMNS,
        vec![Value::Integer(settings.silent.into())],
    )
    .await
}

/// How the running copy of the app was installed, which decides how it
/// updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installation {
    /// By the Windows installer, which also installs the next version.
    WindowsInstaller,
    /// As the AppImage at this path.
    AppImage(PathBuf),
    /// By a Linux package, or a development build: the owner updates it.
    Manual,
}

impl Installation {
    /// How the running copy was installed.
    pub fn detect() -> Self {
        if cfg!(windows) {
            let installed = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|folder| folder.join("uninstall.exe")))
                .is_some_and(|uninstaller| uninstaller.is_file());
            if installed {
                return Self::WindowsInstaller;
            }
        } else if cfg!(target_os = "linux") {
            // Set by the AppImage runtime to the file the app runs from.
            if let Some(path) = std::env::var_os("APPIMAGE").map(PathBuf::from)
                && path.is_file()
            {
                return Self::AppImage(path);
            }
        }
        Self::Manual
    }

    /// The installer it updates with; `None` when the owner updates it.
    pub fn target(&self) -> Option<Target> {
        match self {
            Self::WindowsInstaller => Some(Target::WindowsInstaller),
            Self::AppImage(_) => Some(Target::LinuxAppImage),
            Self::Manual => None,
        }
    }
}

/// What the app does to finish an install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finish {
    /// Quit, then run this installer, which starts the new version: see
    /// [`run_installer_after_exit`].
    RunInstaller(PathBuf),
    /// Restart from this file, which already holds the new version.
    Restart(PathBuf),
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error(transparent)]
    Release(#[from] ReleaseError),
    #[error("this copy of the app is updated through its package or a new download")]
    Manual,
    #[error("version {0} has no installer for this computer")]
    NoInstaller(Version),
    #[error("no earlier version is recorded to go back to")]
    NothingToGoBackTo,
    #[error("version {0} is not published")]
    NotPublished(Version),
    #[error("could not write {path}: {source}")]
    File { path: PathBuf, source: io::Error },
}

/// Finds and installs newer versions of the running app.
pub struct Updater {
    channel: ReleaseChannel,
    current: Version,
    installation: Installation,
    /// Holds downloaded installers and the files one version leaves for the
    /// next.
    folder: PathBuf,
    /// Every module's migrations, to tell which Releases are risky here.
    modules: &'static [ModuleMigrations],
}

impl Updater {
    pub fn new(
        channel: ReleaseChannel,
        current: Version,
        installation: Installation,
        folder: PathBuf,
        modules: &'static [ModuleMigrations],
    ) -> Self {
        Self {
            channel,
            current,
            installation,
            folder,
            modules,
        }
    }

    pub fn current(&self) -> Version {
        self.current
    }

    pub fn installation(&self) -> &Installation {
        &self.installation
    }

    /// The newest Release, when it is newer than the running app.
    pub fn check(&self) -> Result<Option<Release>, UpdateError> {
        Ok(self
            .channel
            .latest()?
            .filter(|release| release.version > self.current))
    }

    /// Whether installing `release` runs a risky migration here.
    pub fn is_risky(&self, release: &Release) -> bool {
        release.is_risky_for(self.modules)
    }

    /// Whether `release` was installed here before and could not migrate
    /// the database.
    pub fn failed_before(&self, release: &Release) -> bool {
        self.read_version(FAILED) == Some(release.version)
    }

    /// Whether a silent update may install `release`: never one with a
    /// risky migration, nor one that failed before.
    pub fn installs_silently(&self, release: &Release) -> bool {
        self.installation != Installation::Manual
            && release.installer.is_some()
            && !self.is_risky(release)
            && !self.failed_before(release)
    }

    /// Downloads and checks `release`'s installer and puts it in place now;
    /// the app finishes as the result says.
    pub fn install(&self, release: &Release) -> Result<Finish, UpdateError> {
        let finish = self.put_in_place(release)?;
        self.write_version(PREVIOUS, self.current)?;
        Ok(finish)
    }

    /// Like [`Updater::install`], for the next start: the Windows installer
    /// waits in the updates folder for [`Updater::take_staged`]; an AppImage
    /// is swapped now and runs the next time the app starts.
    pub fn install_on_next_start(&self, release: &Release) -> Result<(), UpdateError> {
        let finish = self.put_in_place(release)?;
        self.write_version(PREVIOUS, self.current)?;
        if let Finish::RunInstaller(installer) = finish {
            let name = installer
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            self.write(STAGED, &format!("{}\n{name}\n", release.version))?;
        }
        Ok(())
    }

    /// An installer [`Updater::install_on_next_start`] left for this start,
    /// if it brings a newer version. Each one is offered once, so an
    /// installer that fails is not run on every start.
    pub fn take_staged(&self) -> Option<Finish> {
        let staged = std::fs::read_to_string(self.folder.join(STAGED)).ok()?;
        let _ = std::fs::remove_file(self.folder.join(STAGED));
        let mut lines = staged.lines();
        let version = Version::parse(lines.next()?)?;
        let installer = self.folder.join(lines.next()?);
        (version > self.current && installer.is_file()).then_some(Finish::RunInstaller(installer))
    }

    /// Goes back to the version the app updated from, after this one could
    /// not migrate the database. Records this version as failed, so a
    /// silent update does not bring it back.
    pub fn roll_back(&self) -> Result<Finish, UpdateError> {
        let previous = self
            .read_version(PREVIOUS)
            .filter(|previous| *previous < self.current)
            .ok_or(UpdateError::NothingToGoBackTo)?;
        self.write_version(FAILED, self.current)?;
        let release = self
            .channel
            .by_version(previous)?
            .ok_or(UpdateError::NotPublished(previous))?;
        self.put_in_place(&release)
    }

    /// The version the app updated from, when one is recorded.
    pub fn previous(&self) -> Option<Version> {
        self.read_version(PREVIOUS)
    }

    fn put_in_place(&self, release: &Release) -> Result<Finish, UpdateError> {
        let installer = release
            .installer
            .as_ref()
            .ok_or(UpdateError::NoInstaller(release.version))?;
        match &self.installation {
            Installation::Manual => Err(UpdateError::Manual),
            Installation::WindowsInstaller => {
                self.remove_old_installers();
                let path = self.channel.download(installer, &self.folder)?;
                Ok(Finish::RunInstaller(path))
            }
            Installation::AppImage(current) => {
                // Downloaded beside it, so the swap is one rename on one disk.
                let folder = current.parent().unwrap_or(Path::new("."));
                let downloaded = self.channel.download(installer, folder)?;
                let file_error = |source| UpdateError::File {
                    path: current.clone(),
                    source,
                };
                make_executable(&downloaded).map_err(file_error)?;
                if downloaded != *current {
                    std::fs::rename(&downloaded, current).map_err(|source| {
                        let _ = std::fs::remove_file(&downloaded);
                        file_error(source)
                    })?;
                }
                Ok(Finish::Restart(current.clone()))
            }
        }
    }

    /// Installers of earlier updates; the notes files stay.
    fn remove_old_installers(&self) {
        let Ok(entries) = std::fs::read_dir(&self.folder) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let notes = [PREVIOUS, FAILED, STAGED].map(std::ffi::OsStr::new);
            if entry.path().is_file() && !notes.contains(&name.as_os_str()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    fn read_version(&self, name: &str) -> Option<Version> {
        let text = std::fs::read_to_string(self.folder.join(name)).ok()?;
        Version::parse(text.trim())
    }

    fn write_version(&self, name: &str, version: Version) -> Result<(), UpdateError> {
        self.write(name, &format!("{version}\n"))
    }

    fn write(&self, name: &str, text: &str) -> Result<(), UpdateError> {
        let path = self.folder.join(name);
        std::fs::create_dir_all(&self.folder)
            .and_then(|()| std::fs::write(&path, text))
            .map_err(|source| UpdateError::File { path, source })
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> io::Result<()> {
    Ok(())
}

/// Starts a hidden PowerShell that waits for this process to exit, then
/// runs the Windows `installer` silently (`/S`) and has it start the new
/// version (`/R`). Waiting keeps the installer from closing the app itself.
#[cfg(windows)]
pub fn run_installer_after_exit(installer: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // The full path, so no other powershell.exe on the PATH runs instead.
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let powershell =
        Path::new(&system_root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    // Paths travel in variables, never inside the script text.
    let script = "while (Get-Process -Id $env:MASCATE_WAIT_PID -ErrorAction SilentlyContinue) \
                  { Start-Sleep -Milliseconds 100 }; \
                  Start-Process -FilePath $env:MASCATE_INSTALLER -ArgumentList '/S','/R'";
    std::process::Command::new(powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            script,
        ])
        .env("MASCATE_WAIT_PID", std::process::id().to_string())
        .env("MASCATE_INSTALLER", installer)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
}

/// Windows installers only run on Windows.
#[cfg(not(windows))]
pub fn run_installer_after_exit(_: &Path) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}
