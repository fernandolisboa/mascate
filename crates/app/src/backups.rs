//! Settings › Backup (#6): where Backups go and how many stay, a Backup on
//! demand, export, restore, also when the database does not open (#8), the
//! comparison with another machine's database (#117), the merge of it and
//! the merges applied before (#118).
//! The daily Backup runs in the background while the app is open or in the
//! tray. The rules live in the platform module.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::Local;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState, NumberInput};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, Global, PathPromptOptions, Subscription, Window, div, px};
use mascate_platform::{
    AppliedMerge, Backup, BackupError, BackupSettings, Backups, MAX_KEEP, MergeError, MergeImport,
    MergeReport, RestoreError, stage_restore_without_backup,
};

use crate::appearance::look;
use crate::catalog::catalog;
use crate::connections::AppConnections;
use crate::forms::{self, Outcome};
use crate::kit;
use crate::merge_import::{self, MergeFailed};
use crate::ordered_saves::OrderedSaves;
use crate::startup::MODULE_MIGRATIONS;
use crate::updates;

/// How often the app checks whether the daily Backup is due.
const DAILY_CHECK: Duration = Duration::from_secs(60 * 60);

/// The app's Backups and what became of a restore staged before this start;
/// absent when the database did not open.
pub struct AppBackups {
    pub backups: Arc<Backups>,
    pub merge_import: Arc<MergeImport>,
    pub settings: BackupSettings,
    pub restore: Option<Result<(), String>>,
    /// The Merge Import applied before this start, until Settings opens it.
    pub merged: Option<AppliedMerge>,
    /// Why the last daily Backup failed, until one succeeds.
    pub daily_failure: Arc<Mutex<Option<String>>>,
}

impl Global for AppBackups {}

/// The database file when it did not open: a restore can still replace it.
pub struct UnopenedDatabase(pub PathBuf);

impl Global for UnopenedDatabase {}

/// Takes the daily Backup when due, now and every hour after, for as long
/// as the app runs, window open or not.
pub fn start_daily(cx: &App) {
    let Some(app) = cx.try_global::<AppBackups>() else {
        return;
    };
    let backups = app.backups.clone();
    let failure = app.daily_failure.clone();
    let executor = cx.background_executor().clone();
    cx.background_executor()
        .spawn(async move {
            loop {
                let outcome = backups.back_up_if_due().await;
                *failure.lock().unwrap_or_else(PoisonError::into_inner) =
                    outcome.err().map(backup_failed);
                executor.timer(DAILY_CHECK).await;
            }
        })
        .detach();
}

/// What became of the restore staged before this start, for the home screen.
pub fn restore_notice(cx: &App) -> Option<AnyElement> {
    let outcome = cx.try_global::<AppBackups>()?.restore.clone()?;
    Some(match outcome {
        Ok(()) => kit::success_notice(
            "Banco restaurado. O banco de antes está guardado num Backup.",
            cx,
        )
        .into_any_element(),
        Err(error) => kit::error_notice(error, cx).into_any_element(),
    })
}

fn when(backup: &Backup) -> String {
    backup
        .taken_at
        .with_timezone(&Local)
        .format("%d/%m/%Y às %H:%M")
        .to_string()
}

fn backup_failed(error: BackupError) -> String {
    format!("Não consegui fazer o Backup: {error}")
}

/// Why `action` refused a file the Restore's check turned down; `Err` when
/// the check itself failed.
fn file_refused(error: RestoreError, action: &str) -> Result<String, BackupError> {
    match error {
        RestoreError::NotMascate(path) => {
            Ok(format!("{} não é um banco do Mascate.", path.display()))
        }
        RestoreError::Damaged { path, .. } => Ok(format!(
            "{} está danificado; escolha outro arquivo.",
            path.display()
        )),
        RestoreError::NewerThanApp { path, .. } => Ok(format!(
            "{} veio de uma versão mais nova do Mascate; atualize o app antes de {action}.",
            path.display()
        )),
        RestoreError::Backup(error) => Err(error),
    }
}

