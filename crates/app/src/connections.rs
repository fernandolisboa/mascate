//! Settings › Conexões (#5): each Connection with its state, where the owner
//! pastes its keys or removes them. The keys go to the system secret store
//! through the integrations module; this screen never keeps them. Each
//! Connection also opens its own guide (#65): how to get the keys on the
//! Platform, step by step, ending in the same fields.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, Global, Hsla, SharedString, Window, div, px};
use mascate_integrations::{Connection, ConnectionState, Connections, Credential};
use mascate_platform::Build;

use crate::appearance::look;
use crate::credential_guides::guide;
use crate::kit;

/// The app's Connections, shared by every screen that needs them.
pub struct AppConnections(pub Arc<Connections>);

impl Global for AppConnections {}

pub(crate) fn connection_name(connection: Connection) -> &'static str {
    match connection {
        Connection::MercadoLivre => "Mercado Livre",
        Connection::ShopeeAffiliates => "Shopee Afiliados",
        Connection::Anthropic => "Anthropic",
    }
}

pub(crate) fn connection_purpose(connection: Connection) -> &'static str {
    match connection {
        Connection::MercadoLivre => "Conta de vendedor: anúncios, pedidos e estoque.",
        Connection::ShopeeAffiliates => {
            "Open API de afiliados: busca de ofertas. Sem credenciais, fica aguardando \
             aprovação e a descoberta usa o cadastro manual de ofertas."
        }
        Connection::Anthropic => "Títulos e descrições de anúncio gerados como rascunho.",
    }
}

fn credential_label(credential: Credential) -> &'static str {
    match credential.name {
        "ML_CLIENT_ID" => "Client ID",
        "ML_CLIENT_SECRET" => "Client Secret",
        "SHOPEE_AFFILIATE_APP_ID" => "AppID",
        "SHOPEE_AFFILIATE_SECRET" => "Secret",
        "ANTHROPIC_API_KEY" => "Chave da API",
        other => other,
    }
}

/// Identifiers are not secret; everything else is masked while typed.
fn is_identifier(credential: Credential) -> bool {
    matches!(credential.name, "ML_CLIENT_ID" | "SHOPEE_AFFILIATE_APP_ID")
}

/// What the screen knows about one Connection, read off the UI thread
/// because the system store may ask to be unlocked.
#[derive(Clone)]
struct Snapshot {
    state: ConnectionState,
    /// Per pasted credential, in order: whether it is stored.
    stored: Vec<bool>,
}

fn snapshot(connections: &Connections, connection: Connection) -> Snapshot {
    Snapshot {
        state: connections.state(connection),
        stored: connection
            .pasted_credentials()
            .map(|credential| connections.has_credential(credential.name).unwrap_or(false))
            .collect(),
    }
}

struct Card {
    connection: Connection,
    inputs: Vec<(Credential, Entity<InputState>)>,
    /// `None` until the first read finishes.
    snapshot: Option<Snapshot>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    busy: bool,
    confirming_removal: bool,
    error: Option<SharedString>,
}

pub struct ConnectionsSection {
    connections: Arc<Connections>,
    cards: Vec<Card>,
    /// The card whose guide is open in place of the list.
    guide: Option<usize>,
}

