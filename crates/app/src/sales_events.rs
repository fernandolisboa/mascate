//! The Sales Events calendar on Divulgação (#105): the commercial dates by
//! next date, to add, edit and delete. The calendar lives in the Marketing
//! (ADR 0031, decision 9) and only plans: Hoje shows the events inside
//! their lead time, and scheduling an Offer Post offers their dates.

use std::sync::Arc;

use chrono::{NaiveDate, Utc, Weekday};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, Global, Window, div, px};
use mascate_kernel::{RecordId, brasilia};
use mascate_marketing::{
    DateRule, EVENT_NAME_MAX_CHARS, EVENT_NOTES_MAX_CHARS, EventError, MAX_DAYS_AFTER_THANKSGIVING,
    MAX_LEAD_DAYS, NewSalesEvent, SalesEvent, SalesEvents,
};

use crate::appearance::look;
use crate::forms::{Outcome, day_text, input, notice, parse_day};
use crate::kit;
use crate::post_queue::short_weekday;
use crate::sales::table_row;

/// The app's Sales Events; absent when the database did not open.
pub struct AppSalesEvents(pub Arc<SalesEvents>);

impl Global for AppSalesEvents {}

pub(crate) fn sales_events(cx: &App) -> Option<Arc<SalesEvents>> {
    cx.try_global::<AppSalesEvents>().map(|app| app.0.clone())
}

/// Counts the changes to the calendar, so what lists it reads it again.
#[derive(Default)]
pub struct EventChanges(pub u64);

impl Global for EventChanges {}

pub(crate) fn failure(error: &EventError) -> String {
    match error {
        EventError::BadName => {
            format!("Dê um nome à data, com até {EVENT_NAME_MAX_CHARS} caracteres.")
        }
        EventError::BadRule => "Essa regra não cai em nenhum dia: confira o dia e o mês.".into(),
        EventError::BadLeadDays => {
            format!("A antecedência vai de 0 a {MAX_LEAD_DAYS} dias.")
        }
        EventError::LongNotes => {
            format!("As notas têm até {EVENT_NOTES_MAX_CHARS} caracteres.")
        }
        EventError::NotFound => "Essa data mudou; o calendário foi atualizado.".into(),
        EventError::Unreadable(_) | EventError::Sql(_) => {
            format!("Não consegui ler ou gravar o calendário no banco: {error}")
        }
    }
}

/// The day it is now on Brasília's clock.
pub(crate) fn today() -> NaiveDate {
    Utc::now().with_timezone(&brasilia()).date_naive()
}

const MONTHS: [&str; 12] = [
    "janeiro",
    "fevereiro",
    "março",
    "abril",
    "maio",
    "junho",
    "julho",
    "agosto",
    "setembro",
    "outubro",
    "novembro",
    "dezembro",
];

const WEEKDAYS: [Weekday; 7] = [
    Weekday::Sun,
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
];

fn month_name(month: u32) -> &'static str {
    MONTHS
        .get(month.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("?")
}

fn weekday_name(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "segunda",
        Weekday::Tue => "terça",
        Weekday::Wed => "quarta",
        Weekday::Thu => "quinta",
        Weekday::Fri => "sexta",
        Weekday::Sat => "sábado",
        Weekday::Sun => "domingo",
    }
}

/// "1º", "2º".
fn ordinal(nth: u8) -> String {
    format!("{nth}º")
}

/// A date rule as the owner reads it: "todo ano em 15/03", "2º domingo de
/// maio", "dia seguinte à 4ª quinta de novembro", "só em 08/07/2027".
pub(crate) fn rule_text(rule: DateRule) -> String {
    match rule {
        DateRule::Yearly { day, month } => format!("todo ano em {day:02}/{month:02}"),
        DateRule::NthWeekday {
            nth,
            weekday,
            month,
        } => format!(
            "{} {} de {}",
            ordinal(nth),
            weekday_name(weekday),
            month_name(month)
        ),
        DateRule::AfterThanksgiving { days: 0 } => "4ª quinta de novembro".into(),
        DateRule::AfterThanksgiving { days: 1 } => "dia seguinte à 4ª quinta de novembro".into(),
        DateRule::AfterThanksgiving { days } => {
            format!("{days} dias depois da 4ª quinta de novembro")
        }
        DateRule::Once(date) => format!("só em {}", day_text(date)),
    }
}

