//! Perguntas (#24): what buyers ask on the owner's Mercado Livre listings,
//! read by a Sync when the app opens and with every Order Sync after it,
//! window open or in the tray. Each new question comes with a system
//! notification; the unanswered ones sit on top, the longest waiting first.
//! A reply template fills the answer, which the owner edits, reviews in its
//! final form and confirms before it goes; the app keeps what it sent. The
//! rules live in Marketing (ADR 0022).

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::TimeDelta;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, Global, Hsla, Subscription, SystemNotification, Task};
use gpui_kit::{Window, div, px};
use mascate_catalog::Catalog;
use mascate_commerce::Listings;
use mascate_integrations::{Connection, ConnectionState};
use mascate_kernel::{PlatformError, RecordId, Timestamp};
use mascate_marketing::{
    ANSWER_MAX_CHARS, AnswerRequest, ChannelQuestion, DEADLINE_VARIABLE, MAX_DISPATCH_DAYS,
    NewReplyTemplate, PRODUCT_VARIABLE, Question, QuestionError, QuestionSettings, QuestionStatus,
    QuestionSync, Questions, ReplyProblem, ReplyTemplate, ReplyTemplates,
};

use crate::appearance::{Tokens, look};
use crate::catalog::{self, NO_DATABASE};
use crate::connections::AppConnections;
use crate::forms::{Outcome, input, notice};
use crate::kit;
use crate::layout;
use crate::listings::{self, listings};
use crate::mercado_livre;
use crate::parts::ScreenParts;

/// The app's questions and reply templates; absent when the database did
/// not open.
pub struct AppQuestions {
    pub questions: Arc<Questions>,
    pub templates: Arc<ReplyTemplates>,
}

impl Global for AppQuestions {}

pub(crate) fn questions(cx: &App) -> Option<Arc<Questions>> {
    cx.try_global::<AppQuestions>()
        .map(|app| app.questions.clone())
}

fn templates(cx: &App) -> Option<Arc<ReplyTemplates>> {
    cx.try_global::<AppQuestions>()
        .map(|app| app.templates.clone())
}

/// The question Syncs since the app opened; the screens that show
/// questions read them again after each.
#[derive(Default)]
pub struct QuestionSyncs {
    pub finished: u64,
}

impl Global for QuestionSyncs {}

/// What one Sync of questions did, with the names its notifications use.
pub struct Round {
    synced: QuestionSync,
    names: BTreeMap<String, String>,
}

/// Why reading, saving or sending failed, as the owner reads it.
pub fn failure(error: &QuestionError) -> String {
    match error {
        QuestionError::Platform(error) => mercado_livre::failure(error),
        QuestionError::Reply(problem) => problem_text(problem),
        QuestionError::NotWaiting => "Esta pergunta já foi respondida ou encerrada no Mercado \
                                      Livre, então nada foi enviado. A lista mostra como ela \
                                      está agora."
            .into(),
        QuestionError::Sending => "A resposta desta pergunta já está sendo enviada.".into(),
        QuestionError::Gone => {
            "O Mercado Livre não tem mais esta pergunta, então nada foi enviado.".into()
        }
        QuestionError::NotFound => "Não encontrei esse modelo ou essa pergunta; atualize a \
                                    tela."
            .into(),
        QuestionError::InvalidSettings => {
            format!("O prazo de despacho vai de 1 a {MAX_DISPATCH_DAYS} dias úteis.")
        }
        error => format!("Não consegui ler ou gravar as perguntas no banco: {error}"),
    }
}

