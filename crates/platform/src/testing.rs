//! Doubles for other crates' tests: a [`SecretStore`] in memory and a fake
//! HTTP server.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::{Secret, SecretStore, SecretStoreError};

/// Keeps secrets in a map. [`MemorySecretStore::unavailable`] fails every
/// call, like a system store that is locked or missing.
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    secrets: Mutex<BTreeMap<String, Secret>>,
    unavailable: bool,
}

impl MemorySecretStore {
    pub fn unavailable() -> Self {
        Self {
            unavailable: true,
            ..Self::default()
        }
    }

    /// The names it holds, in order.
    pub fn names(&self) -> Vec<String> {
        self.secrets().keys().cloned().collect()
    }

    fn secrets(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Secret>> {
        self.secrets.lock().expect("secret store lock poisoned")
    }

    fn check(&self) -> Result<(), SecretStoreError> {
        if self.unavailable {
            Err(SecretStoreError("no store in this session".into()))
        } else {
            Ok(())
        }
    }
}

impl SecretStore for MemorySecretStore {
    fn read(&self, name: &str) -> Result<Option<Secret>, SecretStoreError> {
        self.check()?;
        Ok(self.secrets().get(name).cloned())
    }

    fn write(&self, name: &str, secret: &Secret) -> Result<(), SecretStoreError> {
        self.check()?;
        self.secrets().insert(name.to_owned(), secret.clone());
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<(), SecretStoreError> {
        self.check()?;
        self.secrets().remove(name);
        Ok(())
    }
}

/// A local HTTP server answering canned responses, for tests of code that
/// talks to a Platform or to GitHub. Routes match the request's path and
/// query, for any method unless one is named; unknown ones answer 404.
pub struct FakeHttpServer {
    url: String,
    routes: Arc<Mutex<BTreeMap<String, VecDeque<Canned>>>>,
    requests: Arc<Mutex<Vec<Request>>>,
}

/// A request the fake server received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// The path with its query.
    pub path: String,
    pub authorization: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone)]
struct Canned {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl FakeHttpServer {
    pub fn start() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let url = format!("http://{}", listener.local_addr().expect("local address"));
        let routes = Arc::new(Mutex::new(BTreeMap::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (served, logged) = (routes.clone(), requests.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (served, logged) = (served.clone(), logged.clone());
                std::thread::spawn(move || answer(stream, &served, &logged));
            }
        });
        Self {
            url,
            routes,
            requests,
        }
    }

    /// `http://127.0.0.1:<port>`, with no slash at the end.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Answers `path` with `status` and `body`, every time.
    pub fn serve(&self, path: &str, status: u16, body: impl Into<Vec<u8>>) {
        self.route(path, status, Vec::new(), body.into(), false);
    }

    /// Answers `method` requests to `path` with `status` and `body`, every
    /// time; other methods get the answers for `path` alone.
    pub fn serve_method(&self, method: &str, path: &str, status: u16, body: impl Into<Vec<u8>>) {
        self.route(
            &format!("{method} {path}"),
            status,
            Vec::new(),
            body.into(),
            false,
        );
    }

    /// Answers `path` with `status` and `body` after the answers already
    /// queued for it; the last answer keeps repeating.
    pub fn then_serve(&self, path: &str, status: u16, body: impl Into<Vec<u8>>) {
        self.route(path, status, Vec::new(), body.into(), true);
    }

    /// Answers `path` with a redirect to `to`.
    pub fn redirect(&self, path: &str, to: &str) {
        self.route(
            path,
            302,
            vec![("Location".into(), to.into())],
            Vec::new(),
            false,
        );
    }

    /// The paths requested so far, in order.
    pub fn requests(&self) -> Vec<String> {
        self.received()
            .into_iter()
            .map(|request| request.path)
            .collect()
    }

    /// Every request received so far, in order.
    pub fn received(&self) -> Vec<Request> {
        self.requests
            .lock()
            .expect("requests lock poisoned")
            .clone()
    }

    fn route(
        &self,
        path: &str,
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        queued: bool,
    ) {
        let canned = Canned {
            status,
            headers,
            body,
        };
        let mut routes = self.routes.lock().expect("routes lock poisoned");
        let answers = routes.entry(path.to_owned()).or_default();
        if !queued {
            answers.clear();
        }
        answers.push_back(canned);
    }
}

fn answer(
    stream: std::net::TcpStream,
    routes: &Mutex<BTreeMap<String, VecDeque<Canned>>>,
    requests: &Mutex<Vec<Request>>,
) {
    use std::io::{BufRead, BufReader, Read, Write};

    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut length = 0;
    let mut authorization = None;
    let mut header = String::new();
    while reader.read_line(&mut header).is_ok_and(|read| read > 2) {
        if let Some((name, value)) = header.split_once(':') {
            let value = value.trim().to_owned();
            if name.eq_ignore_ascii_case("content-length") {
                length = value.parse().unwrap_or(0);
            } else if name.eq_ignore_ascii_case("authorization") {
                authorization = Some(value);
            }
        }
        header.clear();
    }
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let mut words = request_line.split_whitespace();
    let method = words.next().unwrap_or_default().to_owned();
    let path = words.next().unwrap_or_default().to_owned();
    requests
        .lock()
        .expect("requests lock poisoned")
        .push(Request {
            method: method.clone(),
            path: path.clone(),
            authorization,
            body: String::from_utf8_lossy(&body).into_owned(),
        });
    let canned = {
        let mut routes = routes.lock().expect("routes lock poisoned");
        let by_method = format!("{method} {path}");
        let key = if routes.contains_key(&by_method) {
            by_method
        } else {
            path
        };
        match routes.get_mut(&key) {
            Some(answers) if answers.len() > 1 => answers.pop_front(),
            Some(answers) => answers.front().cloned(),
            None => None,
        }
    }
    .unwrap_or(Canned {
        status: 404,
        headers: Vec::new(),
        body: br#"{"message":"Not Found"}"#.to_vec(),
    });
    let mut head = format!(
        "HTTP/1.1 {} Canned\r\nContent-Length: {}\r\nConnection: close\r\n",
        canned.status,
        canned.body.len()
    );
    for (name, value) in &canned.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let mut stream = &stream;
    let _ = stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(&canned.body));
}
