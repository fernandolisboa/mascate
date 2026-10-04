//! Updates against a fake GitHub serving the Releases API answers in
//! `fixtures/github-releases` (built from GitHub's documentation).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::testing::FakeHttpServer;
use mascate_platform::{
    Database, Finish, Installation, MIGRATIONS, ModuleMigrations, Release, ReleaseChannel,
    ReleaseError, Target, UpdateError, UpdateSettings, Updater, Version, load_update_settings,
    migrate, save_update_settings,
};
use sha2::{Digest, Sha256};

const THIS_APP: &[ModuleMigrations] = &[MIGRATIONS];
const REPOSITORY: &str = "/repos/fernandolisboa/mascate";
const DOWNLOADS: &str = "/fernandolisboa/mascate/releases/download/";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/github-releases")
            .join(name),
    )
    .unwrap()
}

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// GitHub as the app sees it: the Releases API and the downloads.
struct FakeGitHub {
    server: FakeHttpServer,
}

impl FakeGitHub {
    fn new() -> Self {
        Self {
            server: FakeHttpServer::start(),
        }
    }

    fn windows_installer(version: &str) -> Vec<u8> {
        format!("windows installer of {version}")
            .repeat(1000)
            .into_bytes()
    }

    fn appimage(version: &str) -> Vec<u8> {
        format!("appimage of {version}").repeat(1000).into_bytes()
    }

    /// Publishes `version` as the latest Release, declaring `risky`
    /// migrations in its manifest.
    fn publish(&self, version: &str, risky: &[(&str, u32)]) {
        let (release, manifest) = self.answers(version, risky);
        self.server.serve(
            &format!("{REPOSITORY}/releases/latest"),
            200,
            release.clone(),
        );
        self.server.serve(
            &format!("{REPOSITORY}/releases/tags/v{version}"),
            200,
            release,
        );
        let files = format!("{DOWNLOADS}v{version}/");
        self.server
            .serve(&format!("{files}mascate-update.json"), 200, manifest);
        // GitHub sends downloads on to its file storage.
        for (name, bytes) in [
            (
                format!("Mascate_{version}_x64-setup.exe"),
                Self::windows_installer(version),
            ),
            (
                format!("Mascate_{version}_x86_64.AppImage"),
                Self::appimage(version),
            ),
        ] {
            let stored = format!("/release-assets/{name}");
            self.server
                .redirect(&format!("{files}{name}"), &self.url(&stored));
            self.server.serve(&stored, 200, bytes);
        }
    }

