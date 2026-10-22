//! The browser engine (#160, ADR 0035 decision 4): the installed Edge or
//! Chrome, run with a profile folder of the Mascate's own and remote
//! debugging on loopback, driven through the DevTools Protocol over a
//! WebSocket. The owner logs in by hand in its window, once; afterwards
//! pages open without a window. It only opens pages and reads them: it
//! never clicks, types or answers a captcha.

use std::ffi::OsString;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mascate_kernel::{Pause, PlatformError, ThreadPause};
use mascate_platform::http_agent;
use serde::Deserialize;
use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};

/// How long a page has to finish loading before it is read anyway.
const LOAD_WAIT: Duration = Duration::from_secs(30);
/// How long one DevTools command has to answer.
const COMMAND_WAIT: Duration = Duration::from_secs(15);
/// How often a page that is still drawing is read again, and how many
/// times: a panel draws its table after the page loads.
const READ_AGAIN_AFTER: Duration = Duration::from_millis(500);
const READ_ATTEMPTS: u32 = 30;
/// How long a browser has to start listening, checked this often.
const START_STEP: Duration = Duration::from_millis(250);
const START_ATTEMPTS: u32 = 120;
/// How long a browser asked to close has before it is stopped.
const CLOSE_ATTEMPTS: u32 = 20;
/// The file a browser started on port 0 writes in its profile: the port,
/// then the path of its own WebSocket.
const PORT_FILE: &str = "DevToolsActivePort";

/// Where each installed browser lives, in the order they are tried: Edge
/// first on Windows, where it always is; elsewhere Chrome, then Chromium,
/// then Edge, by the names their packages put on the `PATH`.
pub fn installed_browser(environment: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    const WINDOWS: [&str; 2] = [
        r"Microsoft\Edge\Application\msedge.exe",
        r"Google\Chrome\Application\chrome.exe",
    ];
    const ON_PATH: [&str; 6] = [
        "google-chrome-stable",
        "google-chrome",
        "chromium",
        "chromium-browser",
        "microsoft-edge-stable",
        "microsoft-edge",
    ];
    let candidates: Vec<PathBuf> = if cfg!(windows) {
        WINDOWS
            .iter()
            .flat_map(|program| {
                ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"]
                    .iter()
                    .filter_map(|folder| environment(folder))
                    .map(move |folder| Path::new(&folder).join(program))
            })
            .collect()
    } else {
        let path = OsString::from(environment("PATH").unwrap_or_default());
        let folders: Vec<PathBuf> = std::env::split_paths(&path).collect();
        ON_PATH
            .iter()
            .flat_map(|name| folders.iter().map(move |folder| folder.join(name)))
            .collect()
    };
    candidates.into_iter().find(|candidate| candidate.is_file())
}

/// The installed browser, run on the Mascate's own profile. One browser at
/// a time: a read waits for another read or for the login window to open.
pub struct BrowserSession {
    program: Option<PathBuf>,
    profile: PathBuf,
    /// A DevTools endpoint already listening, which the session never
    /// starts or stops: a test's fake server.
    attached: Option<String>,
    running: Mutex<Option<Running>>,
    pause: Arc<dyn Pause>,
}

/// A browser the session started.
struct Running {
    child: Child,
    /// `http://127.0.0.1:<port>`.
    endpoint: String,
    /// The browser's own WebSocket, which takes the command to close.
    browser: String,
    /// With a window the owner sees, rather than without one.
    window: bool,
}

impl BrowserSession {
    /// `program`, found by [`installed_browser`], on the profile folder
    /// `profile`; nothing starts until a page is opened.
    pub fn new(program: Option<PathBuf>, profile: PathBuf) -> Self {
        Self {
            program,
            profile,
            attached: None,
            running: Mutex::new(None),
            pause: Arc::new(ThreadPause),
        }
    }

    /// Talks to the DevTools endpoint at `endpoint`, `http://127.0.0.1:<port>`,
    /// as if the browser had been started on `profile`.
    pub fn attached(endpoint: &str, profile: PathBuf) -> Self {
        let mut session = Self::new(None, profile);
        session.attached = Some(endpoint.trim_end_matches('/').to_owned());
        session
    }

