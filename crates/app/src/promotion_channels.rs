//! Divulgação (#99): the owner's Offer Posts (#102) and Promotion
//! Channels, each channel with the marking its affiliate links carry, the
//! keywords of a paid-search channel, the chat of a Telegram channel and
//! the template its posts start from. The channels live in the Marketing
//! (ADR 0031, decision 5); Afiliação tags links with them. A Telegram
//! channel is checked through the bot (#103): administrator, allowed to
//! post. A Pinterest channel picks the board its Pins go to (#155). Every
//! channel the app posts to has the posting times its scheduled posts go
//! out at (#104, #157).
//! The Sales Events calendar sits between the posts and the channels (#105).
//! What each channel spent comes last (#195).

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, Global, Window, div, px};
use mascate_kernel::RecordId;
use mascate_marketing::{
    CHANNEL_NAME_MAX_CHARS, ChannelError, ChannelKind, DISCLOSURE, DestinationStanding,
    NewPromotionChannel, POST_PITCH, PinterestBoard, PostDestination as _, PromotionChannel,
    PromotionChannels, default_posting_times, suggested_tag,
};

use crate::appearance::look;
use crate::catalog::NO_DATABASE;
use crate::forms::{Outcome, input, notice};
use crate::kit;
use crate::layout;
use crate::offer_posts::{
    OfferPostsSection, StartPost, offer_posts, post_failure, template_variables,
};
use crate::parts::ScreenParts;
use crate::post_queue::{times_text, typed_times};
use crate::promotion_spends::SpendsSection;
use crate::sales::table_row;
use crate::sales_events::CalendarSection;
use crate::{pinterest, telegram};

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
        ChannelError::NoDestination => "Só um canal do Telegram, do Instagram ou do Pinterest tem \
                                        horários de publicação."
            .into(),
        ChannelError::NotPinterest => "Só um canal do Pinterest tem pasta.".into(),
        ChannelError::BadBoard => "Escolha uma das pastas que o Pinterest listou.".into(),
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
            "Com o chat do canal e o token do bot em Configurações › Conexões › Telegram, os \
             posts deste canal saem por \"Publicar agora\" ou \"Agendar\", sempre depois da sua \
             confirmação. Use \"Conferir canal\" para ver se o bot pode publicar e \
             \"Horários\" para escolher quando os agendados saem."
        }
        ChannelKind::Pinterest => {
            "Com a conta conectada em Configurações › Conexões › Pinterest, escolha em \"Pasta\" \
             onde os Pins deste canal entram: eles saem por \"Publicar agora\" ou \"Agendar\", \
             com a foto da oferta e o link de afiliado, sempre depois da sua confirmação. Use \
             \"Horários\" para escolher quando os agendados saem. Enquanto o app do Pinterest \
             estiver em Trial, só você vê os Pins."
        }
        ChannelKind::Instagram => {
            "Com o token da conta em Configurações › Conexões › Instagram, os posts deste canal \
             saem na conta conectada por \"Publicar agora\" ou \"Agendar\", sempre depois da \
             sua confirmação, com uma foto que já está na internet. Use \"Horários\" para \
             escolher quando os agendados saem."
        }
        ChannelKind::PaidSearch => {
            "As palavras-chave entram na conferência das regras de cada programa: a Shopee não \
             aceita link em busca paga, e marcas como amazon, shopee ou hotmart ficam \
             bloqueadas."
        }
        ChannelKind::PaidSocial => {
            "Anúncio que você paga no Gerenciador de Anúncios da Meta (Facebook e Instagram) ou no \
             TikTok. Conta como impulsionar post nas regras de cada programa. Lance o gasto à \
             mão ou importe o relatório da Meta em \"Gasto com divulgação\"."
        }
        ChannelKind::WhatsAppGroup | ChannelKind::TikTok | ChannelKind::Other => {
            "Um canal ou grupo seu, onde você posta de graça."
        }
    }
}

/// A channel's posting times open for editing.
struct TimesEditor {
    channel: RecordId,
    text: Entity<InputState>,
    outcome: Option<Outcome>,
}

/// The boards a Pinterest channel picks from.
struct BoardPicker {
    channel: RecordId,
    boards: Vec<PinterestBoard>,
}

