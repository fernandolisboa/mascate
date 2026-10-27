//! "Gasto com divulgação" on the Divulgação screen (#195, ADR 0037 decision
//! 6): what each Promotion Channel spent in the last 30 days, "Lançar gasto"
//! to type a spend over some days, and "Importar relatório" for the report
//! the Meta Ads Manager or Google Ads exports broken down by day. A campaign
//! the app has not seen asks once which channel it is. The app records the
//! spend and never spends or pauses anything; the rules live in the
//! Marketing, the reading in Integrations.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{NaiveDate, TimeDelta, Utc};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, Global, Window, div, px};
use mascate_integrations::{SpendReading, SpendReportError, read_spend_report};
use mascate_kernel::{Currency, Money, RecordId, channel_day, parse_amount};
use mascate_marketing::{
    CampaignLink, ChannelKind, ChannelSpend, ImportedSpends, NewPromotionSpend, PromotionChannel,
    PromotionSpend, PromotionSpends, SavedSpend, SpendError, SpendOrigin, SpendReport,
};

use crate::amazon_import::{count, pick_report, report_bytes, skipped_list};
use crate::appearance::look;
use crate::forms::{Outcome, amount_text, day_text, input, notice, parse_day};
use crate::kit;
use crate::merge_import::file_name;
use crate::sales::table_row;

/// The app's Promotion Spends; absent when the database did not open.
pub struct AppPromotionSpends(pub Arc<PromotionSpends>);

impl Global for AppPromotionSpends {}

fn promotion_spends(cx: &App) -> Option<Arc<PromotionSpends>> {
    cx.try_global::<AppPromotionSpends>()
        .map(|app| app.0.clone())
}

/// How many days back "what each channel spent" looks, today included.
const RECENT_DAYS: i64 = 30;

/// How many spends a channel's list shows, the latest first.
const LISTED_SPENDS: usize = 40;

fn failure(error: &SpendError) -> String {
    match error {
        SpendError::BadPeriod => "O último dia vem antes do primeiro.".into(),
        SpendError::BadAmount => "Digite quanto gastou, acima de zero, como 150,00.".into(),
        SpendError::BadCampaign => "Uma campanha do relatório está sem nome.".into(),
        SpendError::NoSuchChannel => {
            "Esse canal foi apagado; a lista foi atualizada. Escolha outro.".into()
        }
        SpendError::NotFound => "Esse gasto mudou; a lista foi atualizada.".into(),
        SpendError::FromReport => "Um gasto lido do relatório muda só quando você importa o \
                                   relatório de novo, com o valor corrigido pela plataforma."
            .into(),
        SpendError::UnlinkedCampaign(campaign) => {
            format!("Escolha o canal da campanha {campaign} antes de importar.")
        }
        SpendError::Unreadable(_) | SpendError::Sql(_) => {
            format!("Não consegui ler ou gravar os gastos no banco: {error}")
        }
    }
}

/// Why a report was refused, with what to do, as the owner reads it.
fn report_failure(error: &SpendReportError) -> String {
    match error {
        SpendReportError::NotSpendReport => "Este arquivo não é o relatório de gasto do \
             Gerenciador de Anúncios da Meta nem o do Google Ads. Nada foi gravado."
            .into(),
        SpendReportError::NotDaily(SpendReport::MetaAds) => "Este relatório da Meta soma vários \
             dias numa linha, e o app contaria o gasto em dobro com um relatório por dia. No \
             Gerenciador de Anúncios, clique em Detalhamento › Por tempo › Dia, depois em \
             Exportar › Exportar dados da tabela (CSV ou Excel) e escolha o arquivo de novo. \
             Nada foi gravado."
            .into(),
        SpendReportError::NotDaily(SpendReport::GoogleAds) => "Este relatório do Google Ads \
             soma vários dias numa linha, e o app contaria o gasto em dobro com um relatório por \
             dia. Em Campanhas, clique em Segmentar › Tempo › Dia, depois em Fazer download (CSV \
             ou Excel) e escolha o arquivo de novo. Nada foi gravado."
            .into(),
        SpendReportError::NoCurrency(SpendReport::MetaAds) => "Este relatório da Meta não diz a \
             moeda. Exporte com a coluna \"Valor usado\" (ela traz a moeda, como \"Valor usado \
             (BRL)\") ou com a coluna Moeda. Nada foi gravado."
            .into(),
        SpendReportError::NoCurrency(SpendReport::GoogleAds) => "Este relatório do Google Ads \
             não diz a moeda. Em Colunas, adicione \"Código da moeda\", faça o download de novo \
             e escolha o arquivo. Nada foi gravado."
            .into(),
        SpendReportError::Unreadable(why) => {
            format!("Não consegui ler a planilha ({why}). Nada foi gravado.")
        }
    }
}