    /// The release and manifest JSON for `version`.
    fn answers(&self, version: &str, risky: &[(&str, u32)]) -> (String, String) {
        let windows = Self::windows_installer(version);
        let appimage = Self::appimage(version);
        let fill = |text: String| {
            text.replace("{server}", self.server.url())
                .replace("{version}", version)
                .replace("{windows_size}", &windows.len().to_string())
                .replace("{windows_sha256}", &sha256(&windows))
                .replace("{appimage_size}", &appimage.len().to_string())
                .replace("{appimage_sha256}", &sha256(&appimage))
        };
        let risky: Vec<String> = risky
            .iter()
            .map(|(module, version)| format!(r#"{{"module": "{module}", "version": {version}}}"#))
            .collect();
        let manifest = fill(fixture("mascate-update.json")).replace(
            r#""risky_migrations": []"#,
            &format!(r#""risky_migrations": [{}]"#, risky.join(", ")),
        );
        (fill(fixture("release.json")), manifest)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.server.url())
    }

    fn channel(&self, target: Option<Target>) -> ReleaseChannel {
        ReleaseChannel::new(
            &self.url(REPOSITORY),
            &self.url(DOWNLOADS),
            "Mascate/test",
            target,
        )
    }

    fn updater(&self, current: &str, installation: Installation, folder: &Path) -> Updater {
        Updater::new(
            self.channel(installation.target()),
            version(current),
            installation,
            folder.to_path_buf(),
            THIS_APP,
        )
    }
}

fn updates_folder(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("updates")
}

#[test]
fn a_newer_release_comes_with_its_changelog_page_and_installer() {
    let github = FakeGitHub::new();
    github.publish("0.2.0", &[]);
    let dir = tempfile::tempdir().unwrap();
    let updater = github.updater(
        "0.1.0",
        Installation::WindowsInstaller,
        &updates_folder(&dir),
    );

    let release = updater.check().unwrap().unwrap();

    assert_eq!(release.version, version("0.2.0"));
    assert!(release.notes.contains("atualização com um clique"));
    assert_eq!(
        release.page,
        github.url("/fernandolisboa/mascate/releases/tag/v0.2.0")
    );
    let installer = release.installer.as_ref().unwrap();
    assert_eq!(installer.file_name, "Mascate_0.2.0_x64-setup.exe");
    assert_eq!(
        installer.size,
        FakeGitHub::windows_installer("0.2.0").len() as u64
    );
    assert!(!updater.is_risky(&release));
    assert!(updater.installs_silently(&release));
}

#[test]
fn nothing_is_offered_when_the_app_is_up_to_date_or_ahead() {
    let github = FakeGitHub::new();
    github.publish("0.2.0", &[]);
    let dir = tempfile::tempdir().unwrap();
    for current in ["0.2.0", "0.3.0"] {
        let updater = github.updater(current, Installation::Manual, &updates_folder(&dir));
        assert_eq!(updater.check().unwrap(), None, "{current}");
    }
}

#[test]
fn nothing_is_offered_before_the_first_release() {
    let github = FakeGitHub::new();
    let dir = tempfile::tempdir().unwrap();
    let updater = github.updater("0.1.0", Installation::Manual, &updates_folder(&dir));
    assert_eq!(updater.check().unwrap(), None);
}

#[test]
fn rate_limits_are_reported_as_such() {
    for status in [403, 429] {
        let github = FakeGitHub::new();
        github.server.serve(
            &format!("{REPOSITORY}/releases/latest"),
            status,
            r#"{"message": "API rate limit exceeded"}"#,
        );
        let channel = github.channel(None);
        assert!(
            matches!(channel.latest(), Err(ReleaseError::RateLimited)),
            "{status}"
        );
    }
}

#[test]
fn a_copy_the_owner_updates_sees_the_release_without_an_installer() {
    let github = FakeGitHub::new();
    github.publish("0.2.0", &[]);
    let dir = tempfile::tempdir().unwrap();
    let updater = github.updater("0.1.0", Installation::Manual, &updates_folder(&dir));

    let release = updater.check().unwrap().unwrap();

    assert_eq!(release.installer, None);
    assert!(!updater.installs_silently(&release));
    assert!(matches!(
        updater.install(&release),
        Err(UpdateError::NoInstaller(_))
    ));
}

#[test]
fn a_release_with_a_risky_migration_this_app_has_not_run_is_never_silent() {
    let github = FakeGitHub::new();
    let dir = tempfile::tempdir().unwrap();
    let known = MIGRATIONS.latest_version();
    for (risky, expected) in [
        // Already run here: nothing new is risky.
        (vec![("platform", known)], false),
        (vec![("platform", known + 1)], true),
        // Brought by a version between this one and the newest.
        (vec![("platform", known + 1), ("catalog", 1)], true),
        (vec![("catalog", 1)], true),
    ] {
        github.publish("0.3.0", &risky);
        let updater = github.updater(
            "0.1.0",
            Installation::WindowsInstaller,
            &updates_folder(&dir),
        );
        let release = updater.check().unwrap().unwrap();
        assert_eq!(updater.is_risky(&release), expected, "{risky:?}");
        assert_eq!(updater.installs_silently(&release), !expected, "{risky:?}");
    }
}

#[test]
fn installing_on_windows_downloads_the_checked_installer_to_run_after_exit() {
    let github = FakeGitHub::new();
    github.publish("0.2.0", &[]);
    let dir = tempfile::tempdir().unwrap();
    let folder = updates_folder(&dir);
    let updater = github.updater("0.1.0", Installation::WindowsInstaller, &folder);
    let release = updater.check().unwrap().unwrap();

    let finish = updater.install(&release).unwrap();

    let installer = folder.join("Mascate_0.2.0_x64-setup.exe");
    assert_eq!(finish, Finish::RunInstaller(installer.clone()));
    assert_eq!(
        std::fs::read(&installer).unwrap(),
        FakeGitHub::windows_installer("0.2.0")
    );
    assert_eq!(updater.previous(), Some(version("0.1.0")));
}

#[test]
fn an_installer_that_does_not_match_its_release_is_thrown_away() {
    for (tampered, expected_size_error) in [
        (FakeGitHub::windows_installer("0.2.0")[1..].to_vec(), true),
        (
            FakeGitHub::windows_installer("0.2.0")
                .iter()
                .map(|byte| byte ^ 1)
                .collect(),
            false,
        ),
        (FakeGitHub::windows_installer("0.2.0").repeat(2), true),
    ] {
        let github = FakeGitHub::new();
        github.publish("0.2.0", &[]);
        github
            .server
            .serve("/release-assets/Mascate_0.2.0_x64-setup.exe", 200, tampered);
        let dir = tempfile::tempdir().unwrap();
        let folder = updates_folder(&dir);
        let updater = github.updater("0.1.0", Installation::WindowsInstaller, &folder);
        let release = updater.check().unwrap().unwrap();

        let error = updater.install(&release).unwrap_err();

        match error {
            UpdateError::Release(ReleaseError::SizeMismatch { .. }) => {
                assert!(expected_size_error)
            }
            UpdateError::Release(ReleaseError::ChecksumMismatch { .. }) => {
                assert!(!expected_size_error)
            }
            other => panic!("unexpected {other}"),
        }
        let left: Vec<_> = std::fs::read_dir(&folder).unwrap().flatten().collect();
        assert!(left.is_empty(), "{left:?}");
        assert_eq!(updater.previous(), None);
    }
}

#[test]
fn a_release_whose_files_come_from_elsewhere_is_refused() {
    let github = FakeGitHub::new();
    github.publish("0.2.0", &[]);
    let (release, _) = github.answers("0.2.0", &[]);
    let elsewhere = release.replace(
        &github.url("/fernandolisboa/mascate/releases/download/v0.2.0/Mascate_0.2.0_x64-setup.exe"),
        &github.url("/someone-else/releases/download/v0.2.0/Mascate_0.2.0_x64-setup.exe"),
    );
    github
        .server
        .serve(&format!("{REPOSITORY}/releases/latest"), 200, elsewhere);
    let channel = github.channel(Some(Target::WindowsInstaller));

    assert!(matches!(
        channel.latest(),
        Err(ReleaseError::Invalid { .. })
    ));
}

#[test]
fn a_manifest_for_another_version_is_refused() {
    let github = FakeGitHub::new();
    github.publish("0.2.0", &[]);
    let (_, manifest) = github.answers("0.2.0", &[]);
    github.server.serve(
        &format!("{DOWNLOADS}v0.2.0/mascate-update.json"),
        200,
        manifest.replace(r#""version": "0.2.0""#, r#""version": "0.1.9""#),
    );

    assert!(matches!(
        github.channel(None).latest(),
        Err(ReleaseError::Invalid { .. })
    ));
}

#[test]
fn a_manifest_naming_a_path_instead_of_a_file_is_refused() {
    let github = FakeGitHub::new();
    github.publish("0.2.0", &[]);
    let (_, manifest) = github.answers("0.2.0", &[]);
    github.server.serve(
        &format!("{DOWNLOADS}v0.2.0/mascate-update.json"),
        200,
        manifest.replace(
            r#""file": "Mascate_0.2.0_x64-setup.exe""#,
            r#""file": "../../Startup/Mascate_0.2.0_x64-setup.exe""#,
        ),
    );

    assert!(matches!(
        github.channel(Some(Target::WindowsInstaller)).latest(),
        Err(ReleaseError::Invalid { .. })
    ));
}

#[cfg(unix)]
#[test]
fn installing_an_appimage_swaps_the_file_and_restarts_from_it() {
    use std::os::unix::fs::PermissionsExt;

    let github = FakeGitHub::new();
    github.publish("0.2.0", &[]);
    let dir = tempfile::tempdir().unwrap();
    let applications = dir.path().join("Applications");
    std::fs::create_dir_all(&applications).unwrap();
    let appimage = applications.join("Mascate.AppImage");
    std::fs::write(&appimage, FakeGitHub::appimage("0.1.0")).unwrap();
    let updater = github.updater(
        "0.1.0",
        Installation::AppImage(appimage.clone()),
        &updates_folder(&dir),
    );
    let release = updater.check().unwrap().unwrap();

    let finish = updater.install(&release).unwrap();

    assert_eq!(finish, Finish::Restart(appimage.clone()));
    assert_eq!(
        std::fs::read(&appimage).unwrap(),
        FakeGitHub::appimage("0.2.0")
    );
    let mode = std::fs::metadata(&appimage).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0o111, "executable: {mode:o}");
    let others: Vec<_> = std::fs::read_dir(&applications)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(others, ["Mascate.AppImage"]);
}

#[test]
fn a_silent_update_waits_for_the_next_start_and_runs_once() {
    let github = FakeGitHub::new();
    github.publish("0.2.0", &[]);
    let dir = tempfile::tempdir().unwrap();
    let folder = updates_folder(&dir);
    let updater = github.updater("0.1.0", Installation::WindowsInstaller, &folder);
    let release = updater.check().unwrap().unwrap();

    updater.install_on_next_start(&release).unwrap();

    let next_start = github.updater("0.1.0", Installation::WindowsInstaller, &folder);
    assert_eq!(
        next_start.take_staged(),
        Some(Finish::RunInstaller(
            folder.join("Mascate_0.2.0_x64-setup.exe")
        ))
    );
    assert_eq!(next_start.take_staged(), None);
}

#[test]
fn a_staged_update_the_app_already_has_is_not_run() {
    let github = FakeGitHub::new();
    github.publish("0.2.0", &[]);
    let dir = tempfile::tempdir().unwrap();
    let folder = updates_folder(&dir);
    let updater = github.updater("0.1.0", Installation::WindowsInstaller, &folder);
    let release = updater.check().unwrap().unwrap();
    updater.install_on_next_start(&release).unwrap();

    // The owner installed 0.2.0 by hand meanwhile.
    let installed = github.updater("0.2.0", Installation::WindowsInstaller, &folder);
    assert_eq!(installed.take_staged(), None);
}

/// 0.2.0 updated from 0.1.0, then could not migrate the database.
fn after_a_failed_update(github: &FakeGitHub, folder: &Path) -> Updater {
    github.publish("0.1.0", &[]);
    github.publish("0.2.0", &[]);
    let old = github.updater("0.1.0", Installation::WindowsInstaller, folder);
    let release = old.check().unwrap().unwrap();
    old.install(&release).unwrap();
    github.updater("0.2.0", Installation::WindowsInstaller, folder)
}

#[test]
fn a_version_that_could_not_migrate_goes_back_to_the_one_before() {
    let github = FakeGitHub::new();
    let dir = tempfile::tempdir().unwrap();
    let folder = updates_folder(&dir);
    let failed = after_a_failed_update(&github, &folder);

    let finish = failed.roll_back().unwrap();

    let installer = folder.join("Mascate_0.1.0_x64-setup.exe");
    assert_eq!(finish, Finish::RunInstaller(installer.clone()));
    assert_eq!(
        std::fs::read(&installer).unwrap(),
        FakeGitHub::windows_installer("0.1.0")
    );
    assert!(
        github
            .server
            .requests()
            .contains(&format!("{REPOSITORY}/releases/tags/v0.1.0"))
    );
}

#[test]
fn back_on_the_version_before_a_silent_update_skips_the_one_that_failed() {
    let github = FakeGitHub::new();
    let dir = tempfile::tempdir().unwrap();
    let folder = updates_folder(&dir);
    after_a_failed_update(&github, &folder).roll_back().unwrap();

    let back = github.updater("0.1.0", Installation::WindowsInstaller, &folder);
    let offered: Release = back.check().unwrap().unwrap();
    assert!(back.failed_before(&offered));
    assert!(!back.installs_silently(&offered));

    github.publish("0.2.1", &[]);
    let fixed = back.check().unwrap().unwrap();
    assert!(!back.failed_before(&fixed));
    assert!(back.installs_silently(&fixed));
}

#[test]
fn there_is_nothing_to_go_back_to_without_an_update_on_record() {
    let github = FakeGitHub::new();
    github.publish("0.1.0", &[]);
    let dir = tempfile::tempdir().unwrap();
    let updater = github.updater(
        "0.2.0",
        Installation::WindowsInstaller,
        &updates_folder(&dir),
    );
    assert!(matches!(
        updater.roll_back(),
        Err(UpdateError::NothingToGoBackTo)
    ));
}

#[test]
fn silent_updates_are_off_until_the_owner_turns_them_on() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        let clock = ManualClock::at(Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap());
        let ids = SequentialIds::default();
        migrate(&database, &clock, THIS_APP).await.unwrap();

        assert_eq!(
            load_update_settings(&database).await.unwrap(),
            UpdateSettings { silent: false }
        );
        save_update_settings(&database, &clock, &ids, UpdateSettings { silent: true })
            .await
            .unwrap();
        assert_eq!(
            load_update_settings(&database).await.unwrap(),
            UpdateSettings { silent: true }
        );
    });
}
