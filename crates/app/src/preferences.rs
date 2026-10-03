//! The owner's interface choices while the app runs, and saving them to the
//! app platform module.

use std::sync::Arc;

use futures::lock::Mutex;
use gpui_kit::{App, Global, Task};
use mascate_kernel::{SystemClock, UuidV7Generator};
use mascate_platform::{Appearance, Database, save_appearance};

pub struct Preferences {
    /// `None` when the database could not open: choices then last until the
    /// app closes.
    database: Option<Arc<Database>>,
    appearance: Appearance,
    /// Numbers each save in the order the owner chose.
    saves: u64,
    /// The newest save number written, held while writing so saves run one
    /// at a time and an older one never lands over a newer one.
    written: Arc<Mutex<u64>>,
}

impl Global for Preferences {}

impl Preferences {
    pub fn new(database: Option<Arc<Database>>, appearance: Appearance) -> Self {
        Self {
            database,
            appearance,
            saves: 0,
            written: Arc::default(),
        }
    }
}

pub fn appearance(cx: &App) -> Appearance {
    cx.global::<Preferences>().appearance
}

/// Keeps `appearance` for this run and saves it in the background; the task
/// tells why saving failed, if it did. A save that a newer one overtook is
/// skipped and reads as saved.
pub fn set_appearance(appearance: Appearance, cx: &mut App) -> Task<Result<(), String>> {
    let preferences = cx.global_mut::<Preferences>();
    preferences.appearance = appearance;
    let Some(database) = preferences.database.clone() else {
        return Task::ready(Err(
            "O banco de dados não abriu, então a escolha vale só até fechar o app.".into(),
        ));
    };
    preferences.saves += 1;
    let save = preferences.saves;
    let written = preferences.written.clone();
    cx.background_executor().spawn(async move {
        let mut written = written.lock().await;
        if *written > save {
            return Ok(());
        }
        *written = save;
        save_appearance(&database, &SystemClock, &UuidV7Generator, appearance)
            .await
            .map_err(|error| format!("Não consegui salvar a aparência: {error}"))
    })
}
