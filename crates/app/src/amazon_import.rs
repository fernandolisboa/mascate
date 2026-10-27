//! "Importar relatório" on the Afiliação screen (#83): the Amazon
//! Associados earnings report the owner exports from Associates Central, as
//! CSV or XLSX. The app reads the file, shows what importing it would add,
//! update and keep, and every row it leaves out with the line and why; only
//! "Importar" records it, in one transaction. The reading lives in
//! Integrations, the rules in Affiliate (ADR 0030).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, EventEmitter, PathPromptOptions, Window, div};
use mascate_affiliate::{Affiliate, Recorded};
use mascate_integrations::{
    AMAZON_PLATFORM, EarningsImport, EarningsReport, ReportError, SkippedRow, read_earnings_report,
};
use mascate_kernel::{Currency, Money};
use rust_decimal::Decimal;

use crate::affiliate::{affiliate, failure};
use crate::appearance::look;
use crate::forms::{Outcome, notice};
use crate::kit;
use crate::merge_import::file_name;

/// Commissions came in from a file.
pub struct CommissionsImported;

/// An export of a few years is well under this; anything bigger is not one.
const LARGEST_FILE: u64 = 50 * 1024 * 1024;

/// A report read and checked against what is in the app, waiting for the
/// owner to confirm.
struct Preview {
    file: String,
    report: EarningsReport,
    plan: EarningsImport,
    would: Recorded,
}

/// What the last import did.
struct Imported {
    file: String,
    recorded: Recorded,
    skipped: Vec<SkippedRow>,
}

#[derive(Default)]
pub struct AmazonImportSection {
    preview: Option<Preview>,
    imported: Option<Imported>,
    busy: bool,
    outcome: Option<Outcome>,
}

impl EventEmitter<CommissionsImported> for AmazonImportSection {}

impl AmazonImportSection {
    fn choose(&mut self, cx: &mut Context<Self>) {
        pick_report(cx, |this, picked, cx| match picked {
            Ok(path) => this.read(path, cx),
            Err(why) => {
                this.outcome = Some(Outcome::Failed(why.into()));
                cx.notify();
            }
        });
    }

    /// Reads the file off the UI thread and previews its import.
    fn read(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(affiliate) = affiliate(cx) else {
            return;
        };
        self.busy = true;
        self.preview = None;
        self.imported = None;
        self.outcome = None;
        let reading = cx.background_executor().spawn(async move {
            let file = file_name(&path);
            let report = read_file(&path)?;
            let plan = planned(&affiliate, &report).await?;
            let would = affiliate
                .preview_import(AMAZON_PLATFORM, &plan.conversions)
                .await
                .map_err(|error| failure(&error))?;
            Ok::<_, String>(Preview {
                file,
                report,
                plan,
                would,
            })
        });
        cx.spawn(async move |this, cx| {
            let read = reading.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match read {
                    Ok(preview) => this.preview = Some(preview),
                    Err(why) => this.outcome = Some(Outcome::Failed(why.into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Records the previewed report, checked again against the app, in one
    /// transaction.
    fn import(&mut self, cx: &mut Context<Self>) {
        let (Some(affiliate), Some(preview)) = (affiliate(cx), self.preview.take()) else {
            return;
        };
        self.busy = true;
        let importing = cx.background_executor().spawn(async move {
            let plan = planned(&affiliate, &preview.report).await?;
            let recorded = affiliate
                .import(AMAZON_PLATFORM, &plan.conversions)
                .await
                .map_err(|error| failure(&error))?;
            Ok::<_, String>(Imported {
                file: preview.file,
                recorded,
                skipped: plan.skipped,
            })
        });
        cx.spawn(async move |this, cx| {
            let imported = importing.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match imported {
                    Ok(imported) => {
                        this.outcome = Some(Outcome::Done(
                            format!(
                                "Relatório {} importado: {}.",
                                imported.file,
                                recorded_text(&imported.recorded)
                            )
                            .into(),
                        ));
                        this.imported = Some(imported);
                        cx.emit(CommissionsImported);
                    }
                    Err(why) => this.outcome = Some(Outcome::Failed(why.into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Drops the previewed import without recording it.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.preview = None;
        cx.notify();
    }

    fn render_preview(&self, preview: &Preview, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let period = preview.report.period().map_or(String::new(), |days| {
            format!(
                "de {} a {} · ",
                days.start().format("%d/%m/%Y"),
                days.end().format("%d/%m/%Y")
            )
        });
        let earned: Decimal = preview
            .plan
            .conversions
            .iter()
            .filter(|conversion| conversion.status.is_earned())
            .map(|conversion| conversion.commission.amount())
            .sum();
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.accent_edge)
            .bg(t.surface)
            .child(div().font_medium().child(preview.file.clone()))
            .child(div().text_sm().text_color(t.text2).child(format!(
                "{period}{} e {} · comissões aprovadas: {}",
                count(preview.report.sales(), "venda", "vendas"),
                count(preview.report.returns(), "devolução", "devoluções"),
                Money::new(earned, Currency::Brl).to_pt_br()
            )))
            .child(
                div()
                    .text_sm()
                    .child(format!("Ao importar: {}.", recorded_text(&preview.would))),
            )
            .children(skipped_list(&preview.plan.skipped, cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("import-report")
                            .label("Importar")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.import(cx))),
                    )
                    .child(
                        Button::new("cancel-report")
                            .label("Cancelar")
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                    ),
            )
            .into_any_element()
    }
}

impl Render for AmazonImportSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_center()
                    .child(kit::section_heading("Importar relatório da Amazon"))
                    .child(
                        Button::new("choose-report")
                            .label("Escolher arquivo")
                            .icon(IconName::FolderOpen)
                            .outline()
                            .small()
                            .loading(self.busy && self.preview.is_none())
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.choose(cx))),
                    ),
            )
            .child(div().text_sm().text_color(t.text2).child(
                "A Amazon não tem API de ganhos: no Associates Central, abra Relatórios › Ganhos, \
                 exporte em CSV ou Excel e escolha o arquivo aqui. Antes de gravar, o app mostra o \
                 que vai entrar; importar o mesmo período de novo não duplica nada.",
            ))
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .children(
                self.preview
                    .as_ref()
                    .map(|preview| self.render_preview(preview, cx)),
            )
            .children(
                self.imported
                    .as_ref()
                    .and_then(|imported| skipped_list(&imported.skipped, cx)),
            )
    }
}

/// The rows left out, each with its line and why; nothing without one.
pub(crate) fn skipped_list(skipped: &[SkippedRow], cx: &App) -> Option<AnyElement> {
    if skipped.is_empty() {
        return None;
    }
    let t = look(cx).tokens;
    Some(
        v_flex()
            .gap_1()
            .child(div().text_sm().font_medium().child(format!(
                "{} fora da importação:",
                count(skipped.len(), "linha ficou", "linhas ficaram")
            )))
            .children(skipped.iter().map(|row| {
                div()
                    .text_xs()
                    .text_color(t.text2)
                    .child(format!("Linha {}: {}", row.line, row.reason))
            }))
            .into_any_element(),
    )
}

async fn planned(
    affiliate: &Arc<Affiliate>,
    report: &EarningsReport,
) -> Result<EarningsImport, String> {
    let readings = affiliate
        .readings(AMAZON_PLATFORM)
        .await
        .map_err(|error| failure(&error))?;
    Ok(report.conversions(&readings))
}

/// Asks the owner for one report file and hands `picked` its path, or why
/// the file picker did not open; nothing when the owner closes it.
pub(crate) fn pick_report<V: 'static>(
    cx: &mut Context<V>,
    picked: impl FnOnce(&mut V, Result<PathBuf, String>, &mut Context<V>) + 'static,
) {
    let picking = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some("Ler este relatório".into()),
    });
    cx.spawn(async move |this, cx| {
        let chosen = picking.await;
        let _ = this.update(cx, |this, cx| match chosen {
            Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                picked(this, Ok(paths.swap_remove(0)), cx)
            }
            Ok(Err(error)) => picked(
                this,
                Err(format!("Não consegui abrir o seletor de arquivos: {error}")),
                cx,
            ),
            _ => {}
        });
    })
    .detach();
}