impl ConnectionsSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let connections = cx.global::<AppConnections>().0.clone();
        let cards = Connection::ALL
            .into_iter()
            .map(|connection| Card {
                connection,
                inputs: connection
                    .pasted_credentials()
                    .map(|credential| {
                        let input = cx.new(|cx| {
                            InputState::new(window, cx).masked(!is_identifier(credential))
                        });
                        (credential, input)
                    })
                    .collect(),
                snapshot: None,
                reads: 0,
                busy: false,
                confirming_removal: false,
                error: None,
            })
            .collect();
        let mut section = Self {
            connections,
            cards,
            guide: None,
        };
        for index in 0..section.cards.len() {
            section.refresh(index, window, cx);
        }
        section
    }

    /// The Connection whose guide is open, if any.
    pub fn open_guide(&self) -> Option<Connection> {
        self.guide.map(|index| self.cards[index].connection)
    }

    pub fn close_guide(&mut self, cx: &mut Context<Self>) {
        self.guide = None;
        cx.notify();
    }

    fn refresh(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.run(index, window, cx, |_, _| Ok(()));
    }

    /// Shows what was read; each field's hint says whether its key is saved.
    /// After a successful write the fields are emptied: the keys now live in
    /// the store only.
    fn show(
        &mut self,
        index: usize,
        snapshot: Snapshot,
        clear: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let card = &mut self.cards[index];
        for ((_, input), &stored) in card.inputs.iter().zip(&snapshot.stored) {
            let placeholder = if stored {
                "Salva no cofre. Cole outra para trocar."
            } else {
                "Cole aqui"
            };
            input.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx);
                if clear {
                    input.set_value("", window, cx);
                }
            });
        }
        card.snapshot = Some(snapshot);
        cx.notify();
    }

    fn save(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let card = &self.cards[index];
        let values: Vec<(&'static str, String)> = card
            .inputs
            .iter()
            .map(|(credential, input)| (credential.name, input.read(cx).value().to_string()))
            .collect();
        if values.iter().all(|(_, value)| value.trim().is_empty()) {
            self.cards[index].error = Some("Cole ao menos uma chave antes de salvar.".into());
            cx.notify();
            return;
        }
        self.run(index, window, cx, move |connections, connection| {
            let values: Vec<(&str, &str)> = values
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect();
            connections
                .save(connection, &values)
                .map_err(|error| format!("Não consegui salvar no cofre do sistema: {error}"))
        });
    }

    fn remove(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.run(index, window, cx, move |connections, connection| {
            connections
                .remove(connection)
                .map_err(|error| format!("Não consegui remover do cofre do sistema: {error}"))
        });
    }

    /// Runs a write off the UI thread, then reads the Connection again.
    /// Reading alone is a write that does nothing.
    fn run(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
        write: impl FnOnce(&Connections, Connection) -> Result<(), String> + Send + 'static,
    ) {
        let card = &mut self.cards[index];
        let connection = card.connection;
        card.reads += 1;
        let read = card.reads;
        card.busy = true;
        card.confirming_removal = false;
        card.error = None;
        let connections = self.connections.clone();
        let writing = cx.background_executor().spawn(async move {
            let outcome = write(&connections, connection);
            (outcome, snapshot(&connections, connection))
        });
        cx.spawn_in(window, async move |this, cx| {
            let (outcome, snapshot) = writing.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let card = &mut this.cards[index];
                if card.reads != read {
                    return;
                }
                card.busy = false;
                let written = outcome.is_ok();
                card.error = outcome.err().map(SharedString::from);
                this.show(index, snapshot, written, window, cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// The fields where the Connection's keys are pasted.
    fn fields(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let card = &self.cards[index];
        let fields = card.inputs.iter().map(|(credential, input)| {
            let mut field = Input::new(input).small();
            if !is_identifier(*credential) {
                field = field.mask_toggle();
            }
            v_flex()
                .gap_1()
                .min_w(gpui_kit::px(220.))
                .flex_1()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .child(credential_label(*credential)),
                        )
                        .child(div().text_xs().text_color(t.text2).child(credential.name)),
                )
                .child(field.disabled(card.busy))
        });
        h_flex()
            .flex_wrap()
            .gap_3()
            .children(fields)
            .into_any_element()
    }

    /// What went wrong reading the Connection or with the last write.
    fn problems(&self, index: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let card = &self.cards[index];
        let failed = match card.snapshot.as_ref().map(|s| &s.state) {
            Some(ConnectionState::Failed { reason }) => Some(SharedString::from(reason.clone())),
            _ => None,
        };
        failed
            .into_iter()
            .chain(card.error.clone())
            .map(|text| kit::error_notice(text, cx).into_any_element())
            .collect()
    }

    /// Save, and remove once something is stored, with its confirmation.
    fn actions(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let card = &self.cards[index];
        let stored_any = card
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.stored.iter().any(|&stored| stored));
        h_flex()
            .gap_2()
            .child(
                Button::new(("save", index))
                    .label("Salvar")
                    .primary()
                    .small()
                    .loading(card.busy)
                    .disabled(card.busy)
                    .on_click(cx.listener(move |this, _, window, cx| this.save(index, window, cx))),
            )
            .when(stored_any && !card.confirming_removal, |row| {
                row.child(
                    Button::new(("remove", index))
                        .label("Remover")
                        .outline()
                        .small()
                        .disabled(card.busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.cards[index].confirming_removal = true;
                            cx.notify();
                        })),
                )
            })
            .when(card.confirming_removal, |row| {
                row.child(
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child("Apagar estas chaves do cofre?"),
                )
                .child(
                    Button::new(("confirm-remove", index))
                        .label("Remover")
                        .danger()
                        .small()
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.remove(index, window, cx)),
                        ),
                )
                .child(
                    Button::new(("cancel-remove", index))
                        .label("Cancelar")
                        .ghost()
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.cards[index].confirming_removal = false;
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    /// The Connection's name, what it is for and its state.
    fn heading(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let card = &self.cards[index];
        let connection = card.connection;
        h_flex()
            .gap_3()
            .justify_between()
            .items_start()
            .child(
                v_flex()
                    .min_w_0()
                    .child(div().font_medium().child(connection_name(connection)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text2)
                            .child(connection_purpose(connection)),
                    ),
            )
            .child(state_tag(card.snapshot.as_ref().map(|s| &s.state), cx))
            .into_any_element()
    }

    fn render_card(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let card = &self.cards[index];
        let needs_login = card.connection == Connection::MercadoLivre
            && card.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.state == ConnectionState::NotConfigured
                    && snapshot.stored.iter().all(|&stored| stored)
            });

        v_flex()
            .gap_3()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(self.heading(index, cx))
            .child(
                h_flex().child(
                    Button::new(("guide", index))
                        .label("Como conseguir as chaves")
                        .icon(IconName::Info)
                        .ghost()
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.guide = Some(index);
                            cx.notify();
                        })),
                ),
            )
            .child(self.fields(index, cx))
            .when(needs_login, |card| {
                card.child(div().text_xs().text_color(t.text2).child(
                    "Chaves salvas. Falta entrar na sua conta de vendedor, o que chega numa \
                     próxima versão com o botão Conectar.",
                ))
            })
            .children(self.problems(index, cx))
            .child(self.actions(index, cx))
            .into_any_element()
    }

    /// One Connection's guide: numbered steps on the Platform, each with
    /// the page it happens on, then pasting into the same fields as the card.
    fn render_guide(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let connection = self.cards[index].connection;
        let guide = guide(connection);
        let saved = self.cards[index]
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.stored.iter().all(|&stored| stored));
        let number = |n: usize| {
            div()
                .flex_none()
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border(t.border_width)
                .border_color(t.accent)
                .text_xs()
                .font_medium()
                .text_color(t.accent_text)
                .child(n.to_string())
        };
        let steps = guide.steps.iter().enumerate().map(|(at, step)| {
            h_flex()
                .gap_3()
                .items_start()
                .child(number(at + 1))
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .gap_1()
                        .pt(px(2.))
                        .text_sm()
                        .child(step.text)
                        .children(step.link.map(|link| {
                            kit::external_link(
                                SharedString::from(format!("guide-link-{at}")),
                                link.label,
                                link.url,
                                cx,
                            )
                        })),
                )
                .into_any_element()
        });
        let steps: Vec<AnyElement> = steps.collect();
        let paste = h_flex()
            .gap_3()
            .items_start()
            .child(number(guide.steps.len() + 1))
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .gap_3()
                    .pt(px(2.))
                    .child(div().text_sm().child(
                        "Cole nos campos abaixo e clique em Salvar. As chaves vão para o cofre \
                         do sistema.",
                    ))
                    .child(self.fields(index, cx))
                    .children(self.problems(index, cx))
                    .child(self.actions(index, cx)),
            );

        v_flex()
            .gap_4()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(self.heading(index, cx))
            .children(steps)
            .child(paste)
            .when(saved, |page| {
                page.child(kit::success_notice("Tudo salvo no cofre do sistema.", cx))
            })
            .children(guide.then.map(|then| kit::info_notice(then, cx)))
            .into_any_element()
    }
}

