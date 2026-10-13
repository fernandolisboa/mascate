//! Afiliação (#77): every Commission with its Platform, Program, product,
//! sale date, amount and status, filtered by status and period, the totals
//! per currency, reversals highlighted and cancelled sales listed apart,
//! and the form to record a Commission by hand for Platforms without an API
//! or a file. The rules live in Affiliate (ADR 0030).

use std::sync::Arc;

use chrono::Local;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::{
    Disableable as _, IndexPath, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, Global, Hsla, SharedString, Window, div, px};
use mascate_affiliate::{
    Affiliate, AffiliateError, AffiliatePlatform, Commission, CommissionFilter, CommissionStatus,
    CurrencyTotals, ManualCommission, totals_by_currency,
};
use mascate_kernel::{Currency, Money, RecordId, parse_amount};

use crate::appearance::{Tokens, look};
use crate::catalog::{NO_DATABASE, day};
use crate::forms::{Outcome, amount_text, day_text, input, notice, parse_day};
use crate::kit;
use crate::layout;
use crate::parts::ScreenParts;
use crate::sales::{Period, day_start};

/// The app's Commissions; absent when the database did not open.
pub struct AppAffiliate(pub Arc<Affiliate>);

impl Global for AppAffiliate {}

pub(crate) fn affiliate(cx: &App) -> Option<Arc<Affiliate>> {
    cx.try_global::<AppAffiliate>().map(|app| app.0.clone())
}

/// Counts the changes to the Commissions, so what depends on them (the
/// income Reminder) reads them again.
#[derive(Default)]
pub struct CommissionChanges(pub u64);

impl Global for CommissionChanges {}

/// The Reminders Affiliate raises now. A failure raises nothing.
pub(crate) async fn raised_reminders(affiliate: Option<Arc<Affiliate>>) -> Vec<&'static str> {
    let Some(affiliate) = affiliate else {
        return Vec::new();
    };
    affiliate.raised_reminders().await.unwrap_or_else(|error| {
        eprintln!("could not check the Commissions for the income Reminder: {error}");
        Vec::new()
    })
}

/// The statuses the main list shows: every one but cancelled, which is
/// listed apart because there never was a Commission.
const LISTED: [CommissionStatus; 4] = [
    CommissionStatus::Pending,
    CommissionStatus::Approved,
    CommissionStatus::Paid,
    CommissionStatus::Reversed,
];

/// One Platform in the form's picker.
#[derive(Clone)]
struct PlatformChoice {
    value: AffiliatePlatform,
    title: SharedString,
}

impl SearchableListItem for PlatformChoice {
    type Value = AffiliatePlatform;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &AffiliatePlatform {
        &self.value
    }
}

type PlatformSelect = Entity<SelectState<SearchableVec<PlatformChoice>>>;

/// Platforms without an API or a file come first: they are why the form
/// exists.
const FORM_PLATFORMS: [AffiliatePlatform; 9] = [
    AffiliatePlatform::MercadoLivre,
    AffiliatePlatform::Magalu,
    AffiliatePlatform::Amazon,
    AffiliatePlatform::Shopee,
    AffiliatePlatform::Hotmart,
    AffiliatePlatform::Kiwify,
    AffiliatePlatform::Monetizze,
    AffiliatePlatform::Eduzz,
    AffiliatePlatform::Other,
];

/// What one read of the screen brings.
struct Snapshot {
    listed: Vec<Commission>,
    cancelled: Vec<Commission>,
}

pub struct AffiliateScreen {
    period: Period,
    /// `None` lists every status but cancelled.
    status: Option<CommissionStatus>,
    listed: Vec<Commission>,
    cancelled: Vec<Commission>,
    platform: PlatformSelect,
    program: Entity<InputState>,
    product: Entity<InputState>,
    sold_on: Entity<InputState>,
    amount: Entity<InputState>,
    currency: Currency,
    form_status: CommissionStatus,
    /// The Commission the form is editing, if any.
    editing: Option<RecordId>,
    /// The Commission asking to confirm its deletion.
    deleting: Option<RecordId>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    form: Option<Outcome>,
    outcome: Option<Outcome>,
}

