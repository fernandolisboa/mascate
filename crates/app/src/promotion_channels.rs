//! Divulgação (#99): the owner's Promotion Channels, each with the marking
//! its affiliate links carry, the keywords of a paid-search channel and
//! the chat of a Telegram channel. The channels live in the Marketing
//! (ADR 0031, decision 5); Afiliação tags links with them.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, Global, Window, div, px};
use mascate_kernel::RecordId;
use mascate_marketing::{
    CHANNEL_NAME_MAX_CHARS, ChannelError, ChannelKind, NewPromotionChannel, PromotionChannel,
    PromotionChannels, suggested_tag,
};

use crate::appearance::look;
use crate::catalog::NO_DATABASE;
use crate::forms::{Outcome, input, notice};
use crate::kit;
use crate::layout;
use crate::parts::ScreenParts;
use crate::sales::table_row;

/// The app's Promotion Channels; absent when the database did not open.
pub struct AppPromotionChannels(pub Arc<PromotionChannels>);

impl Global for AppPromotionChannels {}

pub(crate) fn promotion_channels(cx: &App) -> Option<Arc<PromotionChannels>> {
    cx.try_global::<AppPromotionChannels>()
        .map(|app| app.0.clone())
}

/// Counts the changes to the channels, so what lists them reads them again.
#[derive(Default)]
pub struct ChannelChanges(pub u64);

impl Global for ChannelChanges {}

fn failure(error: &ChannelError) -> String {
    match error {
        ChannelError::BadName => {
            format!("Dê um nome ao canal, com até {CHANNEL_NAME_MAX_CHARS} caracteres.")
        }
        ChannelError::BadTag => "A marcação tem letras minúsculas, números e _, até 20 \
                                 caracteres, como telegram ou grupo_vip."
            .into(),
        ChannelError::TagTaken(tag) => {
            format!("Outro canal já usa a marcação {tag}: cada canal tem a sua.")
        }
        ChannelError::BadChat => "O chat do Telegram é o @nome público do canal (como \
                                  @ofertasdofernando) ou o id que começa com -100."
            .into(),
        ChannelError::NotFound => "Esse canal mudou; a lista foi atualizada.".into(),
        ChannelError::Unreadable(_) | ChannelError::Sql(_) => {
            format!("Não consegui ler ou gravar os canais no banco: {error}")
        }
    }
}

/// What a kind of channel needs, under the form.
fn kind_note(kind: ChannelKind) -> &'static str {
    match kind {
        ChannelKind::TelegramChannel => {
            "Publicar no canal pelo app chega numa próxima versão; o chat já fica guardado."
        }
        ChannelKind::PaidSearch => {
            "As palavras-chave entram na conferência das regras de cada programa: a Shopee não \
             aceita link em busca paga, e marcas como amazon, shopee ou hotmart ficam \
             bloqueadas."
        }
        ChannelKind::WhatsAppGroup
        | ChannelKind::Instagram
        | ChannelKind::TikTok
        | ChannelKind::Other => "Um canal ou grupo seu, onde você posta de graça.",
    }
}

pub struct PromotionScreen {
    channels: Vec<PromotionChannel>,
    name: Entity<InputState>,
    tag: Entity<InputState>,
    /// Separated by commas.
    keywords: Entity<InputState>,
    chat: Entity<InputState>,
    kind: ChannelKind,
    /// The channel the form is editing, if any.
    editing: Option<RecordId>,
    /// The channel asking to confirm its deletion.
    deleting: Option<RecordId>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    form: Option<Outcome>,
    outcome: Option<Outcome>,
}

impl PromotionScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut screen = Self {
            channels: Vec::new(),
            name: input("Ofertas do Fernando", window, cx),
            tag: input("Vazia, sai do nome", window, cx),
            keywords: input("fone bluetooth, fone sem fio", window, cx),
            chat: input("@ofertasdofernando ou -100…", window, cx),
            kind: ChannelKind::TelegramChannel,
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