fn problem_text(problem: &ReplyProblem) -> String {
    match problem {
        ReplyProblem::Empty => "Escreva a resposta antes de enviar.".into(),
        ReplyProblem::TooLong { chars } => format!(
            "A resposta tem {chars} caracteres; o Mercado Livre aceita até {ANSWER_MAX_CHARS}."
        ),
        ReplyProblem::Contact(contact) => format!(
            "A resposta tem {}. O Mercado Livre não permite passar contato nem levar o \
             comprador para fora dele; tire esse trecho para enviar.",
            contact.name()
        ),
        ReplyProblem::UnknownVariable(variable) => format!(
            "O modelo usa {variable}, que o app não preenche. Use {PRODUCT_VARIABLE} e \
             {DEADLINE_VARIABLE}."
        ),
        ReplyProblem::BadName => "Dê ao modelo um nome de até 60 caracteres.".into(),
        ReplyProblem::NameTaken(name) => format!("Já existe um modelo chamado “{name}”."),
    }
}

/// The name each listing goes by in answers and on screen, by the channel's
/// id: its Product's name when a variation is linked to one, else its
/// title.
pub(crate) async fn product_names(
    listings: &Listings,
    catalog: &Catalog,
) -> Result<BTreeMap<String, String>, String> {
    Ok(listings::listing_products(listings, catalog)
        .await?
        .into_iter()
        .map(|(id, sold)| (id, sold.name))
        .collect())
}

/// Runs a Sync of questions off the UI thread. `Ok(None)` when the
/// Connection is not up.
pub fn sync_now(cx: &mut App) -> Option<Task<Result<Option<Round>, String>>> {
    let (Some(questions), Some(channel), Some(listings), Some(catalog)) = (
        questions(cx),
        mercado_livre::adapter(cx),
        listings(cx),
        catalog::catalog(cx),
    ) else {
        return None;
    };
    let connections = cx.global::<AppConnections>().0.clone();
    Some(cx.background_executor().spawn(async move {
        if connections.state(Connection::MercadoLivre) != ConnectionState::Connected {
            return Ok(None);
        }
        let synced = questions
            .sync(channel.as_ref())
            .await
            .map_err(|e| failure(&e))?;
        let names = if synced.arrived.is_empty() {
            BTreeMap::new()
        } else {
            product_names(&listings, &catalog).await.unwrap_or_default()
        };
        Ok(Some(Round { synced, names }))
    }))
}

/// Tells the owner about the questions that arrived, and the screens that
/// a Sync ran.
pub fn finished(outcome: &Result<Option<Round>, String>, cx: &mut App) {
    if let Ok(Some(round)) = outcome {
        notify_arrived(&round.synced.arrived, &round.names, cx);
    }
    if let Err(error) = outcome {
        eprintln!("could not sync the questions: {error}");
    }
    cx.default_global::<QuestionSyncs>().finished += 1;
}

/// Above this many new questions in one Sync, as after days closed, one
/// notification sums them up instead of one each.
const NOTIFIED_ONE_BY_ONE: usize = 3;

fn notify_arrived(arrived: &[ChannelQuestion], names: &BTreeMap<String, String>, cx: &App) {
    let name = |question: &ChannelQuestion| {
        names
            .get(&question.listing)
            .cloned()
            .unwrap_or_else(|| question.listing.clone())
    };
    if arrived.len() > NOTIFIED_ONE_BY_ONE {
        let mut listed: Vec<String> = arrived.iter().map(name).collect();
        listed.dedup();
        cx.show_system_notification(SystemNotification {
            tag: "new-questions".into(),
            title: format!("{} perguntas novas no Mercado Livre", arrived.len()).into(),
            body: format!("Em {}.", listed.join(", ")).into(),
            actions: Vec::new(),
        });
        return;
    }
    for question in arrived {
        cx.show_system_notification(SystemNotification {
            tag: format!("question-{}", question.id).into(),
            title: "Pergunta nova no Mercado Livre".into(),
            body: format!("{}: “{}”", name(question), question.text).into(),
            actions: Vec::new(),
        });
    }
}

/// "há 5 min", "há 3 h", "há 2 dias".
pub fn waiting_text(waiting: TimeDelta) -> String {
    format!("há {}", span_text(waiting))
}

