//! Which browser the panels and markets open in (#233): the owner's default
//! browser when it speaks the DevTools Protocol, found through the
//! operating system's own answers (recorded here), else Chrome or Edge.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mascate_integrations::{browser_name, installed_browser};

/// A computer: its environment variables, and what each program the lookup
/// runs prints for the arguments it gets.
struct Computer {
    folder: tempfile::TempDir,
    environment: HashMap<String, String>,
    answers: HashMap<String, String>,
    asked: RefCell<Vec<String>>,
}

impl Computer {
    fn new() -> Self {
        Self {
            folder: tempfile::tempdir().unwrap(),
            environment: HashMap::new(),
            answers: HashMap::new(),
            asked: RefCell::new(Vec::new()),
        }
    }

    fn set(&mut self, name: &str, value: &str) {
        self.environment.insert(name.to_owned(), value.to_owned());
    }

    fn answer(&mut self, program: &str, args: &[&str], printed: &str) {
        self.answers
            .insert(command_line(program, args), printed.to_owned());
    }

    /// An empty file at `relative` inside the computer's folder.
    fn install(&self, relative: &str) -> PathBuf {
        let program = self.folder.path().join(relative);
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, "").unwrap();
        program
    }

    fn root(&self) -> String {
        self.folder.path().display().to_string()
    }

    fn browser(&self) -> Option<PathBuf> {
        let environment = |name: &str| self.environment.get(name).cloned();
        let ask = |program: &str, args: &[&str]| {
            let line = command_line(program, args);
            self.asked.borrow_mut().push(line.clone());
            self.answers.get(&line).cloned()
        };
        installed_browser(&environment, &ask)
    }
}

