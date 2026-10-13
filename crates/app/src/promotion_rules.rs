//! Regras de divulgação on the Afiliação screen (#98): each Program with
//! what it allows, forbids or leaves unread, starting from its Platform's
//! terms, for the owner to edit as the producer's page says. The rules and
//! their defaults live in the Affiliate module (ADR 0031).

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Entity, Window, div, px};
use mascate_affiliate::{
    Affiliate, AffiliateError, AffiliatePlatform, Practice, Program, PromotionRules, Standing,
};
use mascate_kernel::RecordId;

use crate::affiliate::{affiliate, failure};
use crate::appearance::look;
use crate::forms::{Outcome, input, notice};
use crate::kit;
use crate::sales::table_row;

fn practice_name(practice: Practice) -> &'static str {
    match practice {
        Practice::PaidSearch => "Busca paga (Google Ads, Bing Ads)",
        Practice::BoostedPosts => "Impulsionar post nas suas redes",
        Practice::Email => "E-mail",
        Practice::DirectMessage => "Mensagem direta ou SMS não solicitados",
        Practice::Offline => "Divulgação offline",
        Practice::Shortener => "Encurtador de link",
    }
}

fn standing_name(standing: Standing) -> &'static str {
    match standing {
        Standing::Allowed => "Permitido",
        Standing::Forbidden => "Proibido",
        Standing::Unknown => "Não verificado",
    }
}

/// Where the rules of `platform` come from, and where the owner reads them.
fn source_note(platform: AffiliatePlatform) -> String {
    match platform {
        AffiliatePlatform::Shopee => {
            "O padrão vem dos termos da Shopee Afiliados (2.3 e 4.3).".into()
        }
        AffiliatePlatform::Amazon => "O padrão vem das regras do Associados da Amazon, que \
                                      também pedem a data e a hora ao lado de um preço."
            .into(),
        AffiliatePlatform::Hotmart
        | AffiliatePlatform::Kiwify
        | AffiliatePlatform::Eduzz
        | AffiliatePlatform::Monetizze => format!(
            "Na {} quem decide é o produtor, produto a produto: as regras ficam na página do \
             produto na área de afiliados. Até você conferir, cada uma fica não verificada e \
             avisa como proibida.",
            platform.name()
        ),
        AffiliatePlatform::MercadoLivre | AffiliatePlatform::Magalu | AffiliatePlatform::Other => {
            "O app ainda não leu os termos desta plataforma: cada regra fica não verificada e \
             avisa como proibida até você conferir."
                .into()
        }
    }
}

/// "2 proibidas · 4 não verificadas", or that everything is allowed.
fn summary(rules: &PromotionRules) -> String {
    let count = |standing| {
        Practice::ALL
            .into_iter()
            .filter(|practice| rules.standing(*practice) == standing)
            .count()
    };
    let parts: Vec<String> = [
        (count(Standing::Forbidden), "proibida", "proibidas"),
        (
            count(Standing::Unknown),
            "não verificada",
            "não verificadas",
        ),
    ]
    .into_iter()
    .filter(|(count, _, _)| *count > 0)
    .map(|(count, one, many)| format!("{count} {}", if count == 1 { one } else { many }))
    .collect();
    if parts.is_empty() {
        "tudo permitido".into()
    } else {
        parts.join(" · ")
    }
}

/// The rules being edited, before they are saved.
struct Draft {
    program: RecordId,
    platform: AffiliatePlatform,
    rules: PromotionRules,
}

pub struct PromotionRulesSection {
    programs: Vec<Program>,
    editing: Option<Draft>,
    /// The brand keywords being edited, separated by commas.
    keywords: Entity<InputState>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
}