impl AffiliateScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let choices = SearchableVec::new(
            FORM_PLATFORMS
                .map(|value| PlatformChoice {
                    value,
                    title: value.name().into(),
                })
                .to_vec(),
        );
        let platform = cx.new(|cx| SelectState::new(choices, Some(IndexPath::new(0)), window, cx));
        let sold_on = input("dd/mm/aaaa", window, cx);
        sold_on.update(cx, |field, cx| field.set_value(today(), window, cx));
        let mut screen = Self {
            period: Period::ThisMonth,
            status: None,
            listed: Vec::new(),
            cancelled: Vec::new(),
            platform,
            program: input("Igual à plataforma", window, cx),
            product: input("Fone Bluetooth", window, cx),
            sold_on,
            amount: input("12,40", window, cx),
            currency: Currency::Brl,
            form_status: CommissionStatus::Pending,
            editing: None,
            deleting: None,
            busy: false,
            reads: 0,
            form: None,
            outcome: None,
        };
        screen.refresh(window, cx);
        screen
    }

    /// Reads the Commissions again, as when the screen comes into view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx, |_| async { Ok(None) }, |_, _: (), _, _| {});
    }

    /// Runs `change` off the UI thread, then reads the Commissions of the
    /// period again and hands what the change returned to `then`.
    fn run<T, F, Fut>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: F,
        then: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) where
        T: Send + 'static,
        F: FnOnce(Arc<Affiliate>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<T>, AffiliateError>> + Send,
    {
        let Some(affiliate) = affiliate(cx) else {
            return;
        };
        let sold = self.period.range(Local::now().date_naive());
        let statuses = self
            .status
            .map_or_else(|| LISTED.to_vec(), |status| vec![status]);
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(affiliate.clone()).await;
            let snapshot = async {
                let listed = affiliate
                    .commissions(&CommissionFilter {
                        statuses,
                        sold: sold.clone(),
                    })
                    .await?;
                let cancelled = affiliate
                    .commissions(&CommissionFilter {
                        statuses: vec![CommissionStatus::Cancelled],
                        sold,
                    })
                    .await?;
                Ok::<_, AffiliateError>(Snapshot { listed, cancelled })
            }
            .await;
            (changed, snapshot)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (changed, snapshot) = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reads == read {
                    this.busy = false;
                    match snapshot {
                        Ok(snapshot) => {
                            this.listed = snapshot.listed;
                            this.cancelled = snapshot.cancelled;
                        }
                        Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                    }
                }
                match changed {
                    Ok(Some(value)) => {
                        cx.update_default_global::<CommissionChanges, _>(|changes, _| {
                            changes.0 += 1
                        });
                        then(this, value, window, cx);
                    }
                    Ok(None) => {}
                    Err(error) => this.form = Some(Outcome::Failed(failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn pick_period(&mut self, period: Period, window: &mut Window, cx: &mut Context<Self>) {
        self.period = period;
        self.refresh(window, cx);
    }

    fn pick_status(
        &mut self,
        status: Option<CommissionStatus>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.status = status;
        self.refresh(window, cx);
    }

    /// The Commission the form describes, or why it cannot be one yet.
    fn written(&self, cx: &App) -> Result<ManualCommission, &'static str> {
        let platform = *self
            .platform
            .read(cx)
            .selected_value()
            .ok_or("Escolha a plataforma.")?;
        let product = self.product.read(cx).value().trim().to_owned();
        if product.is_empty() {
            return Err("Digite o produto da venda.");
        }
        let sold_on = parse_day(&self.sold_on.read(cx).value())
            .ok_or("Digite a data da venda como 01/10/2026.")?;
        let amount = parse_amount(&self.amount.read(cx).value())
            .filter(|amount| !amount.is_sign_negative())
            .ok_or("Digite a comissão como 12,40.")?;
        Ok(ManualCommission {
            platform,
            program: self.program.read(cx).value().trim().to_owned(),
            product,
            sold_at: day_start(sold_on),
            amount: Money::new(amount, self.currency),
            status: self.form_status,
        })
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let manual = match self.written(cx) {
            Ok(manual) => manual,
            Err(why) => {
                self.form = Some(Outcome::Failed(why.into()));
                cx.notify();
                return;
            }
        };
        self.form = None;
        self.outcome = None;
        let editing = self.editing;
        self.run(
            window,
            cx,
            move |affiliate| async move {
                match editing {
                    Some(id) => affiliate.edit_manual(id, manual).await.map(Some),
                    None => affiliate.record_manual(manual).await.map(Some),
                }
            },
            move |this, _: Commission, window, cx| {
                this.clear_form(window, cx);
                this.form = Some(Outcome::Done(
                    match editing {
                        Some(_) => "Comissão atualizada.",
                        None => "Comissão lançada.",
                    }
                    .into(),
                ));
            },
        );
    }

    fn clear_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        self.form_status = CommissionStatus::Pending;
        self.sold_on
            .update(cx, |field, cx| field.set_value(today(), window, cx));
        for field in [&self.program, &self.product, &self.amount] {
            field.update(cx, |field, cx| field.set_value("", window, cx));
        }
    }

    /// Loads a manual Commission into the form to change it.
    fn edit(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(commission) = self
            .listed
            .iter()
            .chain(&self.cancelled)
            .find(|commission| commission.id == id)
            .cloned()
        else {
            return;
        };
        self.deleting = None;
        self.form = None;
        self.outcome = None;
        self.editing = Some(id);
        self.currency = commission.amount.currency();
        self.form_status = commission.status;
        self.platform.update(cx, |select, cx| {
            select.set_selected_value(&commission.platform, window, cx)
        });
        let program = if commission.program_name == commission.platform.name() {
            String::new()
        } else {
            commission.program_name.clone()
        };
        let sold_on = day_text(commission.sold_at.with_timezone(&Local).date_naive());
        let amount = amount_text(commission.amount);
        for (field, value) in [
            (&self.program, program),
            (&self.product, commission.product.clone()),
            (&self.sold_on, sold_on),
            (&self.amount, amount),
        ] {
            field.update(cx, |field, cx| field.set_value(value, window, cx));
        }
        cx.notify();
    }

    fn delete(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.outcome = None;
        self.run(
            window,
            cx,
            move |affiliate| async move { affiliate.delete_manual(id).await.map(Some) },
            move |this, (), window, cx| {
                this.deleting = None;
                if this.editing == Some(id) {
                    this.clear_form(window, cx);
                }
                this.outcome = Some(Outcome::Done("Comissão excluída.".into()));
            },
        );
    }

    fn render_filters(&self, cx: &mut Context<Self>) -> AnyElement {
        let chip = |id: (&'static str, usize), label: &'static str, on: bool| {
            let button = Button::new(id).label(label).small();
            if on { button.primary() } else { button.ghost() }
        };
        let statuses =
            [None]
                .into_iter()
                .chain(LISTED.map(Some))
                .enumerate()
                .map(|(index, status)| {
                    chip(
                        ("status", index),
                        status_filter_name(status),
                        self.status == status,
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.pick_status(status, window, cx)
                        }),
                    )
                });
        let periods = Period::ALL.into_iter().map(|period| {
            chip(
                ("period", period as usize),
                period.name(),
                self.period == period,
            )
            .on_click(cx.listener(move |this, _, window, cx| this.pick_period(period, window, cx)))
        });
        h_flex()
            .flex_wrap()
            .gap_4()
            .child(h_flex().gap_1().children(periods))
            .child(h_flex().gap_1().children(statuses))
            .into_any_element()
    }

    fn render_totals(&self, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let totals = totals_by_currency(&self.listed);
        if totals.is_empty() {
            return div().into_any_element();
        }
        let figure = |label: String, text: String, ink: Hsla| {
            v_flex()
                .flex_1()
                .min_w(px(150.))
                .gap_1()
                .p_4()
                .rounded(t.radius_lg)
                .border(t.border_width)
                .border_color(t.frame)
                .bg(t.surface)
                .child(div().text_sm().text_color(t.text2).child(label))
                .child(
                    div()
                        .text_xl()
                        .font_semibold()
                        .whitespace_nowrap()
                        .text_color(ink)
                        .child(text),
                )
        };
        let several = totals.len() > 1;
        let label = |name: &str, total: &CurrencyTotals| {
            if several {
                format!("{name} em {}", total.currency.code())
            } else {
                name.to_owned()
            }
        };
        v_flex()
            .gap_2()
            .children(totals.iter().map(|total| {
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .child(figure(
                        label("Ganho", total),
                        total.earned().to_pt_br(),
                        t.text,
                    ))
                    .child(figure(
                        label("Pendente", total),
                        total.pending.to_pt_br(),
                        t.text,
                    ))
                    .child(figure(label("Paga", total), total.paid.to_pt_br(), t.text))
                    .child(figure(
                        label("Estornada", total),
                        total.reversed.to_pt_br(),
                        if total.reversed.amount().is_zero() {
                            t.text
                        } else {
                            t.danger
                        },
                    ))
            }))
            .child(div().text_xs().text_color(t.text2).child(
                "Ganho soma as comissões aprovadas e pagas; estornos ficam fora dele. Cada \
                 moeda soma à parte, sem conversão.",
            ))
            .into_any_element()
    }

    fn render_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, input: AnyElement| {
            v_flex()
                .gap_1()
                .child(div().text_sm().font_medium().child(label))
                .child(input)
        };
        let currency = |value: Currency| {
            let button = Button::new(("commission-currency", value as usize))
                .label(value.symbol())
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.currency = value;
                    cx.notify();
                }));
            if self.currency == value {
                button.primary()
            } else {
                button.outline()
            }
        };
        let status = |value: CommissionStatus| {
            let button = Button::new(("commission-status", value as usize))
                .label(value.name())
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.form_status = value;
                    cx.notify();
                }));
            if self.form_status == value {
                button.primary()
            } else {
                button.outline()
            }
        };
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if self.editing.is_some() {
                t.accent_edge
            } else {
                t.frame
            })
            .bg(t.surface)
            .child(kit::section_heading(if self.editing.is_some() {
                "Editar comissão"
            } else {
                "Lançar comissão"
            }))
            .child(div().text_sm().text_color(t.text2).child(
                "Para plataformas sem API nem arquivo, como Mercado Livre e Magalu. Só as \
                 comissões lançadas à mão podem ser editadas.",
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_end()
                    .child(div().w(px(180.)).child(field(
                        "Plataforma",
                        Select::new(&self.platform).small().into_any_element(),
                    )))
                    .child(div().w(px(200.)).child(field(
                        "Programa",
                        Input::new(&self.program).small().into_any_element(),
                    )))
                    .child(div().w(px(260.)).child(field(
                        "Produto",
                        Input::new(&self.product).small().into_any_element(),
                    ))),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_end()
                    .child(div().w(px(130.)).child(field(
                        "Data da venda",
                        Input::new(&self.sold_on).small().into_any_element(),
                    )))
                    .child(div().w(px(130.)).child(field(
                        "Comissão",
                        Input::new(&self.amount).small().into_any_element(),
                    )))
                    .child(field(
                        "Moeda",
                        h_flex()
                            .gap_1()
                            .child(currency(Currency::Brl))
                            .child(currency(Currency::Usd))
                            .into_any_element(),
                    ))
                    .child(field(
                        "Status",
                        h_flex()
                            .gap_1()
                            .children(CommissionStatus::ALL.map(status))
                            .into_any_element(),
                    )),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("save-commission")
                            .label(if self.editing.is_some() {
                                "Salvar alterações"
                            } else {
                                "Lançar comissão"
                            })
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .when(self.editing.is_some(), |row| {
                        row.child(
                            Button::new("stop-editing-commission")
                                .label("Descartar alterações")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_form(window, cx);
                                    this.form = None;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .children(self.form.as_ref().map(|outcome| notice(outcome, cx)))
            .into_any_element()
    }

    fn render_commission(&self, commission: &Commission, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let id = commission.id;
        let reversed = commission.status == CommissionStatus::Reversed;
        let mut facts = vec![
            format!("venda em {}", day(commission.sold_at)),
            format!(
                "{} desde {}",
                commission.status.name(),
                day(commission.status_since)
            ),
        ];
        if !commission.platform_status.is_empty() {
            facts.push(format!("na plataforma: {}", commission.platform_status));
        }
        if let Some(tracking) = &commission.tracking {
            facts.push(format!("marcação {tracking}"));
        }
        let program = if commission.program_name == commission.platform.name() {
            String::new()
        } else {
            format!("{} · ", commission.program_name)
        };
        let actions = commission.manual.then(|| {
            let button = |name: &'static str, label: &'static str| {
                Button::new(SharedString::from(format!("{name}-{id}")))
                    .label(label)
                    .ghost()
                    .small()
                    .disabled(self.busy)
            };
            if self.deleting == Some(id) {
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_sm().child("Excluir esta comissão?"))
                    .child(
                        button("confirm-delete-commission", "Excluir")
                            .danger()
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.delete(id, window, cx)),
                            ),
                    )
                    .child(button("keep-commission", "Voltar").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.deleting = None;
                            cx.notify();
                        },
                    )))
            } else {
                h_flex()
                    .gap_1()
                    .child(button("edit-commission", "Editar").on_click(
                        cx.listener(move |this, _, window, cx| this.edit(id, window, cx)),
                    ))
                    .child(button("delete-commission", "Excluir").on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.deleting = Some(id);
                            cx.notify();
                        },
                    )))
            }
        });
        h_flex()
            .gap_3()
            .items_center()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if reversed {
                t.danger
            } else if self.editing == Some(id) {
                t.accent_edge
            } else {
                t.frame
            })
            .bg(t.surface)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(kit::tag(commission.platform.name(), t.accent_text, cx))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_medium()
                                    .child(format!("{program}{}", commission.product)),
                            )
                            .child(kit::tag(
                                commission.status.name(),
                                status_ink(commission.status, &t),
                                cx,
                            ))
                            .when(commission.manual, |row| {
                                row.child(kit::tag("à mão", t.text2, cx))
                            }),
                    )
                    .child(div().text_xs().text_color(t.text2).child(facts.join(" · "))),
            )
            .children(actions)
            .child(
                div()
                    .font_semibold()
                    .whitespace_nowrap()
                    .when(reversed, |amount| {
                        amount.text_color(t.danger).line_through()
                    })
                    .child(commission.amount.to_pt_br()),
            )
            .into_any_element()
    }
}