/// "5 min", "3 h", "2 dias".
fn span_text(span: TimeDelta) -> String {
    let minutes = span.num_minutes().max(0);
    match minutes {
        0 => "menos de 1 min".into(),
        1..60 => format!("{minutes} min"),
        60..1440 => format!("{} h", minutes / 60),
        1440..2880 => "1 dia".into(),
        _ => format!("{} dias", minutes / 1440),
    }
}

/// The ink of a waiting time: the longer, the louder.
pub fn waiting_ink(waiting: TimeDelta, t: &Tokens) -> Hsla {
    if waiting >= TimeDelta::hours(12) {
        t.danger
    } else {
        t.accent_text
    }
}

fn status_tag(status: QuestionStatus, t: &Tokens) -> (&'static str, Hsla) {
    match status {
        QuestionStatus::Unanswered => ("Sem resposta", t.accent_text),
        QuestionStatus::Answered => ("Respondida", t.success),
        QuestionStatus::ClosedUnanswered => ("Encerrada sem resposta", t.text2),
        QuestionStatus::UnderReview => ("Em análise no Mercado Livre", t.text2),
        QuestionStatus::Banned => ("Removida pelo Mercado Livre", t.danger),
        QuestionStatus::Deleted => ("Apagada", t.text2),
    }
}

/// Everything the screen shows, read in one go off the UI thread.
struct Snapshot {
    inbox: Vec<Question>,
    names: BTreeMap<String, String>,
    templates: Vec<ReplyTemplate>,
    settings: QuestionSettings,
    last_sync: Option<Timestamp>,
    connection: ConnectionState,
    now: Timestamp,
}

/// The answer being written to one question.
struct Answering {
    question: RecordId,
    text: Entity<TextareaState>,
    /// The final text, checked, waiting for the owner's confirmation.
    review: Option<AnswerRequest>,
    problem: Option<String>,
    sending: bool,
}

/// The template being written or changed.
struct TemplateForm {
    editing: Option<RecordId>,
    name: Entity<InputState>,
    text: Entity<TextareaState>,
}