    /// Waits through `pause` between the reads of a page that is still
    /// drawing and while a browser starts.
    pub fn with_pause(mut self, pause: Arc<dyn Pause>) -> Self {
        self.pause = pause;
        self
    }

    /// Whether there is a browser to open pages with.
    pub fn is_installed(&self) -> bool {
        self.program.is_some() || self.attached.is_some()
    }

    /// Whether the profile was ever opened: before that, no panel has a
    /// login to read with.
    pub fn has_profile(&self) -> bool {
        self.profile.is_dir()
    }

    /// Opens `url` in a window the owner sees, to log in by hand. A window
    /// already open gets a new tab; a browser reading without a window is
    /// closed first, since one profile runs one browser.
    pub fn open_window(&self, url: &str) -> Result<(), PlatformError> {
        if let Some(endpoint) = &self.attached {
            return new_tab(endpoint, url).map(|_| ());
        }
        let mut running = self.running();
        forget_closed(&mut running);
        if let Some(window) = running.as_ref().filter(|open| open.window) {
            return new_tab(&window.endpoint, url).map(|_| ());
        }
        if let Some(headless) = running.take() {
            self.close(headless);
        }
        *running = Some(self.launch(true, url)?);
        Ok(())
    }

    /// Opens `url` in a new tab, waits for it to load and evaluates
    /// `script` in it, again a little later each time until `ready` accepts
    /// the value or the attempts run out; the last value is the answer
    /// either way. `script` must only read the page. The tab closes after,
    /// and so does a browser started for this read.
    pub fn read(
        &self,
        url: &str,
        script: &str,
        ready: &dyn Fn(&Value) -> bool,
    ) -> Result<Value, PlatformError> {
        let mut running = self.running();
        forget_closed(&mut running);
        let endpoint = match (&self.attached, running.as_ref()) {
            (Some(endpoint), _) => endpoint.clone(),
            (None, Some(open)) => open.endpoint.clone(),
            (None, None) => {
                let started = self.launch(false, "about:blank")?;
                let endpoint = started.endpoint.clone();
                *running = Some(started);
                endpoint
            }
        };
        let value = self.read_tab(&endpoint, url, script, ready);
        if let Some(headless) = running.take_if(|open| !open.window) {
            self.close(headless);
        }
        value
    }

    fn read_tab(
        &self,
        endpoint: &str,
        url: &str,
        script: &str,
        ready: &dyn Fn(&Value) -> bool,
    ) -> Result<Value, PlatformError> {
        let target = new_tab(endpoint, "about:blank")?;
        let value = Tab::connect(&target.web_socket_debugger_url).and_then(|mut tab| {
            tab.call("Page.enable", json!({}))?;
            let navigated = tab.call("Page.navigate", json!({ "url": url }))?;
            if let Some(why) = navigated.get("errorText").and_then(Value::as_str) {
                return Err(PlatformError::Failed(format!("a página não abriu: {why}")));
            }
            tab.wait_for_load();
            let mut value = Value::Null;
            for attempt in 0..READ_ATTEMPTS {
                if attempt > 0 {
                    self.pause.pause(READ_AGAIN_AFTER);
                }
                value = tab.evaluate(script)?;
                if ready(&value) {
                    break;
                }
            }
            Ok(value)
        });
        close_tab(endpoint, &target.id);
        value
    }