/// A channel's post template open for editing.
struct TemplateEditor {
    channel: PromotionChannel,
    text: Entity<TextareaState>,
    outcome: Option<Outcome>,
}

pub struct PromotionScreen {
    posts: Entity<OfferPostsSection>,
    calendar: Entity<CalendarSection>,
    spends: Entity<SpendsSection>,
    channels: Vec<PromotionChannel>,
    template: Option<TemplateEditor>,
    times: Option<TimesEditor>,
    boards: Option<BoardPicker>,
    /// The Pinterest channel whose boards are being read.
    reading_boards: Option<RecordId>,
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
    /// What "Conferir canal" found, per channel.
    checks: HashMap<RecordId, Outcome>,
    /// The channel being checked.
    checking: Option<RecordId>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    form: Option<Outcome>,
    outcome: Option<Outcome>,
}

impl PromotionScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut screen = Self {
            posts: cx.new(|cx| OfferPostsSection::new(window, cx)),
            calendar: cx.new(|cx| CalendarSection::new(window, cx)),
            spends: cx.new(|_| SpendsSection::new()),
            channels: Vec::new(),
            template: None,
            times: None,
            boards: None,
            reading_boards: None,
            name: input("Ofertas do Fernando", window, cx),
            tag: input("Vazia, sai do nome", window, cx),
            keywords: input("fone bluetooth, fone sem fio", window, cx),
            chat: input("@ofertasdofernando ou -100…", window, cx),
            kind: ChannelKind::TelegramChannel,
            editing: None,
            deleting: None,
            checks: HashMap::new(),
            checking: None,
            busy: false,
            reads: 0,
            form: None,
            outcome: None,
        };
        screen.refresh(window, cx);
        screen
    }

    /// Reads the posts, the calendar and the channels again, as when the
    /// screen comes into view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.posts.update(cx, |posts, cx| posts.refresh(window, cx));
        self.calendar
            .update(cx, |calendar, cx| calendar.refresh(window, cx));
        self.run(window, cx, |_| async { Ok(()) }, |_, _, _, _| {});
    }

    /// Follows a new Work Context: the posts follow it, and the Promotion
    /// Channels, which promote on several Platforms, stay as they are.
    pub fn follow_context(&mut self, cx: &mut Context<Self>) {
        self.posts.update(cx, OfferPostsSection::follow_context);
        cx.notify();
    }

    /// Opens a new post from an Affiliate Link, as Afiliação and Ofertas ask.
    pub fn start_post(&mut self, start: &StartPost, window: &mut Window, cx: &mut Context<Self>) {
        self.posts
            .update(cx, |posts, cx| posts.start(start, window, cx));
    }

    /// Opens the template of `channel` for editing.
    fn open_template(
        &mut self,
        channel: PromotionChannel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(posts) = offer_posts(cx) else {
            return;
        };
        let text = cx.new(|cx| TextareaState::new(window, cx).auto_grow(6, 14));
        self.template = Some(TemplateEditor {
            channel: channel.clone(),
            text: text.clone(),
            outcome: None,
        });
        let reading = cx
            .background_executor()
            .spawn(async move { posts.template(&channel).await });
        cx.spawn_in(window, async move |this, cx| {
            let read = reading.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match read {
                    Ok(template) => {
                        text.update(cx, |field, cx| field.set_value(template, window, cx))
                    }
                    Err(error) => {
                        if let Some(editor) = this.template.as_mut() {
                            editor.outcome = Some(Outcome::Failed(post_failure(&error).into()));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Keeps the template typed, or with `reset` puts the channel back on
    /// its kind's default.
    fn save_template(&mut self, reset: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(posts), Some(editor)) = (offer_posts(cx), self.template.as_ref()) else {
            return;
        };
        let (channel, typed) = (
            editor.channel.clone(),
            editor.text.read(cx).value().to_string(),
        );
        let working = cx.background_executor().spawn(async move {
            if reset {
                posts.reset_template(channel.id).await?;
            } else {
                posts.set_template(channel.id, &typed).await?;
            }
            posts.template(&channel).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let saved = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(editor) = this.template.as_mut() else {
                    return;
                };
                match saved {
                    Ok(template) => {
                        editor
                            .text
                            .update(cx, |field, cx| field.set_value(template, window, cx));
                        editor.outcome = Some(Outcome::Done(
                            if reset {
                                "Modelo de volta ao padrão. Vale para os próximos posts."
                            } else {
                                "Modelo salvo. Vale para os próximos posts deste canal."
                            }
                            .into(),
                        ));
                    }
                    Err(error) => {
                        editor.outcome = Some(Outcome::Failed(post_failure(&error).into()))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_template(&self, editor: &TemplateEditor, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        v_flex()
            .gap_2()
            .ml_6()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.border)
            .bg(t.raised)
            .child(div().font_medium().child("Modelo do post"))
            .child(div().text_xs().text_color(t.text2).child(format!(
                "Use {}. Uma linha com um valor que o post não tem sai inteira, então deixe cada \
                 um na sua linha. {POST_PITCH} é a chamada que a IA escreve no post, vazia até você \
                 pedir. O app põe \"{DISCLOSURE}\" no fim quando o modelo não identifica a \
                 publicidade.",
                template_variables()
            )))
            .child(Textarea::new(&editor.text).small())
            .children(editor.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("save-template")
                            .label("Salvar modelo")
                            .primary()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_template(false, window, cx)
                            })),
                    )
                    .child(
                        Button::new("reset-template")
                            .label("Voltar ao padrão")
                            .outline()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_template(true, window, cx)
                            })),
                    )
                    .child(
                        Button::new("close-template")
                            .label("Fechar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.template = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    /// Opens the posting times of `channel` for editing.
    fn open_times(
        &mut self,
        channel: &PromotionChannel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = input("08:00, 12:00, 19:00, 21:00", window, cx);
        text.update(cx, |field, cx| {
            field.set_value(times_text(&channel.posting_times), window, cx)
        });
        self.times = Some(TimesEditor {
            channel: channel.id,
            text,
            outcome: None,
        });
        cx.notify();
    }

    /// Keeps the posting times typed, or with `reset` the default ones.
    fn save_times(&mut self, reset: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.times.as_mut() else {
            return;
        };
        let times = if reset {
            default_posting_times()
        } else {
            match typed_times(&editor.text.read(cx).value()) {
                Ok(times) => times,
                Err(wrong) => {
                    editor.outcome = Some(Outcome::Failed(
                        format!(
                            "\"{wrong}\" não é um horário: escreva como 08:00, 12:30 ou 19h, \
                             separados por vírgula."
                        )
                        .into(),
                    ));
                    cx.notify();
                    return;
                }
            }
        };
        let id = editor.channel;
        self.run(
            window,
            cx,
            move |channels| async move { channels.set_posting_times(id, &times).await },
            move |this, saved: PromotionChannel, window, cx| {
                if let Some(editor) = this.times.as_mut() {
                    editor.text.update(cx, |field, cx| {
                        field.set_value(times_text(&saved.posting_times), window, cx)
                    });
                    editor.outcome = Some(Outcome::Done(
                        if saved.posting_times.is_empty() {
                            "Sem horários: os posts deste canal só se agendam com dia e hora \
                             escolhidos."
                                .to_owned()
                        } else {
                            format!(
                                "Horários salvos: {}. Os posts já agendados ficam na hora que \
                                 têm.",
                                times_text(&saved.posting_times)
                            )
                        }
                        .into(),
                    ));
                }
                Self::changed(cx);
            },
        );
    }

    fn render_times(&self, editor: &TimesEditor, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        v_flex()
            .gap_2()
            .ml_6()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.border)
            .bg(t.raised)
            .child(div().font_medium().child("Horários de publicação"))
            .child(div().text_xs().text_color(t.text2).child(
                "No horário de Brasília, separados por vírgula. \"Agendar\" usa o primeiro \
                 horário livre deste canal, hoje ou nos próximos dias; vazio, cada post pede dia e \
                 hora.",
            ))
            .child(div().w(px(360.)).child(Input::new(&editor.text).small()))
            .children(editor.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("save-times")
                            .label("Salvar horários")
                            .primary()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_times(false, window, cx)
                            })),
                    )
                    .child(
                        Button::new("reset-times")
                            .label("Voltar ao padrão")
                            .outline()
                            .small()
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.save_times(true, window, cx)
                                }),
                            ),
                    )
                    .child(
                        Button::new("close-times")
                            .label("Fechar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.times = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
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
                        Ok(channels) => {
                            this.spends
                                .update(cx, |spends, cx| spends.set_channels(channels.clone(), cx));
                            this.channels = channels;
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
                // Its chat may have changed: an earlier check no longer says.
                this.checks.remove(&saved.id);
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

    /// Asks Telegram whether the bot can post in `channel`'s chat.
    fn check(&mut self, channel: &PromotionChannel, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(telegram), Some(chat)) = (telegram::adapter(cx), channel.telegram_chat.clone())
        else {
            return;
        };
        let id = channel.id;
        self.checking = Some(id);
        let working = cx
            .background_executor()
            .spawn(async move { telegram.check(&chat) });
        cx.spawn_in(window, async move |this, cx| {
            let checked = working.await;
            let _ = this.update(cx, |this, cx| {
                this.checking = None;
                let outcome = match checked {
                    Ok(check) if check.standing == DestinationStanding::CanPost => {
                        Outcome::Done(telegram::check_text(&check).into())
                    }
                    Ok(check) => Outcome::Failed(telegram::check_text(&check).into()),
                    Err(error) => {
                        if error == mascate_kernel::PlatformError::Expired {
                            telegram::expired(cx);
                        }
                        Outcome::Failed(telegram::check_failure(&error).into())
                    }
                };
                this.checks.insert(id, outcome);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Reads the boards of the Pinterest account, for `channel` to pick one.
    fn open_boards(&mut self, channel: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pinterest) = pinterest::adapter(cx) else {
            return;
        };
        self.reading_boards = Some(channel);
        self.boards = None;
        self.checks.remove(&channel);
        let working = cx
            .background_executor()
            .spawn(async move { pinterest.boards() });
        cx.spawn_in(window, async move |this, cx| {
            let read = working.await;
            let _ = this.update(cx, |this, cx| {
                this.reading_boards = None;
                match read {
                    Ok(boards) if boards.is_empty() => {
                        this.checks.insert(
                            channel,
                            Outcome::Failed(
                                "A conta do Pinterest não tem pastas: crie uma no Pinterest e \
                                 abra \"Pasta\" de novo."
                                    .into(),
                            ),
                        );
                    }
                    Ok(boards) => this.boards = Some(BoardPicker { channel, boards }),
                    Err(error) => {
                        if error == mascate_kernel::PlatformError::Expired {
                            pinterest::expired(cx);
                        }
                        this.checks.insert(
                            channel,
                            Outcome::Failed(pinterest::boards_failure(&error).into()),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn pick_board(
        &mut self,
        channel: RecordId,
        board: PinterestBoard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.boards = None;
        self.run(
            window,
            cx,
            move |channels| async move { channels.set_pinterest_board(channel, &board).await },
            move |this, saved: PromotionChannel, _, cx| {
                let board = saved
                    .pinterest_board
                    .map_or_else(String::new, |board| board.name);
                this.checks.insert(
                    channel,
                    Outcome::Done(
                        format!("Os Pins do canal {} vão para a pasta {board}.", saved.name).into(),
                    ),
                );
                Self::changed(cx);
            },
        );
    }

    fn render_boards(&self, picker: &BoardPicker, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let current = self
            .channels
            .iter()
            .find(|channel| channel.id == picker.channel)
            .and_then(|channel| channel.pinterest_board.as_ref())
            .map(|board| board.id.clone());
        let channel = picker.channel;
        let chips = picker.boards.iter().enumerate().map(|(index, board)| {
            let picked = board.clone();
            let chip = Button::new(("pinterest-board", index))
                .label(board.name.clone())
                .small()
                .disabled(self.busy);
            if current.as_deref() == Some(board.id.as_str()) {
                chip.primary()
            } else {
                chip.outline()
            }
            .on_click(cx.listener(move |this, _, window, cx| {
                this.pick_board(channel, picked.clone(), window, cx)
            }))
        });
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .child(
                div()
                    .text_sm()
                    .child("Em qual pasta os Pins deste canal entram?"),
            )
            .child(h_flex().flex_wrap().gap_1().children(chips))
            .child(
                h_flex().child(
                    Button::new("close-boards")
                        .label("Fechar")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.boards = None;
                            cx.notify();
                        })),
                ),
            )
            .into_any_element()
    }

    fn render_channel(
        &self,
        index: usize,
        channel: &PromotionChannel,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let times = if channel.posting_times.is_empty() {
            "sem horários de publicação".to_owned()
        } else {
            format!("horários {}", times_text(&channel.posting_times))
        };
        let detail = match channel.kind {
            ChannelKind::TelegramChannel => Some(format!(
                "{} · {times}",
                channel.telegram_chat.clone().map_or_else(
                    || "chat ainda não informado".to_owned(),
                    |chat| format!("chat {chat}"),
                ),
            )),
            ChannelKind::Pinterest => Some(format!(
                "{} · {times}",
                channel.pinterest_board.as_ref().map_or_else(
                    || "pasta ainda não escolhida".to_owned(),
                    |board| format!("pasta {}", board.name),
                ),
            )),
            ChannelKind::Instagram => Some(format!("conta conectada · {times}")),
            ChannelKind::PaidSearch if channel.keywords.is_empty() => {
                Some("nenhuma palavra-chave".to_owned())
            }
            ChannelKind::PaidSearch => {
                Some(format!("palavras-chave: {}", channel.keywords.join(", ")))
            }
            _ => None,
        };
        let edited = channel.clone();
        let templated = channel.clone();
        let checked = channel.clone();
        let timed = channel.clone();
        let id = channel.id;
        let check = (channel.kind == ChannelKind::TelegramChannel
            && channel.telegram_chat.is_some())
        .then(|| {
            Button::new(("check-channel", index))
                .label("Conferir canal")
                .outline()
                .small()
                .loading(self.checking == Some(id))
                .disabled(self.busy || self.checking.is_some())
                .on_click(cx.listener(move |this, _, window, cx| this.check(&checked, window, cx)))
        });
        let board = (channel.kind == ChannelKind::Pinterest).then(|| {
            Button::new(("channel-board", index))
                .label("Pasta")
                .outline()
                .small()
                .loading(self.reading_boards == Some(id))
                .disabled(self.busy || self.reading_boards.is_some())
                .on_click(cx.listener(move |this, _, window, cx| this.open_boards(id, window, cx)))
        });
        let times = channel.kind.has_destination().then(|| {
            Button::new(("channel-times", index))
                .label("Horários")
                .outline()
                .small()
                .disabled(self.busy)
                .on_click(
                    cx.listener(move |this, _, window, cx| this.open_times(&timed, window, cx)),
                )
        });
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
                .flex_wrap()
                .children(check)
                .children(board)
                .children(times)
                .child(
                    Button::new(("channel-template", index))
                        .label("Modelo do post")
                        .outline()
                        .small()
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_template(templated.clone(), window, cx)
                        })),
                )
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
            .children(
                self.checks
                    .get(&id)
                    .map(|outcome| div().w_full().child(notice(outcome, cx))),
            )
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
                    "Letras minúsculas, números e _, até 20. Vai em cada link de afiliado deste canal e volta nas vendas, ligando cada comissão ao canal.",
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
                        "O @nome público do canal ou, num canal privado, o id que começa com -100.",
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
        let mut rows: Vec<AnyElement> = Vec::new();
        for (index, channel) in self.channels.clone().iter().enumerate() {
            rows.push(self.render_channel(index, channel, cx));
            if let Some(editor) = self
                .template
                .as_ref()
                .filter(|editor| editor.channel.id == channel.id)
            {
                rows.push(self.render_template(editor, cx));
            }
            if let Some(editor) = self
                .times
                .as_ref()
                .filter(|editor| editor.channel == channel.id)
            {
                rows.push(self.render_times(editor, cx));
            }
            if let Some(picker) = self
                .boards
                .as_ref()
                .filter(|picker| picker.channel == channel.id)
            {
                rows.push(self.render_boards(picker, cx));
            }
        }
        parts.content.push(self.posts.clone().into_any_element());
        parts.content.push(self.calendar.clone().into_any_element());
        parts.content.push(
            v_flex()
                .gap_3()
                .child(kit::section_heading("Canais de divulgação"))
                .child(div().text_sm().text_color(t.text2).child(
                    "Onde você divulga: canal do Telegram, grupo de WhatsApp, Instagram, TikTok, Pinterest, busca paga, anúncio pago na Meta. Cada canal tem uma marcação, que vai nos links de afiliado criados em Afiliação.",
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
        parts.content.push(self.spends.clone().into_any_element());
        layout::screen(parts, cx)
    }
}
