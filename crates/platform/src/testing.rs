//! Doubles for other crates' tests: a [`SecretStore`] in memory and a fake
//! HTTP server.

use std::collections::BTreeMap;
use std::sync::Mutex;

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
/// talks to a Platform or to GitHub. Unknown paths answer 404.
pub struct FakeHttpServer {
    url: String,
    routes: std::sync::Arc<Mutex<BTreeMap<String, Canned>>>,
    requests: std::sync::Arc<Mutex<Vec<String>>>,
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
        let routes = std::sync::Arc::new(Mutex::new(BTreeMap::<String, Canned>::new()));
        let requests = std::sync::Arc::new(Mutex::new(Vec::new()));
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

    /// Answers `path` with `status` and `body`.
    pub fn serve(&self, path: &str, status: u16, body: impl Into<Vec<u8>>) {
        self.route(path, status, Vec::new(), body.into());
    }

    /// Answers `path` with a redirect to `to`.
    pub fn redirect(&self, path: &str, to: &str) {
        self.route(path, 302, vec![("Location".into(), to.into())], Vec::new());
    }

    /// The paths requested so far, in order.
    pub fn requests(&self) -> Vec<String> {
        self.requests
            .lock()
            .expect("requests lock poisoned")
            .clone()
    }

    fn route(&self, path: &str, status: u16, headers: Vec<(String, String)>, body: Vec<u8>) {
        self.routes.lock().expect("routes lock poisoned").insert(
            path.to_owned(),
            Canned {
                status,
                headers,
                body,
            },
        );
    }
}

fn answer(
    stream: std::net::TcpStream,
    routes: &Mutex<BTreeMap<String, Canned>>,
    requests: &Mutex<Vec<String>>,
) {
    use std::io::{BufRead, BufReader, Write};

    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    // Skips the headers; the fake serves GETs, which have no body.
    let mut header = String::new();
    while reader.read_line(&mut header).is_ok_and(|read| read > 2) {
        header.clear();
    }
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    requests
        .lock()
        .expect("requests lock poisoned")
        .push(path.clone());
    let canned = routes
        .lock()
        .expect("routes lock poisoned")
        .get(&path)
        .cloned()
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