    /// Reads the channels again, as when the screen comes into view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx, |_| async { Ok(()) }, |_, _, _, _| {});
    }

    /// Runs `change` off the UI thread, then reads the channels again and
    /// hands `done` what `change` gave.
    fn run<T, F, Fut, D>(&mut self, window: &mut Window, cx: &mut Context<Self>, change: F, done: D)
    where
        T: Send + 'static,
        F: FnOnce(Arc<PromotionChannels>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, ChannelError>> + Send,
        D: FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    {
        let Some(channels) = promotion_channels(cx) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(channels.clone()).await;
            (changed, channels.channels().await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (changed, read_back) = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reads == read {
                    this.busy = false;
                    match read_back {
                        Ok(channels) => this.channels = channels,
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

    fn typed(&self, cx: &App) -> NewPromotionChannel {
        let name = self.name.read(cx).value().to_string();
        let tag = self.tag.read(cx).value().trim().to_owned();
        NewPromotionChannel {
            default_tag: if tag.is_empty() {
                suggested_tag(&name)
            } else {
                tag
            },
            name,
            kind: self.kind,
            keywords: self
                .keywords
                .read(cx)
                .value()
                .split(',')
                .map(str::to_owned)
                .collect(),
            telegram_chat: Some(self.chat.read(cx).value().to_string()),
        }
    }

    fn changed(cx: &mut Context<Self>) {
        cx.update_default_global::<ChannelChanges, _>(|changes, _| changes.0 += 1);
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.typed(cx);
        let editing = self.editing;
        self.form = None;
        self.run(
            window,
            cx,
            move |channels| async move {
                match editing {
                    Some(id) => channels.update(id, &typed).await,
                    None => channels.create(&typed).await,
                }
            },
            move |this, saved: PromotionChannel, window, cx| {
                this.outcome = Some(Outcome::Done(
                    if editing.is_some() {
                        format!(
                            "Canal {} salvo. Os links já criados ficam com a marcação que \
                             tinham.",
                            saved.name
                        )
                    } else {
                        format!(
                            "Canal {} criado, com a marcação {}. Crie os links dele em \
                             Afiliação › Links de afiliado.",
                            saved.name, saved.default_tag
                        )
                    }
                    .into(),
                ));
                this.clear(window, cx);
                Self::changed(cx);
            },
        );
    }

    fn edit(&mut self, channel: &PromotionChannel, window: &mut Window, cx: &mut Context<Self>) {
        let fill = |field: &Entity<InputState>, text: String, window: &mut Window, cx: &mut App| {
            field.update(cx, |field, cx| field.set_value(text, window, cx));
        };
        fill(&self.name, channel.name.clone(), window, cx);
        fill(&self.tag, channel.default_tag.clone(), window, cx);
        fill(&self.keywords, channel.keywords.join(", "), window, cx);
        fill(
            &self.chat,
            channel.telegram_chat.clone().unwrap_or_default(),
            window,
            cx,
        );
        self.kind = channel.kind;
        self.editing = Some(channel.id);
        self.deleting = None;
        self.form = None;
        cx.notify();
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for field in [&self.name, &self.tag, &self.keywords, &self.chat] {
            field.update(cx, |field, cx| field.set_value("", window, cx));
        }
        self.kind = ChannelKind::TelegramChannel;
        self.editing = None;
        self.form = None;
        cx.notify();
    }

    fn delete(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.deleting = None;
        let name = self
            .channels
            .iter()
            .find(|channel| channel.id == id)
            .map_or_else(String::new, |channel| channel.name.clone());
        self.run(
            window,
            cx,
            move |channels| async move { channels.delete(id).await },
            move |this, _, window, cx| {
                this.outcome = Some(Outcome::Done(
                    format!(
                        "Canal {name} apagado. As vendas dos links dele continuam com a marcação \
                         que tinham."
                    )
                    .into(),
                ));
                if this.editing == Some(id) {
                    this.clear(window, cx);
                }
                Self::changed(cx);
            },
        );
    }

    fn render_channel(
        &self,
        index: usize,
        channel: &PromotionChannel,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let detail = match channel.kind {
            ChannelKind::TelegramChannel => Some(channel.telegram_chat.clone().map_or_else(
                || "chat ainda não informado".to_owned(),
                |chat| format!("chat {chat}"),
            )),
            ChannelKind::PaidSearch if channel.keywords.is_empty() => {
                Some("nenhuma palavra-chave".to_owned())
            }
            ChannelKind::PaidSearch => {
                Some(format!("palavras-chave: {}", channel.keywords.join(", ")))
            }
            _ => None,
        };
        let edited = channel.clone();
        let id = channel.id;
        let actions = if self.deleting == Some(id) {
            h_flex()
                .gap_2()
                .items_center()
                .child(div().text_xs().child("Apagar o canal?"))
                .child(
                    Button::new(("confirm-delete-channel", index))
                        .label("Apagar")
                        .danger()
                        .small()
                        .disabled(self.busy)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.delete(id, window, cx)),
                        ),
                )
                .child(
                    Button::new(("keep-channel", index))
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
                    Button::new(("edit-channel", index))
                        .label("Editar")
                        .outline()
                        .small()
                        .disabled(self.busy)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.edit(&edited, window, cx)),
                        ),
                )
                .child(
                    Button::new(("delete-channel", index))
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
            .child(kit::tag(channel.kind.name(), t.text2, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(220.))
                    .child(div().font_medium().truncate().child(channel.name.clone()))
                    .children(
                        detail.map(|detail| div().text_xs().text_color(t.text2).child(detail)),
                    ),
            )
            .child(kit::tag(
                format!("marcação {}", channel.default_tag),
                t.accent_text,
                cx,
            ))
            .child(actions)
            .into_any_element()
    }

    fn render_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let kinds = ChannelKind::ALL
            .into_iter()
            .enumerate()
            .map(|(index, kind)| {
                let chip = Button::new(("channel-kind", index))
                    .label(kind.name())
                    .small()
                    .disabled(self.busy);
                if kind == self.kind {
                    chip.primary()
                } else {
                    chip.ghost()
                }
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.kind = kind;
                    cx.notify();
                }))
            });
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .child(div().text_sm().child(label))
                .child(div().w(px(360.)).child(Input::new(input).small()))
        };
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(kit::section_heading(if self.editing.is_some() {
                "Editar canal"
            } else {
                "Novo canal"
            }))
            .child(field("Nome", &self.name))
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child("Tipo"))
                    .child(h_flex().flex_wrap().gap_1().children(kinds)),
            )
            .child(div().text_xs().text_color(t.text2).child(kind_note(self.kind)))
            .child(
                field("Marcação", &self.tag).child(div().text_xs().text_color(t.text2).child(
                    "Letras minúsculas, números e _, até 20. Vai em cada link de afiliado                      deste canal e volta nas vendas, ligando cada comissão ao canal.",
                )),
            )
            .when(self.kind == ChannelKind::PaidSearch, |form| {
                form.child(field("Palavras-chave", &self.keywords).child(
                    div()
                        .text_xs()
                        .text_color(t.text2)
                        .child("Separe por vírgulas."),
                ))
            })
            .when(self.kind == ChannelKind::TelegramChannel, |form| {
                form.child(field("Chat do Telegram", &self.chat).child(
                    div().text_xs().text_color(t.text2).child(
                        "O @nome público do canal ou, num canal privado, o id que começa                          com -100.",
                    ),
                ))
            })
            .children(self.form.as_ref().map(|outcome| notice(outcome, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("save-channel")
                            .label(if self.editing.is_some() {
                                "Salvar canal"
                            } else {
                                "Criar canal"
                            })
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .when(self.editing.is_some(), |buttons| {
                        buttons.child(
                            Button::new("cancel-channel")
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

impl Render for PromotionScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let mut parts = ScreenParts::new("Divulgação");
        if promotion_channels(cx).is_none() {
            parts
                .notices
                .push(kit::error_notice(NO_DATABASE, cx).into_any_element());
            return layout::screen(parts, cx);
        }
        parts
            .notices
            .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        let rows: Vec<AnyElement> = self
            .channels
            .clone()
            .iter()
            .enumerate()
            .map(|(index, channel)| self.render_channel(index, channel, cx))
            .collect();
        parts.content.push(
            v_flex()
                .gap_3()
                .child(kit::section_heading("Canais de divulgação"))
                .child(div().text_sm().text_color(t.text2).child(
                    "Onde você divulga: canal do Telegram, grupo de WhatsApp, Instagram,                      TikTok, busca paga. Cada canal tem uma marcação, que vai nos links de                      afiliado criados em Afiliação.",
                ))
                .when(rows.is_empty(), |list| {
                    list.child(
                        div()
                            .text_sm()
                            .text_color(t.text2)
                            .child("Nenhum canal ainda: crie o primeiro abaixo."),
                    )
                })
                .children(rows)
                .into_any_element(),
        );
        parts.content.push(self.render_form(cx));
        layout::screen(parts, cx)
    }
}