fn restore_refused(error: RestoreError) -> String {
    file_refused(error, "restaurar").unwrap_or_else(backup_failed)
}

/// Why comparing or merging, as `action` names it, did not happen.
fn merge_refused(error: MergeError, action: &str) -> String {
    match error {
        MergeError::Refused(error) => file_refused(error, action)
            .unwrap_or_else(|error| format!("Não consegui ler o outro banco: {error}")),
        MergeError::OwnDatabase(_) => {
            "Esse arquivo é o próprio banco do app; escolha o do outro computador.".to_string()
        }
        MergeError::Backup(error) => {
            format!("Não consegui fazer o Backup antes de mesclar, então nada mudou: {error}")
        }
        error => format!("Não consegui {action}: {error}"),
    }
}

fn merge_failed(failed: MergeFailed) -> String {
    match failed {
        MergeFailed::Products(error) => {
            format!("Não consegui ler os produtos antes de mesclar, então nada mudou: {error}")
        }
        MergeFailed::Merge(error @ (MergeError::Refused(_) | MergeError::Backup(_))) => {
            merge_refused(error, "mesclar")
        }
        MergeFailed::Merge(error) => format!(
            "{} O banco ficou como estava antes.",
            merge_refused(error, "mesclar")
        ),
    }
}

/// A finished comparison, shown until the owner goes back.
struct Compared {
    file: PathBuf,
    report: MergeReport,
}

/// The report open in place of the section.
enum Opened {
    Compared(Compared),
    Merged(AppliedMerge),
}

pub struct BackupSection {
    /// The settings as last chosen on this screen.
    settings: BackupSettings,
    keep: Entity<InputState>,
    newest: Option<Backup>,
    /// The file waiting for the owner to confirm the restore.
    confirming: Option<PathBuf>,
    open: Option<Opened>,
    /// The open comparison waits for the owner to confirm the merge.
    confirming_merge: bool,
    /// The merges applied here, newest first.
    merges: Vec<AppliedMerge>,
    busy: bool,
    /// Settings saves still on their way to the database.
    saving: usize,
    outcome: Option<Outcome>,
    saves: OrderedSaves,
    _subscriptions: Vec<Subscription>,
}