/// "R$ 120,00", or "R$ 120,00 + US$ 15,00" when a channel spent in both.
fn amounts_text(amounts: &[Money]) -> String {
    amounts
        .iter()
        .map(|amount| amount.to_pt_br())
        .collect::<Vec<_>>()
        .join(" + ")
}

/// "01/10/2026" or "01/10/2026 a 10/10/2026".
fn days_text(first: NaiveDate, last: NaiveDate) -> String {
    if first == last {
        day_text(first)
    } else {
        format!("{} a {}", day_text(first), day_text(last))
    }
}

/// "5 dias novos, 1 corrigido e 3 já estavam no app".
fn imported_text(imported: &ImportedSpends) -> String {
    let parts: Vec<String> = [
        (imported.added, "dia novo", "dias novos"),
        (imported.changed, "dia corrigido", "dias corrigidos"),
        (
            imported.unchanged,
            "dia já estava no app",
            "dias já estavam no app",
        ),
    ]
    .into_iter()
    .filter(|(number, _, _)| *number > 0)
    .map(|(number, one, many)| count(number, one, many))
    .collect();
    match parts.as_slice() {
        [] => "nenhum dia".into(),
        [only] => only.clone(),
        [first @ .., last] => format!("{} e {last}", first.join(", ")),
    }
}

/// The warning of a typed spend whose days cross other spends of its
/// channel.
fn crossing_text(saved: &SavedSpend) -> Option<String> {
    if saved.crossing.is_empty() {
        return None;
    }
    let crossed: Vec<String> = saved
        .crossing
        .iter()
        .map(|spend| {
            format!(
                "{} ({})",
                days_text(spend.first_day, spend.last_day),
                spend.amount.to_pt_br()
            )
        })
        .collect();
    Some(format!(
        "Atenção: este gasto cruza {} já lançado neste canal em {}. Se for o mesmo dinheiro, \
         apague um deles para não contar duas vezes.",
        if crossed.len() == 1 {
            "outro"
        } else {
            "outros"
        },
        crossed.join(", ")
    ))
}

/// The channels a campaign of `report` most likely belongs to come first:
/// paid social for the Meta, paid search for Google Ads.
fn channels_for(report: SpendReport, channels: &[PromotionChannel]) -> Vec<PromotionChannel> {
    let likely = match report {
        SpendReport::MetaAds => ChannelKind::PaidSocial,
        SpendReport::GoogleAds => ChannelKind::PaidSearch,
    };
    let mut sorted = channels.to_vec();
    sorted.sort_by_key(|channel| channel.kind != likely);
    sorted
}

/// A spend being typed for a channel.
struct SpendForm {
    channel: RecordId,
    /// The typed spend being edited, if any.
    editing: Option<RecordId>,
    first_day: Entity<InputState>,
    last_day: Entity<InputState>,
    amount: Entity<InputState>,
    outcome: Option<Outcome>,
}

/// A report read, waiting for its new campaigns' channels and the owner's
/// confirmation.
struct Preview {
    file: String,
    reading: SpendReading,
    /// The campaigns with no channel yet, in the report's order.
    unlinked: Vec<String>,
    picks: HashMap<String, RecordId>,
    /// What importing would record, once every campaign has its channel.
    would: Option<ImportedSpends>,
}

impl Preview {
    fn links(&self) -> Vec<CampaignLink> {
        self.unlinked
            .iter()
            .filter_map(|campaign| {
                Some(CampaignLink {
                    report: self.reading.report,
                    campaign: campaign.clone(),
                    channel: *self.picks.get(campaign)?,
                })
            })
            .collect()
    }

    fn ready(&self) -> bool {
        self.unlinked
            .iter()
            .all(|campaign| self.picks.contains_key(campaign))
    }
}

