//! What the crawler tests share: the engine at a fake server, on a clock
//! that moves instead of sleeping, and the fixtures written by hand. Each
//! test binary uses only part of it.

#![allow(dead_code)]

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use mascate_integrations::{Crawler, Pause};
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{Currency, Money};
use mascate_platform::testing::FakeHttpServer;
use rust_decimal::Decimal;

/// `name` in the fixtures folder `folder`.
pub fn fixture(folder: &str, name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{folder}/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

pub fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

/// Moves the clock instead of sleeping, and remembers each wait.
pub struct ClockPause {
    clock: Arc<ManualClock>,
    waits: Mutex<Vec<Duration>>,
}

impl Pause for ClockPause {
    fn pause(&self, duration: Duration) {
        self.waits.lock().unwrap().push(duration);
        self.clock.advance(TimeDelta::from_std(duration).unwrap());
    }
}

impl ClockPause {
    pub fn seconds(&self) -> Vec<u64> {
        self.waits
            .lock()
            .unwrap()
            .iter()
            .map(Duration::as_secs)
            .collect()
    }
}

/// A fake server serving `robots` as its robots.txt.
pub struct Site {
    pub server: FakeHttpServer,
    pub clock: Arc<ManualClock>,
    pub pauses: Arc<ClockPause>,
    pub engine: Arc<Crawler>,
}

/// The engine, as the Mascate, at a fake server serving `robots`, on
/// 2026-10-16 12:00 UTC.
pub fn site(robots: String) -> Site {
    let server = FakeHttpServer::start();
    server.serve("/robots.txt", 200, robots);
    let clock = Arc::new(ManualClock::at(
        Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
    ));
    let pauses = Arc::new(ClockPause {
        clock: clock.clone(),
        waits: Mutex::new(Vec::new()),
    });
    let engine = Crawler::new("Mascate/0.5.0", clock.clone()).with_pause(pauses.clone());
    Site {
        server,
        clock,
        pauses,
        engine: Arc::new(engine),
    }
}

/// The pages requested, robots.txt aside.
pub fn pages(server: &FakeHttpServer) -> Vec<String> {
    server
        .requests()
        .into_iter()
        .filter(|path| path != "/robots.txt")
        .collect()
}