    /// Starts the browser on the profile with remote debugging on a port
    /// it picks, opening `url`, and waits for it to say which port.
    fn launch(&self, window: bool, url: &str) -> Result<Running, PlatformError> {
        let program = self.program.as_ref().ok_or_else(not_installed)?;
        std::fs::create_dir_all(&self.profile).map_err(|error| {
            PlatformError::Failed(format!("não consegui criar a pasta do navegador: {error}"))
        })?;
        let port_file = self.profile.join(PORT_FILE);
        let _ = std::fs::remove_file(&port_file);
        let mut command = Command::new(program);
        command
            .arg(format!("--user-data-dir={}", self.profile.display()))
            .args([
                "--remote-debugging-port=0",
                "--no-first-run",
                "--no-default-browser-check",
            ]);
        if !window {
            command.arg("--headless=new");
        }
        let mut child = command
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                PlatformError::Failed(format!("não consegui abrir o navegador: {error}"))
            })?;
        for _ in 0..START_ATTEMPTS {
            if let Some((port, browser)) = std::fs::read_to_string(&port_file)
                .ok()
                .as_deref()
                .and_then(listening)
            {
                return Ok(Running {
                    child,
                    endpoint: format!("http://127.0.0.1:{port}"),
                    browser: format!("ws://127.0.0.1:{port}{browser}"),
                    window,
                });
            }
            if let Ok(Some(_)) = child.try_wait() {
                return Err(PlatformError::Failed(
                    "o navegador fechou logo ao abrir; se a janela do navegador do Mascate \
                     estiver aberta por outro programa, feche-a e tente de novo"
                        .into(),
                ));
            }
            self.pause.pause(START_STEP);
        }
        let _ = child.kill();
        let _ = child.wait();
        Err(PlatformError::Failed(
            "o navegador não abriu a depuração a tempo".into(),
        ))
    }

    /// Asks the browser to close, so it saves the profile, and stops it if
    /// it does not.
    fn close(&self, mut running: Running) {
        if let Ok(mut browser) = Tab::connect(&running.browser) {
            let _ = browser.send("Browser.close", json!({}));
        }
        for _ in 0..CLOSE_ATTEMPTS {
            if let Ok(Some(_)) = running.child.try_wait() {
                return;
            }
            self.pause.pause(START_STEP);
        }
        let _ = running.child.kill();
        let _ = running.child.wait();
    }

    fn running(&self) -> std::sync::MutexGuard<'_, Option<Running>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for BrowserSession {
    /// A browser reading without a window never outlives the app; the
    /// owner's window is theirs to close.
    fn drop(&mut self) {
        if let Some(headless) = self.running().take_if(|open| !open.window) {
            self.close(headless);
        }
    }
}

fn not_installed() -> PlatformError {
    PlatformError::Refused("não há Edge nem Chrome instalado neste computador".into())
}

/// Forgets a browser that is no longer running: the owner closed its
/// window.
fn forget_closed(running: &mut Option<Running>) {
    if running
        .as_mut()
        .is_some_and(|open| !matches!(open.child.try_wait(), Ok(None)))
    {
        *running = None;
    }
}

/// The port and the browser's WebSocket path, once the browser wrote both.
fn listening(port_file: &str) -> Option<(u16, String)> {
    let mut lines = port_file.lines();
    let port = lines.next()?.trim().parse().ok()?;
    let path = lines.next()?.trim();
    path.starts_with('/').then(|| (port, path.to_owned()))
}

/// A tab, as the DevTools endpoint lists it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Target {
    id: String,
    web_socket_debugger_url: String,
}

/// Opens a tab on `url`; the endpoint takes it only by PUT.
fn new_tab(endpoint: &str, url: &str) -> Result<Target, PlatformError> {
    let agent = http_agent(endpoint, "Mascate", COMMAND_WAIT);
    let mut response = agent
        .put(format!("{endpoint}/json/new?{url}"))
        .send_empty()
        .map_err(not_answering)?;
    if response.status() != 200 {
        return Err(PlatformError::Failed(format!(
            "o navegador recusou abrir uma aba ({})",
            response.status()
        )));
    }
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(not_answering)?;
    serde_json::from_str(&body)
        .map_err(|error| PlatformError::Failed(format!("o navegador respondeu sem a aba: {error}")))
}

fn close_tab(endpoint: &str, id: &str) {
    let _ = http_agent(endpoint, "Mascate", COMMAND_WAIT)
        .get(format!("{endpoint}/json/close/{id}"))
        .call();
}