impl PromotionRulesSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            programs: Vec::new(),
            editing: None,
            keywords: input("amazon, kindle", window, cx),
            busy: false,
            reads: 0,
            outcome: None,
        }
    }

    /// Reads the Programs again.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx, |_| async { Ok(()) });
    }

    /// Runs `change` off the UI thread, then reads the Programs again.
    fn run<F, Fut>(&mut self, window: &mut Window, cx: &mut Context<Self>, change: F)
    where
        F: FnOnce(Arc<Affiliate>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), AffiliateError>> + Send,
    {
        let Some(affiliate) = affiliate(cx) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(affiliate.clone()).await;
            (changed, affiliate.programs().await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (changed, read_back) = working.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.reads == read {
                    this.busy = false;
                    match read_back {
                        Ok(programs) => this.programs = programs,
                        Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                    }
                }
                if let Err(error) = changed {
                    this.outcome = Some(Outcome::Failed(failure(&error).into()));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open(&mut self, program: &Program, window: &mut Window, cx: &mut Context<Self>) {
        self.fill_keywords(&program.rules, window, cx);
        self.editing = Some(Draft {
            program: program.id,
            platform: program.platform,
            rules: program.rules.clone(),
        });
        self.outcome = None;
        cx.notify();
    }

    fn fill_keywords(&self, rules: &PromotionRules, window: &mut Window, cx: &mut Context<Self>) {
        let typed = rules.brand_keywords.join(", ");
        self.keywords
            .update(cx, |field, cx| field.set_value(typed, window, cx));
    }

    fn set_standing(&mut self, practice: Practice, standing: Standing, cx: &mut Context<Self>) {
        if let Some(draft) = self.editing.as_mut() {
            draft.rules.set(practice, standing);
            cx.notify();
        }
    }

    fn restore_defaults(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(platform) = self.editing.as_ref().map(|draft| draft.platform) else {
            return;
        };
        let defaults = PromotionRules::defaults(platform);
        self.fill_keywords(&defaults, window, cx);
        if let Some(draft) = self.editing.as_mut() {
            draft.rules = defaults;
        }
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut draft) = self.editing.take() else {
            return;
        };
        draft.rules.brand_keywords = self
            .keywords
            .read(cx)
            .value()
            .split(',')
            .map(str::to_owned)
            .collect();
        let name = self
            .programs
            .iter()
            .find(|program| program.id == draft.program)
            .map_or_else(String::new, |program| program.name.clone());
        self.outcome = Some(Outcome::Done(
            format!("Regras de divulgação de {name} salvas. Uma sincronização nunca as muda.")
                .into(),
        ));
        self.run(window, cx, move |affiliate| async move {
            affiliate
                .set_promotion_rules(draft.program, &draft.rules)
                .await
        });
    }

    fn render_editor(&self, draft: &Draft, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let defaults = PromotionRules::defaults(draft.platform);
        let rows = Practice::ALL
            .into_iter()
            .enumerate()
            .map(|(row, practice)| {
                let current = draft.rules.standing(practice);
                let default = defaults.standing(practice);
                let chips = Standing::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(column, standing)| {
                        let label = if standing == default {
                            format!("{} (padrão)", standing_name(standing))
                        } else {
                            standing_name(standing).to_owned()
                        };
                        let chip = Button::new(("standing", row * Standing::ALL.len() + column))
                            .label(label)
                            .small()
                            .disabled(self.busy);
                        if standing == current {
                            chip.primary()
                        } else {
                            chip.ghost()
                        }
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_standing(practice, standing, cx)
                        }))
                    });
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(280.)).text_sm().child(practice_name(practice)))
                    .child(h_flex().gap_1().children(chips))
            });
        let default_keywords = if defaults.brand_keywords.is_empty() {
            "nenhuma".to_owned()
        } else {
            defaults.brand_keywords.join(", ")
        };
        v_flex()
            .gap_3()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.border)
            .bg(t.raised)
            .child(div().font_medium().child("Regras de divulgação"))
            .child(
                div()
                    .text_sm()
                    .text_color(t.text2)
                    .child(source_note(draft.platform)),
            )
            .children(rows)
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .child("Palavras de marca proibidas em busca paga"),
                    )
                    .child(div().w(px(420.)).child(Input::new(&self.keywords).small()))
                    .child(div().text_xs().text_color(t.text2).child(format!(
                        "Separe por vírgulas; maiúsculas e acentos não importam. Padrão: \
                         {default_keywords}."
                    ))),
            )
            .child(
                Checkbox::new("price-needs-time")
                    .label(if defaults.price_needs_time {
                        "Preço só com a data e a hora em que foi visto (padrão)"
                    } else {
                        "Preço só com a data e a hora em que foi visto"
                    })
                    .checked(draft.rules.price_needs_time)
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        if let Some(draft) = this.editing.as_mut() {
                            draft.rules.price_needs_time = *checked;
                            cx.notify();
                        }
                    })),
            )
            .child(div().text_xs().text_color(t.text2).child(
                "Uma regra proibida ou não verificada só avisa antes de divulgar. Link da \
                 Shopee em busca paga, palavra de marca em busca paga e preço da Amazon \
                 digitado por você ficam bloqueados até você ligá-los em Configurações › \
                 Funcionalidades restritas, mesmo que estejam permitidos aqui.",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("save-promotion-rules")
                            .label("Salvar regras")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .child(
                        Button::new("default-promotion-rules")
                            .label("Voltar ao padrão da plataforma")
                            .outline()
                            .small()
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.restore_defaults(window, cx)
                                }),
                            ),
                    )
                    .child(
                        Button::new("cancel-promotion-rules")
                            .label("Cancelar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.editing = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_row(&self, index: usize, program: &Program, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let edited = program.rules != PromotionRules::defaults(program.platform);
        let editor = self
            .editing
            .as_ref()
            .filter(|draft| draft.program == program.id)
            .map(|draft| self.render_editor(draft, cx));
        let opened = program.clone();
        let row = table_row(&t)
            .flex_wrap()
            .child(kit::tag(program.platform.name(), t.text2, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(220.))
                    .child(div().font_medium().truncate().child(program.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text2)
                            .child(summary(&program.rules)),
                    ),
            )
            .when(edited, |row| {
                row.child(kit::tag("editadas por você", t.accent_text, cx))
            })
            .child(
                Button::new(("open-promotion-rules", index))
                    .label("Regras de divulgação")
                    .outline()
                    .small()
                    .disabled(self.busy || editor.is_some())
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.open(&opened, window, cx)),
                    ),
            );
        v_flex()
            .gap_2()
            .child(row)
            .children(editor)
            .into_any_element()
    }
}

impl Render for PromotionRulesSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let section = v_flex()
            .gap_3()
            .child(kit::section_heading("Programas"))
            .child(div().text_sm().text_color(t.text2).child(
                "Cada programa tem as suas regras de divulgação: o que a plataforma ou o \
                 produtor permite, proíbe ou ainda não foi conferido. O app avisa antes de \
                 uma divulgação quebrar uma delas.",
            ))
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        if self.programs.is_empty() {
            return section.child(
                div()
                    .text_sm()
                    .text_color(t.text2)
                    .child("Nenhum programa ainda: eles aparecem aqui com a primeira comissão."),
            );
        }
        let rows: Vec<AnyElement> = self
            .programs
            .clone()
            .iter()
            .enumerate()
            .map(|(index, program)| self.render_row(index, program, cx))
            .collect();
        section.children(rows)
    }
}