/// The bytes of a report file the owner picked; one over 50 MB is not
/// `what`.
pub(crate) fn report_bytes(path: &Path, what: &str) -> Result<Vec<u8>, String> {
    let unreadable = |error: std::io::Error| format!("Não consegui ler o arquivo: {error}");
    if std::fs::metadata(path).map_err(unreadable)?.len() > LARGEST_FILE {
        return Err(format!("O arquivo passa de 50 MB: não é {what}."));
    }
    std::fs::read(path).map_err(unreadable)
}

fn read_file(path: &Path) -> Result<EarningsReport, String> {
    let bytes = report_bytes(path, "um relatório de ganhos")?;
    read_earnings_report(&bytes).map_err(|error| match error {
        ReportError::NotEarningsReport => "Este arquivo não é o relatório de ganhos da Amazon: \
             no Associates Central, exporte Relatórios › Ganhos em CSV ou Excel. Nada foi gravado."
            .into(),
        ReportError::Unreadable(why) => {
            format!("Não consegui ler a planilha ({why}). Nada foi gravado.")
        }
    })
}

/// "1 venda", "3 vendas".
pub(crate) fn count(count: usize, one: &str, many: &str) -> String {
    match count {
        1 => format!("1 {one}"),
        count => format!("{count} {many}"),
    }
}

/// "2 novas, 1 atualizada e 5 já estavam no app".
fn recorded_text(recorded: &Recorded) -> String {
    let parts: Vec<String> = [
        (recorded.added, "nova", "novas"),
        (recorded.changed, "atualizada", "atualizadas"),
        (recorded.unchanged, "já estava no app", "já estavam no app"),
    ]
    .into_iter()
    .filter(|(number, _, _)| *number > 0)
    .map(|(number, one, many)| count(number, one, many))
    .collect();
    match parts.as_slice() {
        [] => "nenhuma comissão".into(),
        [only] => only.clone(),
        [first @ .., last] => format!("{} e {last}", first.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_an_import_did_reads_as_a_sentence() {
        let recorded = |added, changed, unchanged| Recorded {
            added,
            changed,
            unchanged,
        };
        assert_eq!(recorded_text(&recorded(0, 0, 0)), "nenhuma comissão");
        assert_eq!(recorded_text(&recorded(1, 0, 0)), "1 nova");
        assert_eq!(
            recorded_text(&recorded(2, 1, 5)),
            "2 novas, 1 atualizada e 5 já estavam no app"
        );
        assert_eq!(recorded_text(&recorded(0, 0, 8)), "8 já estavam no app");
    }
}
