//! What the panel reader tests share: a fake DevTools endpoint standing in
//! for the installed browser. Its HTTP side opens and closes tabs; each
//! tab's WebSocket answers the commands the session sends, fires the page's
//! load and answers each evaluation with the next value queued, the last
//! one repeating. It runs no JavaScript: a value is what the script would
//! have read on the page. Each test binary uses only part of it.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mascate_integrations::Pause;
use mascate_platform::testing::FakeHttpServer;
use serde_json::{Value, json};
use tungstenite::Message;

/// `name` in the fixtures folder `folder`, as JSON.
pub fn fixture(folder: &str, name: &str) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/tests/fixtures/{folder}/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap()
}

/// One command a tab received.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub method: String,
    pub params: Value,
}

#[derive(Default)]
struct Script {
    values: VecDeque<Value>,
    /// Whether the first value queued already answered once.
    answered: bool,
    navigation_error: Option<String>,
    commands: Vec<Command>,
}

pub struct FakeDevTools {
    http: FakeHttpServer,
    script: Arc<Mutex<Script>>,
}

impl FakeDevTools {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let tab = format!("ws://{}/devtools/page/T1", listener.local_addr().unwrap());
        let script = Arc::new(Mutex::new(Script::default()));
        let shared = script.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let script = shared.clone();
                std::thread::spawn(move || answer(stream, &script));
            }
        });
        let http = FakeHttpServer::start();
        http.serve_any_query(
            "/json/new",
            200,
            json!({ "id": "T1", "type": "page", "url": "about:blank", "webSocketDebuggerUrl": tab })
                .to_string(),
        );
        http.serve("/json/close/T1", 200, "Target is closing");
        Self { http, script }
    }

    /// `http://127.0.0.1:<port>`, as a browser's DevTools endpoint.
    pub fn endpoint(&self) -> &str {
        self.http.url()
    }

    /// Answers the next evaluation with `value`; the last one queued keeps
    /// answering.
    pub fn then_read(&self, value: Value) {
        self.script.lock().unwrap().values.push_back(value);
    }

    /// Answers every navigation with `error`, as a page that did not open.
    pub fn fail_navigation(&self, error: &str) {
        self.script.lock().unwrap().navigation_error = Some(error.to_owned());
    }

    /// Every command the tabs received, in order.
    pub fn commands(&self) -> Vec<Command> {
        self.script.lock().unwrap().commands.clone()
    }

    /// The methods of every command, in order.
    pub fn methods(&self) -> Vec<String> {
        self.commands()
            .into_iter()
            .map(|command| command.method)
            .collect()
    }

    /// The addresses the tabs were sent to.
    pub fn navigations(&self) -> Vec<String> {
        self.commands()
            .into_iter()
            .filter(|command| command.method == "Page.navigate")
            .map(|command| command.params["url"].as_str().unwrap().to_owned())
            .collect()
    }

    /// The requests to the HTTP side: tabs opened and closed.
    pub fn tab_requests(&self) -> Vec<String> {
        self.http
            .received()
            .into_iter()
            .map(|request| format!("{} {}", request.method, request.path))
            .collect()
    }
}

fn answer(stream: std::net::TcpStream, script: &Mutex<Script>) {
    let Ok(mut socket) = tungstenite::accept(stream) else {
        return;
    };
    while let Ok(message) = socket.read() {
        let Message::Text(text) = message else {
            continue;
        };
        let command: Value = serde_json::from_str(text.as_str()).unwrap();
        let id = command["id"].clone();
        let method = command["method"].as_str().unwrap().to_owned();
        let mut script = script.lock().unwrap();
        script.commands.push(Command {
            method: method.clone(),
            params: command["params"].clone(),
        });
        let (result, event) = match method.as_str() {
            "Page.navigate" => match &script.navigation_error {
                Some(error) => (json!({ "frameId": "F1", "errorText": error }), None),
                None => (
                    json!({ "frameId": "F1", "loaderId": "L1" }),
                    Some(
                        json!({ "method": "Page.loadEventFired", "params": { "timestamp": 1.0 } }),
                    ),
                ),
            },
            "Runtime.evaluate" => {
                if script.answered && script.values.len() > 1 {
                    script.values.pop_front();
                }
                script.answered = true;
                let value = script.values.front().cloned();
                (
                    json!({ "result": { "type": "object", "value": value.unwrap_or(Value::Null) } }),
                    None,
                )
            }
            _ => (json!({}), None),
        };
        drop(script);
        let reply = json!({ "id": id, "result": result }).to_string();
        if socket.send(Message::text(reply)).is_err() {
            return;
        }
        if let Some(event) = event
            && socket.send(Message::text(event.to_string())).is_err()
        {
            return;
        }
    }
}

/// Remembers each wait instead of sleeping.
#[derive(Default)]
pub struct RecordedPauses(Mutex<Vec<Duration>>);

impl Pause for RecordedPauses {
    fn pause(&self, duration: Duration) {
        self.0.lock().unwrap().push(duration);
    }
}

impl RecordedPauses {
    pub fn count(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}