fn status_filter_name(status: Option<CommissionStatus>) -> &'static str {
    match status {
        None => "Todas",
        Some(CommissionStatus::Pending) => "Pendentes",
        Some(CommissionStatus::Approved) => "Aprovadas",
        Some(CommissionStatus::Paid) => "Pagas",
        Some(CommissionStatus::Reversed) => "Estornadas",
        Some(CommissionStatus::Cancelled) => "Canceladas",
    }
}

fn status_ink(status: CommissionStatus, t: &Tokens) -> Hsla {
    match status {
        CommissionStatus::Pending => t.text2,
        CommissionStatus::Approved => t.accent_text,
        CommissionStatus::Paid => t.success,
        CommissionStatus::Reversed => t.danger,
        CommissionStatus::Cancelled => t.text2,
    }
}

fn today() -> String {
    Local::now().format("%d/%m/%Y").to_string()
}

/// "1 comissão", "3 comissões".
fn commissions_text(count: usize) -> String {
    match count {
        1 => "1 comissão".into(),
        count => format!("{count} comissões"),
    }
}

/// Why an action on the Commissions failed, as the owner reads it.
pub(crate) fn failure(error: &AffiliateError) -> String {
    match error {
        AffiliateError::NotFound => "Essa comissão mudou; a lista foi atualizada.".into(),
        AffiliateError::NotManual => {
            "Só comissões lançadas à mão podem ser editadas ou excluídas; as outras seguem a \
             plataforma."
                .into()
        }
        AffiliateError::NoProduct => "Digite o produto da venda.".into(),
        AffiliateError::Negative => "A comissão não pode ser negativa.".into(),
        AffiliateError::Platform(error) => format!("A plataforma não respondeu: {error}"),
        AffiliateError::Unreadable(_) | AffiliateError::Sql(_) => {
            format!("Não consegui ler ou gravar as comissões no banco: {error}")
        }
    }
}

