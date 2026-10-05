//! Listing Copy as the draft and the settings use it (#17): the Claude
//! API's adapter, where it points, the model the owner picks, and how the
//! failures read in Portuguese.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{App, Entity, Global, Window, div, px};
use mascate_integrations::{Anthropic, Connection, ConnectionState};
use mascate_kernel::PlatformError;
use mascate_marketing::{
    CopyError, CopyProblem, CopySettings, DEFAULT_COPY_MODEL, ListingCopy, MODEL_MAX_CHARS,
};
use mascate_platform::{Build, Environment};

use crate::appearance::look;
use crate::connections::AppConnections;
use crate::forms::{Outcome, input, notice};

/// Listing Copy and the Claude API's adapter that writes it.
pub struct AppListingCopy {
    pub copy: Arc<ListingCopy>,
    pub writer: Arc<Anthropic>,
}

impl Global for AppListingCopy {}

pub fn listing_copy(cx: &App) -> Option<(Arc<ListingCopy>, Arc<Anthropic>)> {
    cx.try_global::<AppListingCopy>()
        .map(|app| (app.copy.clone(), app.writer.clone()))
}

/// Where the app reaches the Claude API: Anthropic's or, in a development
/// build only, the URL in `MASCATE_ANTHROPIC_API_URL`, for a local fake
/// server.
pub fn api_url(build: Build, environment: &Environment) -> String {
    crate::mercado_livre::local_fake(build, environment, "MASCATE_ANTHROPIC_API_URL")
        .unwrap_or_else(|| mascate_integrations::ANTHROPIC_API_URL.to_owned())
}

/// Why writing with AI is off while the Anthropic Connection is in
/// `state`; `None` once it is connected.
pub fn writing_off(state: &ConnectionState) -> Option<&'static str> {
    match state {
        ConnectionState::Connected => None,
        ConnectionState::Failed { .. } => Some(
            "Não consegui ler a chave da Anthropic no cofre do sistema; o texto com IA fica \
             desligado. Título e descrição continuam editáveis à mão.",
        ),
        _ => Some(
            "Para gerar título e descrição com IA, cole a chave da API da Anthropic em \
             Configurações › Conexões. Sem ela, escreva à mão.",
        ),
    }
}

/// The Anthropic Connection's state right now.
pub fn anthropic_state(cx: &App) -> ConnectionState {
    cx.try_global::<AppConnections>()
        .map_or(ConnectionState::NotConfigured, |connections| {
            connections.0.state(Connection::Anthropic)
        })
}

/// Why writing failed, as the owner reads it.
pub fn failure(error: &CopyError) -> String {
    match error {
        CopyError::Channel(error) => crate::mercado_livre::failure(error),
        CopyError::Writer(PlatformError::NotConnected) => {
            "Cole a chave da API da Anthropic em Configurações › Conexões para gerar o texto."
                .into()
        }
        CopyError::Writer(PlatformError::Expired) => {
            "A Anthropic não aceitou a chave; confira em Configurações › Conexões.".into()
        }
        CopyError::Writer(PlatformError::RateLimited) => {
            "A Anthropic pediu para esperar e continuou pedindo depois de três tentativas. Gere \
             de novo daqui a alguns minutos."
                .into()
        }
        CopyError::Writer(PlatformError::Refused(why)) => format!(
            "A Anthropic recusou o pedido: {why}. Se for o modelo, confira o nome em \
             Configurações › Títulos e descrições com IA."
        ),
        CopyError::Writer(PlatformError::NotFound) => {
            "A Anthropic não encontrou o que foi pedido; confira o modelo em Configurações › \
             Títulos e descrições com IA."
                .into()
        }
        CopyError::Writer(PlatformError::Failed(why)) => {
            format!("Não consegui falar com a Anthropic: {why}")
        }
        CopyError::Rejected(problems) => format!(
            "O texto gerado veio fora das regras duas vezes ({}), então não entrou no rascunho. \
             Gere de novo ou escreva à mão.",
            problems
                .iter()
                .map(CopyProblem::describe)
                .collect::<Vec<_>>()
                .join("; ")
        ),
        CopyError::InvalidModel => format!(
            "Digite o nome de um modelo da Anthropic, como {DEFAULT_COPY_MODEL}: letras, números \
             e hífens, até {MODEL_MAX_CHARS} caracteres."
        ),
        CopyError::Sql(error) => {
            format!("Não consegui ler o modelo do texto com IA no banco: {error}")
        }
    }
}

/// The model that writes Listing Copy, in Configurações.
pub struct CopySettingsSection {
    model: Entity<InputState>,
    busy: bool,
    outcome: Option<Outcome>,
}

impl CopySettingsSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let section = Self {
            model: input(DEFAULT_COPY_MODEL, window, cx),
            busy: false,
            outcome: None,
        };
        section.load(window, cx);
        section
    }

    fn load(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((copy, _)) = listing_copy(cx) else {
            return;
        };
        let reading = cx
            .background_executor()
            .spawn(async move { copy.settings().await });
        cx.spawn_in(window, async move |this, cx| {
            let read = reading.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match read {
                    Ok(settings) => this
                        .model
                        .update(cx, |input, cx| input.set_value(settings.model, window, cx)),
                    Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some((copy, _)) = listing_copy(cx) else {
            return;
        };
        let settings = CopySettings {
            model: self.model.read(cx).value().to_string(),
        };
        self.busy = true;
        self.outcome = None;
        let saving = cx
            .background_executor()
            .spawn(async move { copy.save_settings(settings).await });
        cx.spawn(async move |this, cx| {
            let saved = saving.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.outcome = Some(match saved {
                    Ok(()) => Outcome::Done(
                        "Salvo: o próximo texto gerado no rascunho usa este modelo.".into(),
                    ),
                    Err(error) => Outcome::Failed(failure(&error).into()),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn use_default(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.update(cx, |input, cx| {
            input.set_value(DEFAULT_COPY_MODEL, window, cx)
        });
        self.save(cx);
    }
}

impl Render for CopySettingsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(div().text_sm().text_color(t.text2).child(format!(
                "No rascunho de anúncio, o botão de IA escreve título e descrição pela API da \
                 Anthropic, cobrada por uso na sua conta. O texto entra só no rascunho, para \
                 você revisar; nada é publicado sem o seu clique. O padrão é {DEFAULT_COPY_MODEL}, \
                 o Claude mais recente com bom custo para texto curto."
            )))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_end()
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(260.))
                            .child(div().text_sm().font_medium().child("Modelo"))
                            .child(Input::new(&self.model).small()),
                    )
                    .child(
                        Button::new("save-copy-model")
                            .label("Salvar")
                            .outline()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    )
                    .child(
                        Button::new("default-copy-model")
                            .label("Voltar ao padrão")
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.use_default(window, cx)),
                            ),
                    ),
            )
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
    }
}