/// "sex, 27/11/2026".
pub(crate) fn event_date_text(date: NaiveDate) -> String {
    use chrono::Datelike as _;
    format!("{}, {}", short_weekday(date.weekday()), day_text(date))
}

/// "hoje", "amanhã", "em 12 dias".
pub(crate) fn days_away_text(days: u32) -> String {
    match days {
        0 => "hoje".into(),
        1 => "amanhã".into(),
        days => format!("em {days} dias"),
    }
}

/// A day and month as the owner types them: `15/03`, `15/3`.
fn typed_day_month(text: &str) -> Option<(u32, u32)> {
    let (day, month) = text.trim().split_once('/')?;
    Some((day.trim().parse().ok()?, month.trim().parse().ok()?))
}

/// Which kind of rule the form writes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleKind {
    Yearly,
    NthWeekday,
    AfterThanksgiving,
    Once,
}

impl RuleKind {
    const ALL: [RuleKind; 4] = [
        RuleKind::Yearly,
        RuleKind::NthWeekday,
        RuleKind::AfterThanksgiving,
        RuleKind::Once,
    ];

    fn name(self) -> &'static str {
        match self {
            RuleKind::Yearly => "Todo ano no mesmo dia",
            RuleKind::NthWeekday => "Dia da semana do mês",
            RuleKind::AfterThanksgiving => "Depois da 4ª quinta de novembro",
            RuleKind::Once => "Data única",
        }
    }

    fn of(rule: DateRule) -> Self {
        match rule {
            DateRule::Yearly { .. } => RuleKind::Yearly,
            DateRule::NthWeekday { .. } => RuleKind::NthWeekday,
            DateRule::AfterThanksgiving { .. } => RuleKind::AfterThanksgiving,
            DateRule::Once(_) => RuleKind::Once,
        }
    }
}

pub struct CalendarSection {
    events: Vec<SalesEvent>,
    /// The day the list was read on, Brasília's clock.
    today: NaiveDate,
    name: Entity<InputState>,
    /// The day and month of a yearly date, or the day of a single date.
    date: Entity<InputState>,
    /// Days after the fourth Thursday of November.
    days_after: Entity<InputState>,
    lead_days: Entity<InputState>,
    notes: Entity<InputState>,
    kind: RuleKind,
    nth: u8,
    weekday: Weekday,
    month: u32,
    /// The event the form is editing, if any.
    editing: Option<RecordId>,
    /// The event asking to confirm its deletion.
    deleting: Option<RecordId>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    form: Option<Outcome>,
    outcome: Option<Outcome>,
}