pub struct QuestionsScreen {
    inbox: Vec<Question>,
    names: BTreeMap<String, String>,
    templates: Vec<ReplyTemplate>,
    settings: QuestionSettings,
    last_sync: Option<Timestamp>,
    connection: Option<ConnectionState>,
    now: Option<Timestamp>,
    syncing: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
    answering: Option<Answering>,
    form: TemplateForm,
    dispatch_days: Entity<InputState>,
    saving: bool,
    template_outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl QuestionsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe_global::<QuestionSyncs>(Self::refresh)];
        let mut screen = Self {
            inbox: Vec::new(),
            names: BTreeMap::new(),
            templates: Vec::new(),
            settings: QuestionSettings::default(),
            last_sync: None,
            connection: None,
            now: None,
            syncing: false,
            reads: 0,
            outcome: None,
            answering: None,
            form: TemplateForm {
                editing: None,
                name: input("Ex.: Prazo de envio", window, cx),
                text: template_area(window, cx),
            },
            dispatch_days: input("1", window, cx),
            saving: false,
            template_outcome: None,
            _subscriptions: subscriptions,
        };
        screen.read(cx);
        screen.load_dispatch_days(window, cx);
        screen
    }

    /// Reads the questions again, as when the screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.read(cx);
    }

    /// Reads everything the screen shows off the UI thread.
    fn read(&mut self, cx: &mut Context<Self>) {
        let (Some(questions), Some(templates), Some(listings), Some(catalog)) = (
            questions(cx),
            templates(cx),
            listings(cx),
            catalog::catalog(cx),
        ) else {
            return;
        };
        let connections = cx.global::<AppConnections>().0.clone();
        self.reads += 1;
        let read = self.reads;
        let reading = cx.background_executor().spawn(async move {
            Ok::<_, String>(Snapshot {
                inbox: questions.inbox().await.map_err(|e| failure(&e))?,
                names: product_names(&listings, &catalog).await?,
                templates: templates.templates().await.map_err(|e| failure(&e))?,
                settings: templates.settings().await.map_err(|e| failure(&e))?,
                last_sync: questions.last_sync().await.map_err(|e| failure(&e))?,
                connection: connections.state(Connection::MercadoLivre),
                now: questions.now(),
            })
        });
        cx.spawn(async move |this, cx| {
            let read_back = reading.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                this.syncing = false;
                match read_back {
                    Ok(snapshot) => {
                        this.inbox = snapshot.inbox;
                        this.names = snapshot.names;
                        this.templates = snapshot.templates;
                        this.settings = snapshot.settings;
                        this.last_sync = snapshot.last_sync;
                        this.connection = Some(snapshot.connection);
                        this.now = Some(snapshot.now);
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_dispatch_days(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(templates) = templates(cx) else {
            return;
        };
        let reading = cx
            .background_executor()
            .spawn(async move { templates.settings().await.unwrap_or_default() });
        cx.spawn_in(window, async move |this, cx| {
            let settings = reading.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.dispatch_days.update(cx, |field, cx| {
                    field.set_value(settings.dispatch_days.to_string(), window, cx)
                });
            });
        })
        .detach();
    }

    /// Reads the questions from Mercado Livre now.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(round) = sync_now(cx) else {
            return;
        };
        self.syncing = true;
        self.outcome = None;
        cx.spawn(async move |this, cx| {
            let outcome = round.await;
            let _ = this.update(cx, |this, cx| {
                this.syncing = false;
                this.outcome = Some(match &outcome {
                    Ok(Some(round)) => Outcome::Done(synced_text(&round.synced).into()),
                    Ok(None) => {
                        Outcome::Failed(mercado_livre::failure(&PlatformError::NotConnected).into())
                    }
                    Err(error) => Outcome::Failed(error.clone().into()),
                });
                finished(&outcome, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn connected(&self) -> bool {
        self.connection == Some(ConnectionState::Connected)
    }

    fn question(&self, id: RecordId) -> Option<&Question> {
        self.inbox.iter().find(|question| question.id == id)
    }

    fn name_of(&self, question: &Question) -> String {
        self.names
            .get(&question.asked.listing)
            .cloned()
            .unwrap_or_else(|| question.asked.listing.clone())
    }

    /// Opens the answer to `question`, empty or filled by the only template.
    fn start_answer(&mut self, question: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let text = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 10)
                .placeholder("Escreva a resposta ou escolha um modelo acima.")
        });
        self.answering = Some(Answering {
            question,
            text,
            review: None,
            problem: None,
            sending: false,
        });
        if let [only] = self.templates.as_slice() {
            let only = only.clone();
            self.fill_with(&only, window, cx);
        }
        cx.notify();
    }

    fn fill_with(&mut self, template: &ReplyTemplate, window: &mut Window, cx: &mut Context<Self>) {
        let Some(answering) = &self.answering else {
            return;
        };
        let Some(question) = self.question(answering.question) else {
            return;
        };
        let filled = template.fill(&self.name_of(question), &self.settings);
        let answering = self.answering.as_mut().expect("answering");
        answering.review = None;
        answering.problem = None;
        answering
            .text
            .update(cx, |field, cx| field.set_value(filled, window, cx));
        cx.notify();
    }

    /// Checks the answer and shows its final text for the owner to confirm.
    fn review(&mut self, cx: &mut Context<Self>) {
        let (Some(questions), Some(answering)) = (questions(cx), &self.answering) else {
            return;
        };
        let Some(question) = self.question(answering.question).cloned() else {
            return;
        };
        let text = answering.text.read(cx).value().to_string();
        let answering = self.answering.as_mut().expect("answering");
        match questions.prepare(&question, &text) {
            Ok(request) => {
                answering.review = Some(request);
                answering.problem = None;
            }
            Err(error) => {
                answering.review = None;
                answering.problem = Some(failure(&error));
            }
        }
        cx.notify();
    }

    /// Sends the reviewed answer: the owner's confirmation.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        let (Some(questions), Some(channel)) = (questions(cx), mercado_livre::adapter(cx)) else {
            return;
        };
        let Some(answering) = self.answering.as_mut() else {
            return;
        };
        let Some(request) = answering.review.clone() else {
            return;
        };
        answering.sending = true;
        let confirmed = request.confirm();
        let sending = cx.background_executor().spawn(async move {
            questions
                .send(channel.as_ref(), confirmed)
                .await
                .map_err(|e| failure(&e))
        });
        cx.spawn(async move |this, cx| {
            let sent = sending.await;
            let _ = this.update(cx, |this, cx| {
                match sent {
                    Ok(_) => {
                        this.answering = None;
                        this.outcome = Some(Outcome::Done(
                            "Resposta enviada ao Mercado Livre e guardada com a pergunta.".into(),
                        ));
                    }
                    Err(error) => {
                        if let Some(answering) = this.answering.as_mut() {
                            answering.sending = false;
                            answering.review = None;
                            answering.problem = Some(error);
                        }
                    }
                }
                this.read(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn edit_template(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(template) = self.templates.iter().find(|t| t.id == id).cloned() else {
            return;
        };
        self.form.editing = Some(id);
        self.form
            .name
            .update(cx, |field, cx| field.set_value(template.name, window, cx));
        self.form
            .text
            .update(cx, |field, cx| field.set_value(template.text, window, cx));
        self.template_outcome = None;
        cx.notify();
    }

    fn clear_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form.editing = None;
        self.form
            .name
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.form
            .text
            .update(cx, |field, cx| field.set_value("", window, cx));
    }

    fn save_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(templates) = templates(cx) else {
            return;
        };
        let new = NewReplyTemplate {
            name: self.form.name.read(cx).value().to_string(),
            text: self.form.text.read(cx).value().to_string(),
        };
        let editing = self.form.editing;
        self.saving = true;
        self.template_outcome = None;
        let saving = cx.background_executor().spawn(async move {
            match editing {
                Some(id) => templates.update(id, new).await,
                None => templates.create(new).await,
            }
            .map_err(|e| failure(&e))
        });
        cx.spawn_in(window, async move |this, cx| {
            let saved = saving.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.saving = false;
                this.template_outcome = Some(match saved {
                    Ok(template) => {
                        this.clear_form(window, cx);
                        Outcome::Done(format!("Modelo “{}” salvo.", template.name).into())
                    }
                    Err(error) => Outcome::Failed(error.into()),
                });
                this.read(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn delete_template(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(templates) = templates(cx) else {
            return;
        };
        if self.form.editing == Some(id) {
            self.clear_form(window, cx);
        }
        self.saving = true;
        let deleting = cx
            .background_executor()
            .spawn(async move { templates.delete(id).await.map_err(|e| failure(&e)) });
        cx.spawn(async move |this, cx| {
            let deleted = deleting.await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                this.template_outcome = Some(match deleted {
                    Ok(()) => Outcome::Done("Modelo apagado.".into()),
                    Err(error) => Outcome::Failed(error.into()),
                });
                this.read(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn save_dispatch_days(&mut self, cx: &mut Context<Self>) {
        let Some(templates) = templates(cx) else {
            return;
        };
        let Ok(days) = self.dispatch_days.read(cx).value().trim().parse::<u8>() else {
            self.template_outcome = Some(Outcome::Failed(
                format!("Digite o prazo de despacho em dias úteis, de 1 a {MAX_DISPATCH_DAYS}.")
                    .into(),
            ));
            cx.notify();
            return;
        };
        let settings = QuestionSettings {
            dispatch_days: days,
        };
        self.saving = true;
        let saving = cx.background_executor().spawn(async move {
            templates
                .save_settings(settings)
                .await
                .map_err(|e| failure(&e))
        });
        cx.spawn(async move |this, cx| {
            let saved = saving.await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                this.template_outcome = Some(match saved {
                    Ok(()) => Outcome::Done(
                        format!("Salvo: {DEADLINE_VARIABLE} vira “{}”.", settings.deadline())
                            .into(),
                    ),
                    Err(error) => Outcome::Failed(error.into()),
                });
                this.read(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn render_sync(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let when = match self.last_sync {
            Some(at) => format!("Lidas em {}", catalog::day_and_time(at)),
            None => "Perguntas ainda não lidas.".into(),
        };
        h_flex()
            .gap_2()
            .items_center()
            .child(div().text_xs().text_color(t.text2).child(when))
            .child(
                Button::new("sync-questions")
                    .label("Atualizar")
                    .icon(IconName::RefreshCw)
                    .primary()
                    .small()
                    .loading(self.syncing)
                    .disabled(self.syncing || !self.connected())
                    .on_click(cx.listener(|this, _, _, cx| this.sync(cx))),
            )
            .into_any_element()
    }

    fn render_question(
        &self,
        index: usize,
        question: &Question,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let id = question.id;
        let (status, ink) = status_tag(question.asked.status, &t);
        let mut facts = vec![
            question.asked.listing.clone(),
            format!(
                "perguntada em {}",
                catalog::day_and_time(question.asked.asked_at)
            ),
        ];
        let waiting = self.now.and_then(|now| question.waiting(now));
        if let Some(waited) = question.waited() {
            facts.push(format!("respondida depois de {}", span_text(waited)));
        }
        let text = if question.asked.text.trim().is_empty() {
            "(texto removido pelo Mercado Livre)".to_owned()
        } else {
            question.asked.text.clone()
        };
        let answering = self
            .answering
            .as_ref()
            .filter(|answering| answering.question == id);
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .font_medium()
                            .truncate()
                            .child(self.name_of(question)),
                    )
                    .children(waiting.map(|waiting| {
                        kit::tag(
                            format!("Espera {}", waiting_text(waiting)),
                            waiting_ink(waiting, &t),
                            cx,
                        )
                    }))
                    .child(kit::tag(status, ink, cx)),
            )
            .child(div().text_xs().text_color(t.text2).child(facts.join(" · ")))
            .child(div().text_sm().child(format!("“{text}”")))
            .children(question.asked.answer.as_ref().map(|answer| {
                let by = if question.sent.is_some() {
                    "Sua resposta, enviada pelo app"
                } else {
                    "Resposta no Mercado Livre"
                };
                v_flex()
                    .gap_0p5()
                    .pl_3()
                    .border_l_2()
                    .border_color(t.border)
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text2)
                            .child(format!("{by} em {}", catalog::day_and_time(answer.at))),
                    )
                    .child(div().text_sm().child(answer.text.clone()))
            }))
            .when(question.is_unanswered() && answering.is_none(), |card| {
                card.child(
                    h_flex().child(
                        Button::new(("answer", index))
                            .label("Responder")
                            .icon(IconName::Plus)
                            .outline()
                            .small()
                            .disabled(!self.connected())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start_answer(id, window, cx)
                            })),
                    ),
                )
            })
            .children(answering.map(|answering| self.render_answer(answering, cx)))
            .into_any_element()
    }

    fn render_answer(&self, answering: &Answering, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let length = answering.text.read(cx).value().chars().count();
        let mut editor = v_flex().gap_2().pt_2().border_t_1().border_color(t.border);
        if let Some(review) = &answering.review {
            return editor
                .child(div().text_sm().font_medium().child(
                    "Confira a resposta como ela vai para o comprador. Ela só sai quando você \
                     confirmar, e fica pública no anúncio.",
                ))
                .child(
                    div()
                        .p_3()
                        .rounded(t.radius)
                        .bg(t.raised)
                        .text_sm()
                        .child(review.text().to_owned()),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("confirm-answer")
                                .label("Confirmar e enviar")
                                .icon(IconName::Check)
                                .primary()
                                .small()
                                .loading(answering.sending)
                                .disabled(answering.sending || !self.connected())
                                .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                        )
                        .child(
                            Button::new("edit-answer")
                                .label("Voltar e editar")
                                .ghost()
                                .small()
                                .disabled(answering.sending)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(answering) = this.answering.as_mut() {
                                        answering.review = None;
                                    }
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element();
        }
        if !self.templates.is_empty() {
            editor = editor.child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(div().text_xs().text_color(t.text2).child("Modelos:"))
                    .children(self.templates.iter().enumerate().map(|(at, template)| {
                        let template = template.clone();
                        Button::new(("use-template", at))
                            .label(template.name.clone())
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.fill_with(&template, window, cx)
                            }))
                    })),
            );
        }
        editor
            .child(Textarea::new(&answering.text).small())
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(if length > ANSWER_MAX_CHARS {
                                t.danger
                            } else {
                                t.text2
                            })
                            .child(format!("{length}/{ANSWER_MAX_CHARS}")),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("review-answer")
                            .label("Revisar resposta")
                            .primary()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.review(cx))),
                    )
                    .child(
                        Button::new("cancel-answer")
                            .label("Cancelar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.answering = None;
                                cx.notify();
                            })),
                    ),
            )
            .children(
                answering
                    .problem
                    .clone()
                    .map(|problem| kit::error_notice(problem, cx)),
            )
            .into_any_element()
    }

    fn render_templates(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let rows = self.templates.iter().enumerate().map(|(at, template)| {
            let id = template.id;
            h_flex()
                .gap_3()
                .items_start()
                .p_3()
                .rounded(t.radius_lg)
                .border(t.border_width)
                .border_color(t.frame)
                .bg(t.surface)
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .gap_0p5()
                        .child(div().font_medium().child(template.name.clone()))
                        .child(
                            div()
                                .text_sm()
                                .text_color(t.text2)
                                .child(template.text.clone()),
                        ),
                )
                .child(
                    Button::new(("edit-template", at))
                        .label("Editar")
                        .ghost()
                        .small()
                        .disabled(self.saving)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.edit_template(id, window, cx)
                        })),
                )
                .child(
                    Button::new(("delete-template", at))
                        .label("Apagar")
                        .ghost()
                        .small()
                        .disabled(self.saving)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.delete_template(id, window, cx)
                        })),
                )
        });
        let editing = self.form.editing.is_some();
        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(kit::section_heading("Modelos de resposta"))
                    .child(div().text_xs().text_color(t.text2).child(format!(
                        "{PRODUCT_VARIABLE} vira o nome do produto e {DEADLINE_VARIABLE} o seu \
                         prazo de despacho. Um modelo preenche a resposta, que você ainda edita \
                         e confirma antes de enviar."
                    ))),
            )
            .children(rows)
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded(t.radius_lg)
                    .border(t.border_width)
                    .border_color(t.frame)
                    .bg(t.surface)
                    .child(div().text_sm().font_medium().child(if editing {
                        "Editar modelo"
                    } else {
                        "Novo modelo"
                    }))
                    .child(Input::new(&self.form.name).small())
                    .child(Textarea::new(&self.form.text).small())
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("save-template")
                                    .label("Salvar modelo")
                                    .outline()
                                    .small()
                                    .loading(self.saving)
                                    .disabled(self.saving)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_template(window, cx)
                                    })),
                            )
                            .when(editing, |row| {
                                row.child(
                                    Button::new("new-template")
                                        .label("Cancelar edição")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.clear_form(window, cx);
                                            cx.notify();
                                        })),
                                )
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_end()
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(260.))
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .child("Prazo de despacho (dias úteis)"),
                            )
                            .child(Input::new(&self.dispatch_days).small()),
                    )
                    .child(
                        Button::new("save-dispatch-days")
                            .label("Salvar")
                            .outline()
                            .small()
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, _, cx| this.save_dispatch_days(cx))),
                    ),
            )
            .children(
                self.template_outcome
                    .as_ref()
                    .map(|outcome| notice(outcome, cx)),
            )
            .into_any_element()
    }
}