/// The Connection's state as a small label in its color.
fn state_tag(state: Option<&ConnectionState>, cx: &App) -> AnyElement {
    let t = look(cx).tokens;
    let (text, ink): (&str, Hsla) = match state {
        None => ("Verificando", t.text2),
        Some(ConnectionState::NotConfigured) => ("Não configurada", t.text2),
        Some(ConnectionState::AwaitingApproval) => ("Aguardando aprovação", t.accent_text),
        Some(ConnectionState::Connected) => ("Conectada", t.success),
        Some(ConnectionState::Expired) => ("Expirada", t.danger),
        Some(ConnectionState::Failed { .. }) => ("Com erro", t.danger),
    };
    kit::tag(text, ink, cx).into_any_element()
}

impl Render for ConnectionsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(index) = self.guide {
            return self.render_guide(index, cx);
        }
        let t = look(cx).tokens;
        let cards: Vec<AnyElement> = (0..self.cards.len())
            .map(|index| self.render_card(index, cx))
            .collect();
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(t.text2)
                    .child("As chaves ficam no cofre do sistema, nunca no banco nem no Backup."),
            )
            .when(Build::CURRENT == Build::Development, |section| {
                section.child(div().text_xs().text_color(t.text2).child(
                    "Build de desenvolvimento: variáveis de ambiente com esses nomes têm \
                     precedência sobre o cofre.",
                ))
            })
            .children(cards)
            .into_any_element()
    }
}