impl CalendarSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut section = Self {
            events: Vec::new(),
            today: today(),
            name: input("Dia do Frete Grátis", window, cx),
            date: input("15/03", window, cx),
            days_after: input("1", window, cx),
            lead_days: input("7", window, cx),
            notes: input("Cupons, comissão extra, o que preparar", window, cx),
            kind: RuleKind::Yearly,
            nth: 2,
            weekday: Weekday::Sun,
            month: 5,
            editing: None,
            deleting: None,
            busy: false,
            reads: 0,
            form: None,
            outcome: None,
        };
        section.refresh(window, cx);
        section
    }

    /// Reads the calendar again, as when the screen comes into view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx, |_| async { Ok(()) }, |_, _, _, _| {});
    }

    /// Runs `change` off the UI thread, then reads the calendar again and
    /// hands `done` what `change` gave.
    fn run<T, F, Fut, D>(&mut self, window: &mut Window, cx: &mut Context<Self>, change: F, done: D)
    where
        T: Send + 'static,
        F: FnOnce(Arc<SalesEvents>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, EventError>> + Send,
        D: FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    {
        let Some(events) = sales_events(cx) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(events.clone()).await;
            (changed, events.events(Utc::now()).await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (changed, read_back) = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reads == read {
                    this.busy = false;
                    match read_back {
                        Ok(events) => {
                            this.events = events;
                            this.today = today();
                        }
                        Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                    }
                }
                match changed {
                    Ok(value) => done(this, value, window, cx),
                    Err(error) => this.form = Some(Outcome::Failed(failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The event as typed; `Err` says what to fix in a field the Marketing
    /// cannot read.
    fn typed(&self, cx: &App) -> Result<NewSalesEvent, String> {
        let date = self.date.read(cx).value().to_string();
        let rule = match self.kind {
            RuleKind::Yearly => {
                let (day, month) =
                    typed_day_month(&date).ok_or("Digite o dia e o mês como 15/03.".to_owned())?;
                DateRule::Yearly { day, month }
            }
            RuleKind::NthWeekday => DateRule::NthWeekday {
                nth: self.nth,
                weekday: self.weekday,
                month: self.month,
            },
            RuleKind::AfterThanksgiving => {
                let days = self
                    .days_after
                    .read(cx)
                    .value()
                    .trim()
                    .parse()
                    .map_err(|_| {
                        format!(
                            "Digite quantos dias depois da 4ª quinta de novembro, de 0 a \
                         {MAX_DAYS_AFTER_THANKSGIVING}: 1 é a Black Friday, 4 a Cyber Monday."
                        )
                    })?;
                DateRule::AfterThanksgiving { days }
            }
            RuleKind::Once => {
                DateRule::Once(parse_day(&date).ok_or("Digite a data como 08/07/2027.".to_owned())?)
            }
        };
        let lead = self.lead_days.read(cx).value().trim().to_owned();
        let lead_days = if lead.is_empty() {
            0
        } else {
            lead.parse().map_err(|_| {
                format!("A antecedência é um número de dias, de 0 a {MAX_LEAD_DAYS}.")
            })?
        };
        Ok(NewSalesEvent {
            name: self.name.read(cx).value().to_string(),
            rule,
            lead_days,
            notes: self.notes.read(cx).value().to_string(),
        })
    }

    fn changed(cx: &mut Context<Self>) {
        cx.update_default_global::<EventChanges, _>(|changes, _| changes.0 += 1);
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = match self.typed(cx) {
            Ok(typed) => typed,
            Err(why) => {
                self.form = Some(Outcome::Failed(why.into()));
                cx.notify();
                return;
            }
        };
        let editing = self.editing;
        self.form = None;
        self.run(
            window,
            cx,
            move |events| async move {
                match editing {
                    Some(id) => events.update(id, &typed).await,
                    None => events.create(&typed).await,
                }
            },
            move |this, saved: SalesEvent, window, cx| {
                let next = saved.next_date(today()).map_or_else(
                    || "a data já passou".to_owned(),
                    |date| format!("a próxima é {}", event_date_text(date)),
                );
                this.outcome = Some(Outcome::Done(
                    format!(
                        "{} {}: {next}.",
                        saved.name,
                        if editing.is_some() {
                            "salva"
                        } else {
                            "no calendário"
                        }
                    )
                    .into(),
                ));
                this.clear(window, cx);
                Self::changed(cx);
            },
        );
    }

    fn edit(&mut self, event: &SalesEvent, window: &mut Window, cx: &mut Context<Self>) {
        let fill = |field: &Entity<InputState>, text: String, window: &mut Window, cx: &mut App| {
            field.update(cx, |field, cx| field.set_value(text, window, cx));
        };
        fill(&self.name, event.name.clone(), window, cx);
        fill(&self.lead_days, event.lead_days.to_string(), window, cx);
        fill(&self.notes, event.notes.clone(), window, cx);
        let (date, days_after) = match event.rule {
            DateRule::Yearly { day, month } => (format!("{day:02}/{month:02}"), String::new()),
            DateRule::Once(date) => (day_text(date), String::new()),
            DateRule::AfterThanksgiving { days } => (String::new(), days.to_string()),
            DateRule::NthWeekday {
                nth,
                weekday,
                month,
            } => {
                (self.nth, self.weekday, self.month) = (nth, weekday, month);
                (String::new(), String::new())
            }
        };
        fill(&self.date, date, window, cx);
        fill(&self.days_after, days_after, window, cx);
        self.kind = RuleKind::of(event.rule);
        self.editing = Some(event.id);
        self.deleting = None;
        self.form = None;
        cx.notify();
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for field in [
            &self.name,
            &self.date,
            &self.days_after,
            &self.lead_days,
            &self.notes,
        ] {
            field.update(cx, |field, cx| field.set_value("", window, cx));
        }
        self.kind = RuleKind::Yearly;
        self.editing = None;
        self.form = None;
        cx.notify();
    }

    fn delete(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.deleting = None;
        let name = self
            .events
            .iter()
            .find(|event| event.id == id)
            .map_or_else(String::new, |event| event.name.clone());
        self.run(
            window,
            cx,
            move |events| async move { events.delete(id).await },
            move |this, _, window, cx| {
                this.outcome = Some(Outcome::Done(
                    format!("{name} saiu do calendário e não volta sozinha.").into(),
                ));
                if this.editing == Some(id) {
                    this.clear(window, cx);
                }
                Self::changed(cx);
            },
        );
    }

    fn render_event(&self, index: usize, event: &SalesEvent, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let next = event.next_date(self.today);
        let when = match next {
            Some(date) => {
                let days = u32::try_from((date - self.today).num_days()).unwrap_or(0);
                format!("{} · {}", event_date_text(date), days_away_text(days))
            }
            None => "já passou".to_owned(),
        };
        let warns = match event.lead_days {
            0 => "avisa só no dia".to_owned(),
            1 => "avisa 1 dia antes".to_owned(),
            days => format!("avisa {days} dias antes"),
        };
        let detail = format!("{} · {warns}", rule_text(event.rule));
        let edited = event.clone();
        let id = event.id;
        let actions = if self.deleting == Some(id) {
            h_flex()
                .gap_2()
                .items_center()
                .child(div().text_xs().child("Apagar a data?"))
                .child(
                    Button::new(("confirm-delete-event", index))
                        .label("Apagar")
                        .danger()
                        .small()
                        .disabled(self.busy)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.delete(id, window, cx)),
                        ),
                )
                .child(
                    Button::new(("keep-event", index))
                        .label("Manter")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.deleting = None;
                            cx.notify();
                        })),
                )
        } else {
            h_flex()
                .gap_2()
                .child(
                    Button::new(("edit-event", index))
                        .label("Editar")
                        .outline()
                        .small()
                        .disabled(self.busy)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.edit(&edited, window, cx)),
                        ),
                )
                .child(
                    Button::new(("delete-event", index))
                        .label("Apagar")
                        .ghost()
                        .small()
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.deleting = Some(id);
                            cx.notify();
                        })),
                )
        };
        table_row(&t)
            .flex_wrap()
            .child(kit::tag(
                when,
                if next.is_some() {
                    t.accent_text
                } else {
                    t.text2
                },
                cx,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(220.))
                    .child(div().font_medium().truncate().child(event.name.clone()))
                    .child(div().text_xs().text_color(t.text2).child(detail))
                    .when(!event.notes.is_empty(), |cell| {
                        cell.child(div().text_xs().child(event.notes.clone()))
                    }),
            )
            .child(actions)
            .into_any_element()
    }

    fn chips<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        options: impl IntoIterator<Item = (T, String)>,
        picked: T,
        pick: impl Fn(&mut Self, T) + Copy + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .flex_wrap()
            .gap_1()
            .children(
                options
                    .into_iter()
                    .enumerate()
                    .map(|(index, (value, label))| {
                        let chip = Button::new((id, index))
                            .label(label)
                            .small()
                            .disabled(self.busy);
                        if value == picked {
                            chip.primary()
                        } else {
                            chip.ghost()
                        }
                        .on_click(cx.listener(move |this, _, _, cx| {
                            pick(this, value);
                            cx.notify();
                        }))
                    }),
            )
            .into_any_element()
    }

    fn render_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, input: &Entity<InputState>, width: f32| {
            v_flex()
                .gap_1()
                .child(div().text_sm().child(label))
                .child(div().w(px(width)).child(Input::new(input).small()))
        };
        let kinds = self.chips(
            "event-rule",
            RuleKind::ALL.map(|kind| (kind, kind.name().to_owned())),
            self.kind,
            |this, kind| this.kind = kind,
            cx,
        );
        let rule: AnyElement = match self.kind {
            RuleKind::Yearly => field("Dia e mês", &self.date, 120.)
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text2)
                        .child("Como 15/03 ou 11/11. 29/02 só cai em ano bissexto."),
                )
                .into_any_element(),
            RuleKind::Once => field("Data", &self.date, 160.)
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text2)
                        .child("Como 08/07/2027, para uma data anunciada só para aquele ano."),
                )
                .into_any_element(),
            RuleKind::AfterThanksgiving => field("Dias depois", &self.days_after, 120.)
                .child(div().text_xs().text_color(t.text2).child(
                    "A 4ª quinta-feira de novembro é o Dia de Ação de Graças dos Estados Unidos: \
                     1 dia depois é a Black Friday, 4 dias depois a Cyber Monday.",
                ))
                .into_any_element(),
            RuleKind::NthWeekday => v_flex()
                .gap_1()
                .child(div().text_sm().child("Qual"))
                .child(self.chips(
                    "event-nth",
                    (1..=4).map(|nth| (nth, ordinal(nth))),
                    self.nth,
                    |this, nth| this.nth = nth,
                    cx,
                ))
                .child(self.chips(
                    "event-weekday",
                    WEEKDAYS.map(|weekday| (weekday, weekday_name(weekday).to_owned())),
                    self.weekday,
                    |this, weekday| this.weekday = weekday,
                    cx,
                ))
                .child(self.chips(
                    "event-month",
                    (1..=12).map(|month| (month, month_name(month).to_owned())),
                    self.month,
                    |this, month| this.month = month,
                    cx,
                ))
                .child(div().text_xs().text_color(t.text2).child(format!(
                    "Cai no {} {} de {}, como o Dia das Mães no 2º domingo de maio.",
                    ordinal(self.nth),
                    weekday_name(self.weekday),
                    month_name(self.month)
                )))
                .into_any_element(),
        };
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(kit::section_heading(if self.editing.is_some() {
                "Editar data"
            } else {
                "Nova data"
            }))
            .child(field("Nome", &self.name, 360.))
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child("Quando"))
                    .child(kinds),
            )
            .child(rule)
            .child(field("Antecedência em dias", &self.lead_days, 120.).child(
                div().text_xs().text_color(t.text2).child(
                    "Quantos dias antes a data aparece em Hoje, para preparar promoções e posts. \
                     0 avisa só no dia.",
                ),
            ))
            .child(field("Notas", &self.notes, 480.))
            .children(self.form.as_ref().map(|outcome| notice(outcome, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("save-event")
                            .label(if self.editing.is_some() {
                                "Salvar data"
                            } else {
                                "Adicionar data"
                            })
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .when(self.editing.is_some(), |buttons| {
                        buttons.child(
                            Button::new("cancel-event")
                                .label("Cancelar")
                                .ghost()
                                .small()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.clear(window, cx)),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl Render for CalendarSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let rows: Vec<AnyElement> = self
            .events
            .clone()
            .iter()
            .enumerate()
            .map(|(index, event)| self.render_event(index, event, cx))
            .collect();
        v_flex()
            .gap_3()
            .child(kit::section_heading("Calendário"))
            .child(div().text_sm().text_color(t.text2).child(
                "As datas comerciais em que vale planejar promoções e posts, pela próxima data. \
                 Cada uma aparece em Hoje quando entra na antecedência, e a data é oferecida ao \
                 agendar um post. Nada é criado sozinho a partir de uma data.",
            ))
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .when(rows.is_empty(), |list| {
                list.child(
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child("Nenhuma data no calendário: adicione abaixo."),
                )
            })
            .children(rows)
            .child(self.render_form(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn rules_read_the_way_the_owner_says_them() {
        assert_eq!(
            rule_text(DateRule::Yearly { day: 5, month: 3 }),
            "todo ano em 05/03"
        );
        assert_eq!(
            rule_text(DateRule::NthWeekday {
                nth: 2,
                weekday: Weekday::Sun,
                month: 5
            }),
            "2º domingo de maio"
        );
        assert_eq!(
            rule_text(DateRule::AfterThanksgiving { days: 1 }),
            "dia seguinte à 4ª quinta de novembro"
        );
        assert_eq!(
            rule_text(DateRule::AfterThanksgiving { days: 4 }),
            "4 dias depois da 4ª quinta de novembro"
        );
        assert_eq!(
            rule_text(DateRule::Once(date(2027, 7, 8))),
            "só em 08/07/2027"
        );
        assert_eq!(event_date_text(date(2026, 11, 27)), "sex, 27/11/2026");
        assert_eq!(days_away_text(0), "hoje");
        assert_eq!(days_away_text(1), "amanhã");
        assert_eq!(days_away_text(21), "em 21 dias");
    }

    #[test]
    fn a_day_and_month_read_as_typed() {
        assert_eq!(typed_day_month("15/03"), Some((15, 3)));
        assert_eq!(typed_day_month(" 1 / 12 "), Some((1, 12)));
        assert_eq!(typed_day_month("15-03"), None);
        assert_eq!(typed_day_month("15/março"), None);
    }
}