impl Render for QuestionsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let mut parts = ScreenParts::new("Perguntas");
        if questions(cx).is_none() || listings(cx).is_none() {
            parts
                .notices
                .push(kit::error_notice(NO_DATABASE, cx).into_any_element());
            return layout::screen(parts, cx);
        }
        parts.actions.push(self.render_sync(cx));
        if let Some(state) = &self.connection
            && *state != ConnectionState::Connected
        {
            parts
                .notices
                .push(kit::info_notice(mercado_livre::not_connected(state), cx).into_any_element());
        }
        parts
            .notices
            .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        let inbox = self.inbox.clone();
        let (waiting, settled): (Vec<_>, Vec<_>) = inbox
            .iter()
            .enumerate()
            .partition(|(_, question)| question.is_unanswered());
        let heading = match waiting.len() {
            0 => "Nenhuma pergunta esperando resposta".to_owned(),
            1 => "1 pergunta esperando resposta".to_owned(),
            count => format!("{count} perguntas esperando resposta"),
        };
        parts.content.push(
            v_flex()
                .gap_3()
                .child(v_flex().gap_1().child(kit::section_heading(heading)).child(
                    div().text_xs().text_color(t.text2).child(
                        "A que espera há mais tempo fica em cima. As perguntas novas chegam a \
                         cada Sync de pedidos e viram notificação. Nada é respondido sem a sua \
                         confirmação.",
                    ),
                ))
                .children(
                    waiting
                        .iter()
                        .map(|(index, question)| self.render_question(*index, question, cx)),
                )
                .into_any_element(),
        );
        if !settled.is_empty() {
            parts.content.push(
                v_flex()
                    .gap_3()
                    .child(kit::section_heading("Respondidas e encerradas"))
                    .children(
                        settled
                            .iter()
                            .map(|(index, question)| self.render_question(*index, question, cx)),
                    )
                    .into_any_element(),
            );
        }
        parts.content.push(self.render_templates(cx));
        layout::screen(parts, cx)
    }
}

