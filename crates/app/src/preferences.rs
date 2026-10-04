//! The owner's interface choices while the app runs, and saving them to the
//! app platform module.

use std::sync::Arc;

use gpui_kit::{App, Global, Task};
use mascate_kernel::{SystemClock, UuidV7Generator};
use mascate_platform::{Appearance, Database, save_appearance};

use crate::ordered_saves::OrderedSaves;

pub struct Preferences {
    /// `None` when the database could not open: choices then last until the
    /// app closes.
    database: Option<Arc<Database>>,
    appearance: Appearance,
    saves: OrderedSaves,
}

impl Global for Preferences {}

impl Preferences {
    pub fn new(database: Option<Arc<Database>>, appearance: Appearance) -> Self {
        Self {
            database,
            appearance,
            saves: OrderedSaves::default(),
        }
    }
}

pub fn appearance(cx: &App) -> Appearance {
    cx.global::<Preferences>().appearance
}

/// Keeps `appearance` for this run and saves it in the background; the task
/// tells why saving failed, if it did.
pub fn set_appearance(appearance: Appearance, cx: &mut App) -> Task<Result<(), String>> {
    let executor = cx.background_executor().clone();
    let preferences = cx.global_mut::<Preferences>();
    preferences.appearance = appearance;
    let Some(database) = preferences.database.clone() else {
        return Task::ready(Err(
            "O banco de dados não abriu, então a escolha vale só até fechar o app.".into(),
        ));
    };
    preferences.saves.save(
        async move {
            save_appearance(&database, &SystemClock, &UuidV7Generator, appearance)
                .await
                .map_err(|error| format!("Não consegui salvar a aparência: {error}"))
        },
        &executor,
    )
}
