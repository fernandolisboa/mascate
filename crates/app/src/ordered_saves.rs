//! Saves of one choice the owner can change faster than the disk keeps up.

use std::sync::Arc;

use futures::lock::Mutex;
use gpui_kit::{BackgroundExecutor, Task};

/// Runs saves one at a time in the order the owner made the choices; a save
/// that a newer one overtook is skipped and reads as saved, so an older
/// choice never lands over a newer one.
#[derive(Default)]
pub struct OrderedSaves {
    asked: u64,
    /// The newest save number written, held while writing.
    written: Arc<Mutex<u64>>,
}

impl OrderedSaves {
    pub fn save<F>(&mut self, save: F, executor: &BackgroundExecutor) -> Task<Result<(), String>>
    where
        F: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.asked += 1;
        let number = self.asked;
        let written = self.written.clone();
        executor.spawn(async move {
            let mut written = written.lock().await;
            if *written > number {
                return Ok(());
            }
            *written = number;
            save.await
        })
    }
}