/// "2 perguntas novas, 3 esperando resposta."
fn synced_text(synced: &QuestionSync) -> String {
    let arrived = match synced.arrived.len() {
        0 => "Nenhuma pergunta nova".to_owned(),
        1 => "1 pergunta nova".to_owned(),
        count => format!("{count} perguntas novas"),
    };
    let waiting = match synced.unanswered {
        0 => "nenhuma esperando resposta".to_owned(),
        1 => "1 esperando resposta".to_owned(),
        count => format!("{count} esperando resposta"),
    };
    format!("{arrived}, {waiting}.")
}

fn template_area<V: 'static>(window: &mut Window, cx: &mut Context<V>) -> Entity<TextareaState> {
    cx.new(|cx| {
        TextareaState::new(window, cx).auto_grow(3, 10).placeholder(
            "Ex.: Olá! O {produto} sai em até {prazo} depois da aprovação do pagamento.",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_times_read_in_portuguese() {
        for (minutes, text) in [
            (0, "há menos de 1 min"),
            (5, "há 5 min"),
            (59, "há 59 min"),
            (60, "há 1 h"),
            (23 * 60 + 59, "há 23 h"),
            (24 * 60, "há 1 dia"),
            (3 * 24 * 60, "há 3 dias"),
        ] {
            assert_eq!(waiting_text(TimeDelta::minutes(minutes)), text);
        }
    }

    #[test]
    fn a_sync_reads_in_portuguese() {
        assert_eq!(
            synced_text(&QuestionSync::default()),
            "Nenhuma pergunta nova, nenhuma esperando resposta."
        );
        let one = ChannelQuestion {
            id: "1".into(),
            listing: "MLB1".into(),
            text: "Tem?".into(),
            status: QuestionStatus::Unanswered,
            asked_at: Timestamp::default(),
            answer: None,
        };
        assert_eq!(
            synced_text(&QuestionSync {
                arrived: vec![one.clone(), one],
                unanswered: 3,
            }),
            "2 perguntas novas, 3 esperando resposta."
        );
    }
}