fn command_line(program: &str, args: &[&str]) -> String {
    std::iter::once(program)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn the_browsers_driven_are_the_chromium_family_by_their_program_name() {
    for (program, called) in [
        (
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            Some("Chrome"),
        ),
        ("/usr/bin/google-chrome-stable", Some("Chrome")),
        (
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            Some("Edge"),
        ),
        ("/usr/bin/brave-browser", Some("Brave")),
        (
            r"C:\Users\ana\AppData\Local\Vivaldi\Application\vivaldi.exe",
            Some("Vivaldi"),
        ),
        ("/usr/bin/chromium", Some("Chromium")),
        (r"C:\Program Files\Mozilla Firefox\firefox.exe", None),
        (
            r"C:\Users\ana\AppData\Local\Programs\Opera\launcher.exe",
            None,
        ),
    ] {
        let program = program.replace('\\', std::path::MAIN_SEPARATOR_STR);
        assert_eq!(browser_name(Path::new(&program)), called, "{program}");
    }
}

#[test]
fn with_nothing_installed_there_is_no_browser() {
    assert_eq!(Computer::new().browser(), None);
}

#[cfg(windows)]
mod windows {
    use super::*;

    const LATEST: &str = r"HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoiceLatest";
    const OLDER: &str =
        r"HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice";

    /// A Windows with `reg.exe` under `SystemRoot` and the program folders
    /// inside the computer's folder.
    fn windows() -> (Computer, String) {
        let mut computer = Computer::new();
        let root = computer.root();
        computer.set("SystemRoot", &format!(r"{root}\Windows"));
        computer.set("ProgramFiles", &format!(r"{root}\Program Files"));
        computer.set("ProgramFiles(x86)", &format!(r"{root}\Program Files (x86)"));
        computer.set("LOCALAPPDATA", &format!(r"{root}\Local"));
        (computer, format!(r"{root}\Windows\System32\reg.exe"))
    }

    fn prog_id(computer: &mut Computer, reg: &str, key: &str, prog_id: &str) {
        computer.answer(
            reg,
            &["query", key, "/v", "ProgId"],
            &format!("\r\n{key}\r\n    ProgId    REG_SZ    {prog_id}\r\n\r\n"),
        );
    }

    fn command(computer: &mut Computer, reg: &str, prog_id: &str, kind: &str, command: &str) {
        let key = format!(r"HKCR\{prog_id}\shell\open\command");
        computer.answer(
            reg,
            &["query", &key, "/ve"],
            &format!("\r\n{key}\r\n    (Padrão)    {kind}    {command}\r\n\r\n"),
        );
    }

    #[test]
    fn the_default_browser_wins_over_edge_when_it_speaks_devtools() {
        let (mut computer, reg) = windows();
        computer.install(r"Program Files (x86)\Microsoft\Edge\Application\msedge.exe");
        let brave =
            computer.install(r"Program Files\BraveSoftware\Brave-Browser\Application\brave.exe");
        prog_id(&mut computer, &reg, LATEST, "BraveHTML");
        command(
            &mut computer,
            &reg,
            "BraveHTML",
            "REG_SZ",
            &format!("\"{}\" --single-argument %1", brave.display()),
        );

        assert_eq!(computer.browser(), Some(brave));
        assert!(
            computer
                .asked
                .borrow()
                .iter()
                .all(|line| line.starts_with(&reg)),
            "only the system's reg.exe is asked: {:?}",
            computer.asked.borrow()
        );
    }

    #[test]
    fn an_older_windows_keeps_the_choice_where_it_wrote_it_before() {
        let (mut computer, reg) = windows();
        let chrome = computer.install(r"Local\Google\Chrome\Application\chrome.exe");
        computer.install(r"Program Files (x86)\Microsoft\Edge\Application\msedge.exe");
        prog_id(&mut computer, &reg, OLDER, "ChromeHTML");
        command(
            &mut computer,
            &reg,
            "ChromeHTML",
            "REG_EXPAND_SZ",
            r#""%LOCALAPPDATA%\Google\Chrome\Application\chrome.exe" --single-argument %1"#,
        );

        assert_eq!(computer.browser(), Some(chrome));
    }

    #[test]
    fn a_default_that_cannot_be_driven_falls_back_to_chrome_before_edge() {
        let (mut computer, reg) = windows();
        let firefox = computer.install(r"Program Files\Mozilla Firefox\firefox.exe");
        computer.install(r"Program Files (x86)\Microsoft\Edge\Application\msedge.exe");
        let chrome = computer.install(r"Program Files\Google\Chrome\Application\chrome.exe");
        prog_id(&mut computer, &reg, LATEST, "FirefoxURL-308046B0AF4A39CB");
        command(
            &mut computer,
            &reg,
            "FirefoxURL-308046B0AF4A39CB",
            "REG_SZ",
            &format!("\"{}\" -osint -url \"%1\"", firefox.display()),
        );

        assert_eq!(computer.browser(), Some(chrome));
    }

    #[test]
    fn without_a_default_or_chrome_edge_opens() {
        let (computer, _) = windows();
        let edge = computer.install(r"Program Files (x86)\Microsoft\Edge\Application\msedge.exe");

        assert_eq!(computer.browser(), Some(edge));
    }

    #[test]
    fn a_default_whose_program_is_gone_falls_back() {
        let (mut computer, reg) = windows();
        let edge = computer.install(r"Program Files (x86)\Microsoft\Edge\Application\msedge.exe");
        prog_id(&mut computer, &reg, LATEST, "BraveHTML");
        command(
            &mut computer,
            &reg,
            "BraveHTML",
            "REG_SZ",
            r#""C:\Nowhere\brave.exe" --single-argument %1"#,
        );

        assert_eq!(computer.browser(), Some(edge));
    }
}

#[cfg(not(windows))]
mod xdg {
    use super::*;

    const ASK: [&str; 2] = ["get", "default-web-browser"];

    /// A Linux whose data folders and `PATH` are inside the computer's
    /// folder.
    fn linux() -> Computer {
        let mut computer = Computer::new();
        let root = computer.root();
        computer.set("HOME", &format!("{root}/home"));
        computer.set("XDG_DATA_DIRS", &format!("{root}/usr/share"));
        computer.set("PATH", &format!("{root}/usr/bin"));
        computer
    }

    fn entry(computer: &Computer, relative: &str, exec: &str) {
        let file = computer.folder.path().join(relative);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            file,
            format!("[Desktop Entry]\nName=Browser\nExec={exec}\n\n[Desktop Action new-window]\nExec=other --new-window\n"),
        )
        .unwrap();
    }

    #[test]
    fn the_default_browser_wins_over_chrome_when_it_speaks_devtools() {
        let mut computer = linux();
        computer.install("usr/bin/google-chrome-stable");
        let brave = computer.install("opt/brave.com/brave/brave-browser");
        entry(
            &computer,
            "home/.local/share/applications/brave-browser.desktop",
            &format!("{} %U", brave.display()),
        );
        computer.answer("xdg-settings", &ASK, "brave-browser.desktop\n");

        assert_eq!(computer.browser(), Some(brave));
    }

    #[test]
    fn an_entry_naming_a_program_on_the_path_is_found_there() {
        let mut computer = linux();
        let root = computer.root();
        computer.set("XDG_DATA_HOME", &format!("{root}/data"));
        let vivaldi = computer.install("usr/bin/vivaldi-stable");
        entry(
            &computer,
            "usr/share/applications/vivaldi-stable.desktop",
            "\"vivaldi-stable\" %U",
        );
        computer.answer("xdg-settings", &ASK, "vivaldi-stable.desktop\n");

        assert_eq!(computer.browser(), Some(vivaldi));
    }

    #[test]
    fn a_default_that_cannot_be_driven_falls_back_to_chrome_then_chromium() {
        let mut computer = linux();
        computer.install("usr/bin/firefox");
        let chromium = computer.install("usr/bin/chromium");
        entry(
            &computer,
            "usr/share/applications/firefox.desktop",
            "firefox %u",
        );
        computer.answer("xdg-settings", &ASK, "firefox.desktop\n");

        assert_eq!(computer.browser(), Some(chromium.clone()));

        let chrome = computer.install("usr/bin/google-chrome-stable");
        assert_eq!(computer.browser(), Some(chrome));
    }

    #[test]
    fn an_answer_that_is_a_path_is_not_followed() {
        let mut computer = linux();
        let chromium = computer.install("usr/bin/chromium");
        let brave = computer.install("usr/bin/brave-browser");
        entry(
            &computer,
            "home/.local/share/applications/brave-browser.desktop",
            &brave.display().to_string(),
        );
        computer.answer(
            "xdg-settings",
            &ASK,
            "../applications/brave-browser.desktop\n",
        );

        assert_eq!(computer.browser(), Some(chromium));
    }

    #[test]
    fn without_xdg_settings_chrome_or_chromium_opens() {
        let computer = linux();
        let chromium = computer.install("usr/bin/chromium");

        assert_eq!(computer.browser(), Some(chromium));
    }
}
