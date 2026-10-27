//! The owner's interface choices while the app runs, and saving them to the
//! app platform module.

use std::sync::Arc;

use gpui_kit::{App, Global, Task};
use mascate_kernel::{SystemClock, UuidV7Generator};
use mascate_platform::{Appearance, Database, save_appearance, save_work_context};

use crate::ordered_saves::OrderedSaves;
use crate::work_context::WorkContext;

/// Why a choice was not saved when the database did not open.
const KEPT_UNTIL_CLOSED: &str =
    "O banco de dados não abriu, então a escolha vale só até fechar o app.";

pub struct Preferences {
    /// `None` when the database could not open: choices then last until the
    /// app closes.
    database: Option<Arc<Database>>,
    appearance: Appearance,
    saves: OrderedSaves,
    work_context: WorkContext,
    context_saves: OrderedSaves,
}

impl Global for Preferences {}

impl Preferences {
    pub fn new(
        database: Option<Arc<Database>>,
        appearance: Appearance,
        work_context: WorkContext,
    ) -> Self {
        Self {
            database,
            appearance,
            saves: OrderedSaves::default(),
            work_context,
            context_saves: OrderedSaves::default(),
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
        return Task::ready(Err(KEPT_UNTIL_CLOSED.into()));
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

/// The Work Context every screen follows.
pub fn work_context(cx: &App) -> WorkContext {
    cx.try_global::<Preferences>()
        .map_or_else(WorkContext::default, |preferences| preferences.work_context)
}

/// Keeps `context` for this run and saves it in the background; the task
/// tells why saving failed, if it did.
pub fn set_work_context(context: WorkContext, cx: &mut App) -> Task<Result<(), String>> {
    let executor = cx.background_executor().clone();
    let preferences = cx.global_mut::<Preferences>();
    preferences.work_context = context;
    let Some(database) = preferences.database.clone() else {
        return Task::ready(Err(KEPT_UNTIL_CLOSED.into()));
    };
    preferences.context_saves.save(
        async move {
            save_work_context(&database, &SystemClock, &UuidV7Generator, context.code())
                .await
                .map_err(|error| format!("Não consegui salvar o contexto de trabalho: {error}"))
        },
        &executor,
    )
}