pub struct SpendsSection {
    channels: Vec<PromotionChannel>,
    /// What each channel spent in the last 30 days.
    recent: HashMap<RecordId, Vec<Money>>,
    /// The channel whose spends are listed, with them.
    listed: Option<(RecordId, Vec<PromotionSpend>)>,
    form: Option<SpendForm>,
    /// The typed spend asking to confirm its deletion.
    deleting: Option<RecordId>,
    preview: Option<Preview>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
}

impl SpendsSection {
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
            recent: HashMap::new(),
            listed: None,
            form: None,
            deleting: None,
            preview: None,
            busy: false,
            reads: 0,
            outcome: None,
        }
    }

    /// Takes the channels as Divulgação read them, and reads their spends
    /// again.
    pub fn set_channels(&mut self, channels: Vec<PromotionChannel>, cx: &mut Context<Self>) {
        let live = |id: &RecordId| channels.iter().any(|channel| channel.id == *id);
        if self.form.as_ref().is_some_and(|form| !live(&form.channel)) {
            self.form = None;
        }
        if self.listed.as_ref().is_some_and(|(id, _)| !live(id)) {
            self.listed = None;
        }
        self.channels = channels;
        self.refresh(cx);
    }

    /// Reads what each channel spent, and the listed channel's spends.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(spends) = promotion_spends(cx) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        let listed = self.listed.as_ref().map(|(id, _)| *id);
        let today = channel_day(Utc::now());
        let since = today - TimeDelta::days(RECENT_DAYS - 1);
        let reading = cx.background_executor().spawn(async move {
            let spent = spends.spent(since..=today).await?;
            let list = match listed {
                Some(channel) => Some((channel, spends.spends(channel).await?)),
                None => None,
            };
            Ok::<_, SpendError>((spent, list))
        });
        cx.spawn(async move |this, cx| {
            let read_back = reading.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                match read_back {
                    Ok((spent, list)) => {
                        this.recent = recent_by_channel(spent);
                        if let Some(list) = list
                            && this.listed.as_ref().map(|(id, _)| *id) == Some(list.0)
                        {
                            this.listed = Some(list);
                        }
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_list(&mut self, channel: RecordId, cx: &mut Context<Self>) {
        self.deleting = None;
        if self.listed.as_ref().is_some_and(|(id, _)| *id == channel) {
            self.listed = None;
            cx.notify();
        } else {
            self.listed = Some((channel, Vec::new()));
            self.refresh(cx);
        }
    }

    /// Opens "Lançar gasto" for `channel`, or the typed `spend` to edit.
    fn open_form(
        &mut self,
        channel: RecordId,
        spend: Option<&PromotionSpend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let first_day = input("01/10/2026", window, cx);
        let last_day = input("31/10/2026", window, cx);
        let amount = input("150,00", window, cx);
        if let Some(spend) = spend {
            let fill =
                |field: &Entity<InputState>, text: String, window: &mut Window, cx: &mut App| {
                    field.update(cx, |field, cx| field.set_value(text, window, cx));
                };
            fill(&first_day, day_text(spend.first_day), window, cx);
            fill(&last_day, day_text(spend.last_day), window, cx);
            fill(&amount, amount_text(spend.amount), window, cx);
        }
        self.form = Some(SpendForm {
            channel,
            editing: spend.map(|spend| spend.id),
            first_day,
            last_day,
            amount,
            outcome: None,
        });
        cx.notify();
    }

    /// The spend typed in the form, or why it cannot be one.
    fn typed(form: &SpendForm, cx: &App) -> Result<NewPromotionSpend, String> {
        let day = |field: &Entity<InputState>, which: &str| {
            let text = field.read(cx).value().to_string();
            parse_day(&text).ok_or_else(|| {
                format!("\"{text}\" não é um dia: escreva o {which} dia como 01/10/2026.")
            })
        };
        let first_day = day(&form.first_day, "primeiro")?;
        let last_text = form.last_day.read(cx).value().trim().to_owned();
        // One day only when the last is left empty.
        let last_day = if last_text.is_empty() {
            first_day
        } else {
            day(&form.last_day, "último")?
        };
        let typed = form.amount.read(cx).value().to_string();
        let amount = parse_amount(&typed)
            .ok_or_else(|| format!("\"{typed}\" não é um valor: escreva como 150,00."))?;
        Ok(NewPromotionSpend {
            channel: form.channel,
            first_day,
            last_day,
            amount: Money::new(amount, Currency::Brl),
        })
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let (Some(spends), Some(form)) = (promotion_spends(cx), self.form.as_mut()) else {
            return;
        };
        let typed = match Self::typed(form, cx) {
            Ok(typed) => typed,
            Err(why) => {
                form.outcome = Some(Outcome::Failed(why.into()));
                cx.notify();
                return;
            }
        };
        let editing = form.editing;
        self.busy = true;
        let saving = cx.background_executor().spawn(async move {
            match editing {
                Some(id) => spends.update(id, &typed).await,
                None => spends.record(&typed).await,
            }
        });
        cx.spawn(async move |this, cx| {
            let saved = saving.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match saved {
                    Ok(saved) => {
                        let done = format!(
                            "Gasto de {} em {} {}.",
                            saved.spend.amount.to_pt_br(),
                            days_text(saved.spend.first_day, saved.spend.last_day),
                            if editing.is_some() {
                                "salvo"
                            } else {
                                "lançado"
                            }
                        );
                        this.form = None;
                        this.outcome = Some(match crossing_text(&saved) {
                            Some(warning) => Outcome::Failed(format!("{done} {warning}").into()),
                            None => Outcome::Done(done.into()),
                        });
                        this.refresh(cx);
                    }
                    Err(error) => {
                        if let Some(form) = this.form.as_mut() {
                            form.outcome = Some(Outcome::Failed(failure(&error).into()));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn delete(&mut self, id: RecordId, cx: &mut Context<Self>) {
        let Some(spends) = promotion_spends(cx) else {
            return;
        };
        self.deleting = None;
        self.busy = true;
        let deleting = cx
            .background_executor()
            .spawn(async move { spends.delete(id).await });
        cx.spawn(async move |this, cx| {
            let deleted = deleting.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.outcome = Some(match deleted {
                    Ok(()) => Outcome::Done("Gasto apagado.".into()),
                    Err(error) => Outcome::Failed(failure(&error).into()),
                });
                if this.form.as_ref().and_then(|form| form.editing) == Some(id) {
                    this.form = None;
                }
                this.refresh(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn choose(&mut self, cx: &mut Context<Self>) {
        pick_report(cx, |this, picked, cx| match picked {
            Ok(path) => this.read(path, cx),
            Err(why) => {
                this.outcome = Some(Outcome::Failed(why.into()));
                cx.notify();
            }
        });
    }

    /// Reads the report off the UI thread and asks for the channels of the
    /// campaigns the app has not seen.
    fn read(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(spends) = promotion_spends(cx) else {
            return;
        };
        self.busy = true;
        self.preview = None;
        self.outcome = None;
        let reading = cx.background_executor().spawn(async move {
            let bytes = report_bytes(&path, "um relatório de anúncios")?;
            let reading = read_spend_report(&bytes).map_err(|error| report_failure(&error))?;
            let unlinked: Vec<String> = spends
                .unlinked(&reading.spends)
                .await
                .map_err(|error| failure(&error))?
                .into_iter()
                .map(|(_, campaign)| campaign)
                .collect();
            let would = if unlinked.is_empty() {
                Some(
                    spends
                        .preview_import(&reading.spends, &[])
                        .await
                        .map_err(|error| failure(&error))?,
                )
            } else {
                None
            };
            Ok::<_, String>(Preview {
                file: file_name(&path),
                reading,
                unlinked,
                picks: HashMap::new(),
                would,
            })
        });
        cx.spawn(async move |this, cx| {
            let read = reading.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match read {
                    Ok(preview) if preview.reading.spends.is_empty() => {
                        this.outcome = Some(Outcome::Failed(
                            format!(
                                "O relatório {} não tem gasto de nenhuma campanha. Nada foi \
                                 gravado.",
                                preview.file
                            )
                            .into(),
                        ));
                        this.preview = Some(preview).filter(|it| !it.reading.skipped.is_empty());
                    }
                    Ok(preview) => this.preview = Some(preview),
                    Err(why) => this.outcome = Some(Outcome::Failed(why.into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Keeps the channel picked for a new campaign; once every campaign has
    /// one, reads what importing would record.
    fn pick(&mut self, campaign: String, channel: RecordId, cx: &mut Context<Self>) {
        let (Some(spends), Some(preview)) = (promotion_spends(cx), self.preview.as_mut()) else {
            return;
        };
        preview.picks.insert(campaign, channel);
        preview.would = None;
        if !preview.ready() {
            cx.notify();
            return;
        }
        let (reported, links) = (preview.reading.spends.clone(), preview.links());
        let previewing = cx
            .background_executor()
            .spawn(async move { spends.preview_import(&reported, &links).await });
        cx.spawn(async move |this, cx| {
            let would = previewing.await;
            let _ = this.update(cx, |this, cx| {
                match would {
                    Ok(would) => {
                        if let Some(preview) = this.preview.as_mut().filter(|it| it.ready()) {
                            preview.would = Some(would);
                        }
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Records the previewed report with the channels picked, in one
    /// transaction.
    fn import(&mut self, cx: &mut Context<Self>) {
        let Some(spends) = promotion_spends(cx) else {
            return;
        };
        let Some(preview) = self.preview.take_if(|preview| preview.ready()) else {
            return;
        };
        self.busy = true;
        let links = preview.links();
        let importing = cx.background_executor().spawn(async move {
            let imported = spends.import(&preview.reading.spends, &links).await;
            (preview, imported)
        });
        cx.spawn(async move |this, cx| {
            let (preview, imported) = importing.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match imported {
                    Ok(imported) => {
                        this.outcome = Some(Outcome::Done(
                            format!(
                                "Relatório {} importado: {}.",
                                preview.file,
                                imported_text(&imported)
                            )
                            .into(),
                        ));
                        this.refresh(cx);
                    }
                    Err(error) => {
                        this.outcome = Some(Outcome::Failed(failure(&error).into()));
                        this.preview = Some(preview);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn render_preview(&self, preview: &Preview, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let reading = &preview.reading;
        let period = reading.period().map_or(String::new(), |days| {
            format!("{} · ", days_text(*days.start(), *days.end()))
        });
        let summary = format!(
            "{} · {period}{} · total {}",
            reading.report.name(),
            count(reading.campaigns().len(), "campanha", "campanhas"),
            amounts_text(&reading.total())
        );
        let questions = preview.unlinked.iter().enumerate().map(|(row, campaign)| {
            let picked = preview.picks.get(campaign).copied();
            let chips = channels_for(reading.report, &self.channels)
                .into_iter()
                .enumerate()
                .map(|(index, channel)| {
                    let chip = Button::new(("spend-campaign-channel", row * 1000 + index))
                        .label(channel.name.clone())
                        .small()
                        .disabled(self.busy);
                    let chip = if picked == Some(channel.id) {
                        chip.primary()
                    } else {
                        chip.outline()
                    };
                    let campaign = campaign.clone();
                    chip.on_click(cx.listener(move |this, _, _, cx| {
                        this.pick(campaign.clone(), channel.id, cx)
                    }))
                });
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .child(format!("De qual canal é a campanha \"{campaign}\"?")),
                )
                .child(h_flex().flex_wrap().gap_1().children(chips))
        });
        let ready = preview.ready() && !self.channels.is_empty();
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.accent_edge)
            .bg(t.surface)
            .child(div().font_medium().child(preview.file.clone()))
            .child(div().text_sm().text_color(t.text2).child(summary))
            .when(!preview.unlinked.is_empty(), |card| {
                card.child(div().text_xs().text_color(t.text2).child(
                    "Campanhas novas: escolha o canal de cada uma. A escolha fica guardada para os \
                     próximos relatórios.",
                ))
            })
            .when(
                !preview.unlinked.is_empty() && self.channels.is_empty(),
                |card| {
                    card.child(kit::info_notice(
                        "Crie primeiro um canal, como \"Anúncio pago na Meta ou no TikTok\", em \
                         Canais de divulgação.",
                        cx,
                    ))
                },
            )
            .children(questions)
            .children(preview.would.map(|would| {
                div()
                    .text_sm()
                    .child(format!("Ao importar: {}.", imported_text(&would)))
            }))
            .children(skipped_list(&reading.skipped, cx))
            .child(
                h_flex()
                    .gap_2()
                    .when(!reading.spends.is_empty(), |buttons| {
                        buttons.child(
                            Button::new("import-spends")
                                .label("Importar")
                                .primary()
                                .small()
                                .loading(self.busy)
                                .disabled(self.busy || !ready)
                                .on_click(cx.listener(|this, _, _, cx| this.import(cx))),
                        )
                    })
                    .child(
                        Button::new("cancel-spends")
                            .label(if reading.spends.is_empty() {
                                "Fechar"
                            } else {
                                "Cancelar"
                            })
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.preview = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_form(&self, form: &SpendForm, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .child(div().text_sm().child(label))
                .child(div().w(px(150.)).child(Input::new(input).small()))
        };
        v_flex()
            .gap_2()
            .ml_6()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.border)
            .bg(t.raised)
            .child(div().font_medium().child(if form.editing.is_some() {
                "Editar gasto"
            } else {
                "Lançar gasto"
            }))
            .child(div().text_xs().text_color(t.text2).child(
                "Quanto você pagou nestes dias, como está na fatura ou no painel do anúncio. Para \
                 um dia só, deixe o último dia vazio.",
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .child(field("Primeiro dia", &form.first_day))
                    .child(field("Último dia", &form.last_day))
                    .child(field("Valor (R$)", &form.amount)),
            )
            .children(form.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("save-spend")
                            .label(if form.editing.is_some() {
                                "Salvar gasto"
                            } else {
                                "Lançar"
                            })
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    )
                    .child(
                        Button::new("close-spend")
                            .label("Cancelar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.form = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_list(&self, spends: &[PromotionSpend], cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let rows = spends
            .iter()
            .take(LISTED_SPENDS)
            .enumerate()
            .map(|(index, spend)| {
                let origin = match (spend.origin, spend.campaign.as_deref()) {
                    (SpendOrigin::Typed, _) => "lançado à mão".to_owned(),
                    (SpendOrigin::Report(report), Some(campaign)) => {
                        format!("{} · campanha {campaign}", report.name())
                    }
                    (SpendOrigin::Report(report), None) => report.name().to_owned(),
                };
                let id = spend.id;
                let edited = spend.clone();
                let actions = match spend.origin {
                    SpendOrigin::Report(_) => None,
                    SpendOrigin::Typed if self.deleting == Some(id) => Some(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_xs().child("Apagar o gasto?"))
                            .child(
                                Button::new(("confirm-delete-spend", index))
                                    .label("Apagar")
                                    .danger()
                                    .small()
                                    .disabled(self.busy)
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.delete(id, cx)),
                                    ),
                            )
                            .child(
                                Button::new(("keep-spend", index))
                                    .label("Manter")
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.deleting = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
                    SpendOrigin::Typed => Some(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new(("edit-spend", index))
                                    .label("Editar")
                                    .outline()
                                    .small()
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_form(edited.channel, Some(&edited), window, cx)
                                    })),
                            )
                            .child(
                                Button::new(("delete-spend", index))
                                    .label("Apagar")
                                    .ghost()
                                    .small()
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.deleting = Some(id);
                                        cx.notify();
                                    })),
                            ),
                    ),
                };
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_center()
                    .py_1()
                    .child(
                        div()
                            .w(px(200.))
                            .text_sm()
                            .child(days_text(spend.first_day, spend.last_day)),
                    )
                    .child(
                        div()
                            .w(px(110.))
                            .text_sm()
                            .font_medium()
                            .child(spend.amount.to_pt_br()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(180.))
                            .text_xs()
                            .text_color(t.text2)
                            .child(origin),
                    )
                    .children(actions)
            });
        v_flex()
            .gap_1()
            .ml_6()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.border)
            .bg(t.raised)
            .when(spends.is_empty(), |list| {
                list.child(
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child("Nenhum gasto lançado neste canal."),
                )
            })
            .children(rows)
            .when(spends.len() > LISTED_SPENDS, |list| {
                list.child(div().text_xs().text_color(t.text2).child(format!(
                    "Mostrando os {LISTED_SPENDS} mais recentes de {}.",
                    spends.len()
                )))
            })
            .into_any_element()
    }

    fn render_channel(
        &self,
        index: usize,
        channel: &PromotionChannel,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let id = channel.id;
        let recent = self
            .recent
            .get(&id)
            .filter(|amounts| amounts.iter().any(|amount| !amount.amount().is_zero()))
            .map_or_else(
                || "sem gasto nos últimos 30 dias".to_owned(),
                |amounts| format!("{} nos últimos 30 dias", amounts_text(amounts)),
            );
        let open = self
            .listed
            .as_ref()
            .is_some_and(|(listed, _)| *listed == id);
        table_row(&t)
            .flex_wrap()
            .child(kit::tag(channel.kind.name(), t.text2, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(220.))
                    .child(div().font_medium().truncate().child(channel.name.clone()))
                    .child(div().text_xs().text_color(t.text2).child(recent)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(("add-spend", index))
                            .label("Lançar gasto")
                            .outline()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_form(id, None, window, cx)
                            })),
                    )
                    .child(
                        Button::new(("list-spends", index))
                            .label(if open {
                                "Fechar lançamentos"
                            } else {
                                "Lançamentos"
                            })
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_list(id, cx))),
                    ),
            )
            .into_any_element()
    }
}

/// What each channel spent, its currencies together.
fn recent_by_channel(spent: Vec<ChannelSpend>) -> HashMap<RecordId, Vec<Money>> {
    let mut recent: HashMap<RecordId, Vec<Money>> = HashMap::new();
    for spend in spent {
        recent.entry(spend.channel).or_default().push(spend.amount);
    }
    recent
}

impl Render for SpendsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        if promotion_spends(cx).is_none() {
            return div().into_any_element();
        }
        let mut rows: Vec<AnyElement> = Vec::new();
        for (index, channel) in self.channels.clone().iter().enumerate() {
            rows.push(self.render_channel(index, channel, cx));
            if let Some(form) = self.form.as_ref().filter(|form| form.channel == channel.id) {
                rows.push(self.render_form(form, cx));
            }
            if let Some((_, spends)) = self.listed.as_ref().filter(|(id, _)| *id == channel.id) {
                rows.push(self.render_list(spends, cx));
            }
        }
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_center()
                    .child(kit::section_heading("Gasto com divulgação"))
                    .child(
                        Button::new("choose-spend-report")
                            .label("Importar relatório")
                            .icon(IconName::FolderOpen)
                            .outline()
                            .small()
                            .loading(self.busy && self.preview.is_none() && self.form.is_none())
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.choose(cx))),
                    ),
            )
            .child(div().text_sm().text_color(t.text2).child(
                "Quanto você pagou para divulgar em cada canal: anúncio na Meta, no Google, post \
                 impulsionado. O app só registra; nada é gasto nem pausado daqui. Para importar, \
                 exporte o relatório por dia: no Gerenciador de Anúncios da Meta, Detalhamento › \
                 Por tempo › Dia e Exportar dados da tabela; no Google Ads, Segmentar › Tempo › \
                 Dia e Fazer download. CSV ou Excel. Importar o mesmo período de novo não duplica \
                 nada, e um dia corrigido pela plataforma fica com o valor novo.",
            ))
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .children(
                self.preview
                    .as_ref()
                    .map(|preview| self.render_preview(preview, cx)),
            )
            .when(rows.is_empty(), |section| {
                section.child(
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child("Crie um canal abaixo para lançar o gasto dele."),
                )
            })
            .children(rows)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_an_import_did_reads_as_a_sentence() {
        let imported = |added, changed, unchanged| ImportedSpends {
            added,
            changed,
            unchanged,
        };
        assert_eq!(imported_text(&imported(0, 0, 0)), "nenhum dia");
        assert_eq!(imported_text(&imported(1, 0, 0)), "1 dia novo");
        assert_eq!(
            imported_text(&imported(5, 1, 3)),
            "5 dias novos, 1 dia corrigido e 3 dias já estavam no app"
        );
    }

    #[test]
    fn a_report_refused_for_its_days_says_how_to_export_by_day() {
        for report in SpendReport::ALL {
            let text = report_failure(&SpendReportError::NotDaily(report));
            assert!(text.contains("› Dia"), "{text}");
            assert!(text.ends_with("Nada foi gravado."), "{text}");
        }
    }

    #[test]
    fn a_campaign_offers_the_channels_of_its_report_s_kind_first() {
        let channel = |name: &str, kind| PromotionChannel {
            id: RecordId::now_v7(),
            name: name.into(),
            kind,
            default_tag: name.into(),
            keywords: Vec::new(),
            telegram_chat: None,
            posting_times: Vec::new(),
            pinterest_board: None,
        };
        let channels = [
            channel("telegram", ChannelKind::TelegramChannel),
            channel("google", ChannelKind::PaidSearch),
            channel("meta", ChannelKind::PaidSocial),
        ];
        let names = |report| -> Vec<String> {
            channels_for(report, &channels)
                .into_iter()
                .map(|channel| channel.name)
                .collect()
        };
        assert_eq!(names(SpendReport::MetaAds), ["meta", "telegram", "google"]);
        assert_eq!(
            names(SpendReport::GoogleAds),
            ["google", "telegram", "meta"]
        );
    }
}