impl Render for AffiliateScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let mut parts = ScreenParts::new("Afiliação");
        if affiliate(cx).is_none() {
            parts
                .notices
                .push(kit::error_notice(NO_DATABASE, cx).into_any_element());
            return layout::screen(parts, cx);
        }
        parts.actions.push(self.render_filters(cx));
        parts
            .notices
            .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        parts.content.push(self.render_totals(cx));
        let listed: Vec<AnyElement> = self
            .listed
            .iter()
            .map(|commission| self.render_commission(commission, cx))
            .collect();
        parts.content.push(
            v_flex()
                .gap_3()
                .child(kit::section_heading(commissions_text(self.listed.len())))
                .when(listed.is_empty(), |list| {
                    list.child(div().text_sm().text_color(t.text2).child(
                        "Nenhuma comissão neste período. Lance abaixo as de plataformas sem \
                         API; as das outras chegam pela conexão de cada uma.",
                    ))
                })
                .children(listed)
                .into_any_element(),
        );
        if !self.cancelled.is_empty() {
            let cancelled: Vec<AnyElement> = self
                .cancelled
                .iter()
                .map(|commission| self.render_commission(commission, cx))
                .collect();
            parts.content.push(
                v_flex()
                    .gap_3()
                    .child(kit::section_heading(format!(
                        "Canceladas ({})",
                        self.cancelled.len()
                    )))
                    .child(div().text_sm().text_color(t.text2).child(
                        "Vendas que não se concretizaram, como um boleto vencido: nunca houve \
                         comissão, então não contam como estorno nem entram nos totais.",
                    ))
                    .children(cancelled)
                    .into_any_element(),
            );
        }
        parts.content.push(self.render_form(cx));
        layout::screen(parts, cx)
    }
}