impl BackupSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = cx
            .try_global::<AppBackups>()
            .map(|app| app.settings.clone())
            .unwrap_or_else(|| BackupSettings {
                folder: PathBuf::new(),
                keep: mascate_platform::DEFAULT_KEEP,
            });
        let keep = cx.new(|cx| {
            InputState::new(window, cx)
                .min(1.)
                .max(f64::from(MAX_KEEP))
                .validate(|text, _| text.chars().all(|c| c.is_ascii_digit()) && text.len() <= 3)
                .default_value(settings.keep.to_string())
        });
        let subscriptions = vec![cx.subscribe(&keep, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let text = input.read(cx).value();
                if let Ok(keep) = text.parse::<u16>()
                    && (1..=MAX_KEEP).contains(&keep)
                {
                    this.save(
                        BackupSettings {
                            keep,
                            ..this.settings.clone()
                        },
                        cx,
                    );
                }
            }
        })];
        // The app restarted after a merge: it opens on its report.
        let merged = cx
            .has_global::<AppBackups>()
            .then(|| cx.global_mut::<AppBackups>().merged.take())
            .flatten();
        let mut section = Self {
            settings,
            keep,
            newest: None,
            confirming: None,
            open: merged.map(Opened::Merged),
            confirming_merge: false,
            merges: Vec::new(),
            busy: false,
            saving: 0,
            outcome: None,
            saves: OrderedSaves::default(),
            _subscriptions: subscriptions,
        };
        section.read_newest(cx);
        section.read_merges(cx);
        section
    }

    fn read_merges(&mut self, cx: &mut Context<Self>) {
        let Some(merge_import) = Self::merge_import(cx) else {
            return;
        };
        let reading = cx
            .background_executor()
            .spawn(async move { merge_import.merges().await });
        cx.spawn(async move |this, cx| {
            let read = reading.await;
            let _ = this.update(cx, |this, cx| {
                match read {
                    Ok(merges) => this.merges = merges,
                    Err(error) => {
                        this.outcome = Some(Outcome::Failed(
                            format!("Não consegui ler as mesclagens feitas: {error}").into(),
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn merge_import(cx: &App) -> Option<Arc<MergeImport>> {
        cx.try_global::<AppBackups>()
            .map(|app| app.merge_import.clone())
    }

    /// The title of the report open in place of the section, if one is.
    pub fn report_title(&self) -> Option<String> {
        Some(match self.open.as_ref()? {
            Opened::Compared(compared) => {
                format!("Comparação com {}", merge_import::file_name(&compared.file))
            }
            Opened::Merged(applied) => {
                format!("Mesclagem de {}", merge_import::file_name(&applied.file))
            }
        })
    }

    pub fn close_report(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        self.confirming_merge = false;
        self.outcome = None;
        self.read_merges(cx);
        cx.notify();
    }

    fn backups(cx: &App) -> Option<Arc<Backups>> {
        cx.try_global::<AppBackups>().map(|app| app.backups.clone())
    }

    fn read_newest(&mut self, cx: &mut Context<Self>) {
        let Some(backups) = Self::backups(cx) else {
            return;
        };
        let reading = cx
            .background_executor()
            .spawn(async move { backups.list().await });
        cx.spawn(async move |this, cx| {
            let listed = reading.await;
            let _ = this.update(cx, |this, cx| {
                match listed {
                    Ok(listed) => this.newest = listed.into_iter().next(),
                    Err(error) => {
                        this.outcome = Some(Outcome::Failed(
                            format!("Não consegui ler a pasta de Backups: {error}").into(),
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save(&mut self, settings: BackupSettings, cx: &mut Context<Self>) {
        let Some(backups) = Self::backups(cx) else {
            return;
        };
        if settings == self.settings {
            return;
        }
        self.settings = settings.clone();
        cx.global_mut::<AppBackups>().settings = settings.clone();
        let saving = self.saves.save(
            async move {
                backups
                    .save_settings(&settings)
                    .await
                    .map_err(|error| format!("Não consegui salvar as opções de Backup: {error}"))
            },
            cx.background_executor(),
        );
        self.saving += 1;
        cx.spawn(async move |this, cx| {
            let saved = saving.await;
            let _ = this.update(cx, |this, cx| {
                this.saving -= 1;
                if let Err(error) = saved {
                    this.outcome = Some(Outcome::Failed(error.into()));
                }
                // Reads the folder now saved, not the one before.
                this.read_newest(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Working, or settings not saved yet: actions would use the old folder.
    fn blocked(&self) -> bool {
        self.busy || self.saving > 0
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
        let picking = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Usar esta pasta".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = picking.await;
            let _ = this.update(cx, |this, cx| match picked {
                Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                    let folder = paths.swap_remove(0);
                    this.save(
                        BackupSettings {
                            folder,
                            ..this.settings.clone()
                        },
                        cx,
                    );
                }
                Ok(Err(error)) => {
                    this.outcome = Some(Outcome::Failed(
                        format!("Não consegui abrir o seletor de pastas: {error}").into(),
                    ));
                    cx.notify();
                }
                _ => {}
            });
        })
        .detach();
    }

    fn back_up_now(&mut self, cx: &mut Context<Self>) {
        self.run(cx, |backups| async move {
            backups
                .back_up_now()
                .await
                .map(|backup| format!("Backup feito em {}.", backup.path.display()))
                .map_err(backup_failed)
        });
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        let suggested = format!("mascate-{}.db", Local::now().format("%Y-%m-%d"));
        let picking = cx.prompt_for_new_path(&self.settings.folder, Some(&suggested));
        cx.spawn(async move |this, cx| {
            let picked = picking.await;
            let _ = this.update(cx, |this, cx| match picked {
                Ok(Ok(Some(to))) => this.run(cx, move |backups| async move {
                    backups
                        .export(&to)
                        .await
                        .map(|()| format!("Banco exportado para {}.", to.display()))
                        .map_err(|error| match error {
                            BackupError::DatabaseFile(_) => {
                                "Esse arquivo é o próprio banco do app; escolha outro nome."
                                    .to_string()
                            }
                            error => format!("Não consegui exportar: {error}"),
                        })
                }),
                Ok(Err(error)) => {
                    this.outcome = Some(Outcome::Failed(
                        format!("Não consegui abrir a janela de salvar: {error}").into(),
                    ));
                    cx.notify();
                }
                _ => {}
            });
        })
        .detach();
    }

    fn choose_restore(&mut self, cx: &mut Context<Self>) {
        let picking = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Restaurar este arquivo".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = picking.await;
            let _ = this.update(cx, |this, cx| match picked {
                Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                    this.confirming = Some(paths.swap_remove(0));
                    this.outcome = None;
                    cx.notify();
                }
                Ok(Err(error)) => {
                    this.outcome = Some(Outcome::Failed(
                        format!("Não consegui abrir o seletor de arquivos: {error}").into(),
                    ));
                    cx.notify();
                }
                _ => {}
            });
        })
        .detach();
    }

    fn choose_compare(&mut self, cx: &mut Context<Self>) {
        let picking = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Comparar com este banco".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = picking.await;
            let _ = this.update(cx, |this, cx| match picked {
                Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                    this.compare(paths.swap_remove(0), cx);
                }
                Ok(Err(error)) => {
                    this.outcome = Some(Outcome::Failed(
                        format!("Não consegui abrir o seletor de arquivos: {error}").into(),
                    ));
                    cx.notify();
                }
                _ => {}
            });
        })
        .detach();
    }

    /// Compares `file` with this database off the UI thread, then shows the
    /// report. Nothing is written to either database.
    fn compare(&mut self, file: PathBuf, cx: &mut Context<Self>) {
        let Some(merge_import) = Self::merge_import(cx) else {
            return;
        };
        self.busy = true;
        self.outcome = None;
        self.confirming = None;
        let comparing = cx.background_executor().spawn({
            let file = file.clone();
            async move { merge_import.compare(&file).await }
        });
        cx.spawn(async move |this, cx| {
            let compared = comparing.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match compared {
                    Ok(report) => {
                        this.open = Some(Opened::Compared(Compared { file, report }));
                        this.confirming_merge = false;
                    }
                    Err(error) => {
                        this.outcome =
                            Some(Outcome::Failed(merge_refused(error, "comparar").into()));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Merges the compared file after a Backup, then restarts the app,
    /// which opens on the merge's report.
    fn merge(&mut self, cx: &mut Context<Self>) {
        let Some(Opened::Compared(compared)) = &self.open else {
            return;
        };
        let file = compared.file.clone();
        let (Some(app), Some(catalog), Some(connections)) = (
            cx.try_global::<AppBackups>(),
            catalog(cx),
            cx.try_global::<AppConnections>().map(|app| app.0.clone()),
        ) else {
            return;
        };
        let (merge_import, backups) = (app.merge_import.clone(), app.backups.clone());
        self.busy = true;
        self.outcome = None;
        let merging = cx.background_executor().spawn(async move {
            merge_import::merge(&file, &merge_import, &backups, &catalog, &connections).await
        });
        cx.spawn(async move |this, cx| {
            let merged = merging.await;
            let _ = this.update(cx, |this, cx| {
                match merged {
                    Ok(Some(_)) => return updates::restart(cx),
                    Ok(None) => {
                        if let Some(Opened::Compared(compared)) = &mut this.open {
                            compared.report = MergeReport::default();
                        }
                        this.outcome = Some(Outcome::Done(
                            "Com o banco como está agora, o outro banco não traz nada para gravar \
                             aqui: nada mudou."
                                .into(),
                        ));
                    }
                    Err(failed) => {
                        this.outcome = Some(Outcome::Failed(merge_failed(failed).into()));
                    }
                }
                this.busy = false;
                this.confirming_merge = false;
                this.read_newest(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn render_comparison(&self, compared: &Compared, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let actions = if !compared.report.brings_changes() {
            (!compared.report.is_empty()).then(|| {
                kit::info_notice(
                    "Mesclar não mudaria nada aqui: ficam as versões deste computador, e o que \
                     fica de fora não entra.",
                    cx,
                )
                .into_any_element()
            })
        } else if self.confirming_merge {
            Some(self.render_merge_confirmation(&compared.file, cx))
        } else {
            Some(
                h_flex()
                    .child(
                        Button::new("merge-database")
                            .label("Mesclar…")
                            .primary()
                            .small()
                            .disabled(self.blocked())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirming_merge = true;
                                this.outcome = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            )
        };
        v_flex()
            .gap_3()
            .child(div().text_sm().text_color(t.text2).child(format!(
                "O que um Merge Import traria de {}. Vence a alteração mais recente de cada \
                 registro, e os registros diferentes nos dois lados aparecem como conflito. \
                 Comparar não grava nada em nenhum dos dois bancos.",
                compared.file.display()
            )))
            .children(actions)
            .when_some(self.outcome.as_ref(), |view, outcome| {
                view.child(forms::notice(outcome, cx))
            })
            .child(merge_import::report_view(&compared.report, cx))
            .into_any_element()
    }

    fn render_merge_confirmation(&self, file: &Path, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.accent)
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .child(format!("Mesclar {} neste banco?", file.display())),
            )
            .child(div().text_sm().text_color(t.text2).child(
                "Antes, o app faz um Backup do banco atual. Depois compara de novo, com o banco \
                 como estiver agora, grava tudo de uma vez e reinicia já no relatório. O que \
                 fica de fora não entra; o resto entra.",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("confirm-merge")
                            .label("Mesclar e reiniciar")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.blocked())
                            .on_click(cx.listener(|this, _, _, cx| this.merge(cx))),
                    )
                    .child(
                        Button::new("cancel-merge")
                            .label("Cancelar")
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirming_merge = false;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_merges(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.merges.is_empty() {
            return None;
        }
        let t = look(cx).tokens;
        let rows: Vec<AnyElement> = self
            .merges
            .iter()
            .enumerate()
            .map(|(at, applied)| {
                let open = applied.clone();
                h_flex()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_sm()
                            .child(merge_import::merge_line(applied)),
                    )
                    .child(
                        Button::new(("open-merge", at))
                            .label("Abrir")
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open = Some(Opened::Merged(open.clone()));
                                this.outcome = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        Some(
            v_flex()
                .gap_2()
                .child(div().h_2())
                .child(kit::section_heading("Mesclagens"))
                .child(
                    div().text_sm().text_color(t.text2).child(
                        "Os Merge Imports feitos neste computador, o mais recente primeiro.",
                    ),
                )
                .children(rows)
                .into_any_element(),
        )
    }

    /// Checks and stages the confirmed file, then restarts the app, which
    /// swaps it in before opening the database. A database that does not
    /// open has no Backup taken first: it stays beside the restored one.
    fn restore(&mut self, cx: &mut Context<Self>) {
        let Some(file) = self.confirming.take() else {
            return;
        };
        let backups = Self::backups(cx);
        let unopened = cx
            .try_global::<UnopenedDatabase>()
            .map(|unopened| unopened.0.clone());
        let staging = match (backups, unopened) {
            (Some(backups), _) => cx
                .background_executor()
                .spawn(async move { backups.stage_restore(&file).await.map(|_| ()) }),
            (None, Some(database)) => cx.background_executor().spawn(async move {
                stage_restore_without_backup(&database, MODULE_MIGRATIONS, &file).await
            }),
            (None, None) => return,
        };
        self.busy = true;
        self.outcome = None;
        cx.spawn(async move |this, cx| {
            let staged = staging.await;
            let _ = this.update(cx, |this, cx| match staged {
                Ok(()) => updates::restart(cx),
                Err(error) => {
                    this.busy = false;
                    this.outcome = Some(Outcome::Failed(restore_refused(error).into()));
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Runs one action off the UI thread, then shows how it went and reads
    /// the newest Backup again.
    fn run<F, Fut>(&mut self, cx: &mut Context<Self>, action: F)
    where
        F: FnOnce(Arc<Backups>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<String, String>> + Send + 'static,
    {
        let Some(backups) = Self::backups(cx) else {
            return;
        };
        self.busy = true;
        self.outcome = None;
        let working = cx.background_executor().spawn(action(backups));
        cx.spawn(async move |this, cx| {
            let outcome = working.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.outcome = Some(match outcome {
                    Ok(done) => Outcome::Done(done.into()),
                    Err(failed) => Outcome::Failed(failed.into()),
                });
                this.read_newest(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn render_restore_confirmation(&self, file: &Path, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.danger)
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .child(format!("Restaurar {}?", file.display())),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(t.text2)
                    .child(if cx.has_global::<AppBackups>() {
                        "O banco atual é substituído por este arquivo. Antes, o app faz um \
                     Backup do banco atual; depois reinicia já com o arquivo restaurado."
                    } else {
                        "O banco que não abriu é substituído por este arquivo e fica guardado \
                     ao lado dele, como mascate.db.replaced; o app reinicia já com o arquivo \
                     restaurado."
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("confirm-restore")
                            .label("Restaurar e reiniciar")
                            .danger()
                            .small()
                            .disabled(self.blocked())
                            .on_click(cx.listener(|this, _, _, cx| this.restore(cx))),
                    )
                    .child(
                        Button::new("cancel-restore")
                            .label("Cancelar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirming = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for BackupSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let intro = div().text_sm().text_color(t.text2).child(
            "Uma cópia do banco por dia na pasta escolhida, guardando as mais recentes. \
             Senhas e chaves ficam no cofre do sistema e nunca entram no Backup.",
        );
        if cx.try_global::<AppBackups>().is_none() {
            let section = v_flex().gap_3().child(intro);
            if !cx.has_global::<UnopenedDatabase>() {
                return section.child(kit::error_notice(
                    "O banco de dados não abriu, então não há o que copiar.",
                    cx,
                ));
            }
            return section
                .child(kit::error_notice(
                    "O banco de dados não abriu, então não há o que copiar. Dá para trocá-lo \
                     por um Backup ou export do Mascate.",
                    cx,
                ))
                .child(
                    h_flex().child(
                        Button::new("restore-database")
                            .label("Restaurar…")
                            .outline()
                            .small()
                            .disabled(self.blocked())
                            .on_click(cx.listener(|this, _, _, cx| this.choose_restore(cx))),
                    ),
                )
                .when_some(self.confirming.clone(), |section, file| {
                    section.child(self.render_restore_confirmation(&file, cx))
                })
                .when_some(self.outcome.as_ref(), |section, outcome| {
                    section.child(match outcome {
                        Outcome::Done(text) => kit::success_notice(text.clone(), cx),
                        Outcome::Failed(text) => kit::error_notice(text.clone(), cx),
                    })
                });
        }

        match &self.open {
            Some(Opened::Compared(compared)) => {
                return v_flex().child(self.render_comparison(compared, cx));
            }
            Some(Opened::Merged(applied)) => {
                return v_flex().child(merge_import::applied_view(applied, cx));
            }
            None => {}
        }

        let label = |text: &'static str| {
            div()
                .w(px(140.))
                .flex_none()
                .text_sm()
                .text_color(t.text2)
                .child(text)
        };
        let folder = h_flex()
            .gap_3()
            .flex_wrap()
            .items_center()
            .child(label("Pasta"))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_sm()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(self.settings.folder.display().to_string()),
            )
            .child(
                Button::new("choose-backup-folder")
                    .label("Escolher pasta…")
                    .outline()
                    .small()
                    .disabled(self.blocked())
                    .on_click(cx.listener(|this, _, _, cx| this.choose_folder(cx))),
            );
        let keep = h_flex()
            .gap_3()
            .items_center()
            .child(label("Guardar os últimos"))
            .child(
                div()
                    .w(px(120.))
                    .child(NumberInput::new(&self.keep).small()),
            )
            .child(div().text_sm().text_color(t.text2).child("Backups"));

        let daily_failure = cx
            .global::<AppBackups>()
            .daily_failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let newest = match &self.newest {
            Some(backup) => format!(
                "Último Backup em {}: {}",
                when(backup),
                backup.path.display()
            ),
            None => "Nenhum Backup nesta pasta ainda.".into(),
        };
        let reveal = self.newest.as_ref().map(|backup| backup.path.clone());
        let actions = h_flex()
            .gap_2()
            .flex_wrap()
            .child(
                Button::new("back-up-now")
                    .label("Fazer Backup agora")
                    .primary()
                    .small()
                    .loading(self.busy)
                    .disabled(self.blocked())
                    .on_click(cx.listener(|this, _, _, cx| this.back_up_now(cx))),
            )
            .when_some(reveal, |row, path| {
                row.child(
                    Button::new("reveal-backup")
                        .label("Abrir pasta")
                        .ghost()
                        .small()
                        .on_click(move |_, _, cx| cx.reveal_path(&path)),
                )
            });

        let carry = h_flex()
            .gap_2()
            .flex_wrap()
            .child(
                Button::new("export-database")
                    .label("Exportar…")
                    .outline()
                    .small()
                    .disabled(self.blocked())
                    .on_click(cx.listener(|this, _, _, cx| this.export(cx))),
            )
            .child(
                Button::new("restore-database")
                    .label("Restaurar…")
                    .outline()
                    .small()
                    .disabled(self.blocked())
                    .on_click(cx.listener(|this, _, _, cx| this.choose_restore(cx))),
            )
            .child(
                Button::new("compare-database")
                    .label("Comparar com outro banco…")
                    .outline()
                    .small()
                    .disabled(self.blocked())
                    .on_click(cx.listener(|this, _, _, cx| this.choose_compare(cx))),
            );

        v_flex()
            .gap_3()
            .child(intro)
            .child(folder)
            .child(keep)
            .child(div().text_sm().text_color(t.text2).child(newest))
            .when_some(daily_failure, |section, failure| {
                section.child(kit::error_notice(format!("Backup diário: {failure}"), cx))
            })
            .child(actions)
            .child(div().h_2())
            .child(kit::section_heading("Outro computador"))
            .child(div().text_sm().text_color(t.text2).child(
                "Exportar grava a mesma cópia onde você escolher. Restaurar troca o banco \
                 atual por um Backup ou export do Mascate, de versão igual ou anterior. \
                 Comparar mostra o que esse arquivo traria para este banco, registro por \
                 registro, sem gravar nada; dali dá para mesclar, depois de um Backup.",
            ))
            .child(carry)
            .when_some(self.confirming.clone(), |section, file| {
                section.child(self.render_restore_confirmation(&file, cx))
            })
            .when_some(self.outcome.as_ref(), |section, outcome| {
                section.child(match outcome {
                    Outcome::Done(text) => kit::success_notice(text.clone(), cx),
                    Outcome::Failed(text) => kit::error_notice(text.clone(), cx),
                })
            })
            .children(self.render_merges(cx))
    }
}