fn not_answering(error: impl std::fmt::Display) -> PlatformError {
    PlatformError::Failed(format!("o navegador parou de responder: {error}"))
}

/// One tab's WebSocket, which takes a command and answers it by its id,
/// with the tab's events in between.
struct Tab {
    socket: WebSocket<TcpStream>,
    last_id: u64,
    loaded: bool,
}

impl Tab {
    /// Connects to a WebSocket on this computer only: the DevTools Protocol
    /// drives the whole browser, so it never goes to another address.
    fn connect(url: &str) -> Result<Self, PlatformError> {
        let address = loopback(url).ok_or_else(|| {
            PlatformError::Failed(format!(
                "o navegador indicou um endereço fora do computador: {url}"
            ))
        })?;
        let stream = TcpStream::connect_timeout(&address, COMMAND_WAIT).map_err(not_answering)?;
        stream
            .set_read_timeout(Some(COMMAND_WAIT))
            .map_err(not_answering)?;
        let (socket, _) = tungstenite::client(url, stream).map_err(not_answering)?;
        Ok(Self {
            socket,
            last_id: 0,
            loaded: false,
        })
    }

    fn send(&mut self, method: &str, params: Value) -> Result<u64, PlatformError> {
        self.last_id += 1;
        let command = json!({ "id": self.last_id, "method": method, "params": params });
        self.socket
            .send(Message::text(command.to_string()))
            .map_err(not_answering)?;
        Ok(self.last_id)
    }

    /// Sends a command and waits for its answer.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, PlatformError> {
        let id = self.send(method, params)?;
        let deadline = Instant::now() + COMMAND_WAIT;
        loop {
            let message = self.next(deadline)?.ok_or_else(|| {
                PlatformError::Failed(format!("o navegador não respondeu a {method}"))
            })?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                let why = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("erro sem descrição");
                return Err(PlatformError::Failed(format!(
                    "o navegador recusou {method}: {why}"
                )));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Waits for the page to finish loading, up to [`LOAD_WAIT`]: a page
    /// that never finishes is read as it stands.
    fn wait_for_load(&mut self) {
        let deadline = Instant::now() + LOAD_WAIT;
        while !self.loaded {
            if !matches!(self.next(deadline), Ok(Some(_))) {
                return;
            }
        }
    }

    /// Evaluates `script` in the page and gives back what it returns.
    fn evaluate(&mut self, script: &str) -> Result<Value, PlatformError> {
        let answer = self.call(
            "Runtime.evaluate",
            json!({ "expression": script, "returnByValue": true, "awaitPromise": true }),
        )?;
        if let Some(exception) = answer.get("exceptionDetails") {
            let why = exception
                .pointer("/exception/description")
                .or_else(|| exception.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("erro sem descrição");
            return Err(PlatformError::Failed(format!(
                "a leitura da página falhou: {why}"
            )));
        }
        Ok(answer
            .pointer("/result/value")
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// The next message before `deadline`, noting when the page loaded;
    /// `None` once the deadline passes.
    fn next(&mut self, deadline: Instant) -> Result<Option<Value>, PlatformError> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.socket
                .get_ref()
                .set_read_timeout(Some(left))
                .map_err(not_answering)?;
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    let Ok(message) = serde_json::from_str::<Value>(text.as_str()) else {
                        continue;
                    };
                    if message.get("method").and_then(Value::as_str) == Some("Page.loadEventFired")
                    {
                        self.loaded = true;
                    }
                    return Ok(Some(message));
                }
                Ok(_) => continue,
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(not_answering(error)),
            }
        }
    }
}

/// The address of a `ws://` URL on this computer; `None` for any other.
fn loopback(url: &str) -> Option<SocketAddr> {
    let rest = url.strip_prefix("ws://")?;
    let authority = rest.split('/').next()?;
    let (host, _) = authority.rsplit_once(':')?;
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        return None;
    }
    authority
        .to_socket_addrs()
        .ok()?
        .find(|address| address.ip().is_loopback())
}
