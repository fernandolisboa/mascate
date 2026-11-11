//! "Suas categorias" on Oportunidades (#237): the owner's Mascate
//! Categories, each grouping Mercado Livre categories at any level, found
//! by browsing the tree or by a word, with optional keywords.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Entity, EventEmitter, Window, div};
use mascate_catalog::{
    Catalog, CatalogError, CategoryNode, DemandCategory, DemandSource, MascateCategory,
    MascateCategoryDraft,
};
use mascate_integrations::{Connection, ConnectionState};
use mascate_kernel::{PlatformError, RecordId};

use crate::appearance::look;
use crate::catalog::{self, failure};
use crate::connections::AppConnections;
use crate::forms::{Outcome, input, notice};
use crate::kit;
use crate::mercado_livre;

/// The owner saved or removed a Mascate Category.
pub struct CategoriesChanged;

/// Where the picker of Mercado Livre categories stands.
enum Browsing {
    /// Nothing read yet.
    Closed,
    /// The top-level categories.
    Top(Vec<DemandCategory>),
    /// One category, with its path and subcategories.
    Inside(CategoryNode),
}

/// A Mascate Category being created (`id` empty) or changed.
struct Editing {
    id: Option<RecordId>,
    chosen: Vec<DemandCategory>,
    browsing: Browsing,
    /// What the last search by word found.
    found: Option<Vec<CategoryNode>>,
}

/// What a background step hands back.
enum Answer {
    Top(Vec<DemandCategory>),
    Inside(CategoryNode),
    Found(Vec<CategoryNode>),
}

pub struct MascateCategoriesSection {
    categories: Vec<MascateCategory>,
    connected: bool,
    editing: Option<Editing>,
    /// The category whose removal waits for a second click.
    removing: Option<RecordId>,
    name: Entity<InputState>,
    keywords: Entity<InputState>,
    words: Entity<InputState>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
}

impl EventEmitter<CategoriesChanged> for MascateCategoriesSection {}

impl MascateCategoriesSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut section = Self {
            categories: Vec::new(),
            connected: false,
            editing: None,
            removing: None,
            name: input("Ex.: Café", window, cx),
            keywords: input("Ex.: coado, moedor manual", window, cx),
            words: input("Ex.: moedor de café", window, cx),
            busy: false,
            reads: 0,
            outcome: None,
        };
        section.refresh(window, cx);
        section
    }

    /// Reads the categories again, as when the screen comes into view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save(window, cx, |_| async { Ok(()) }, |_, _| {});
    }

    /// Runs `change` on the catalog off the UI thread, then reads the
    /// categories again and calls `done` when `change` went through.
    fn save<F, Fut>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: F,
        done: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) where
        F: FnOnce(Arc<Catalog>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), CatalogError>> + Send,
    {
        let Some(catalog) = catalog::catalog(cx) else {
            return;
        };
        let connections = cx.global::<AppConnections>().0.clone();
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(catalog.clone()).await;
            let state = connections.state(Connection::MercadoLivre);
            (changed, catalog.mascate_categories().await, state)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (changed, categories, state) = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads == read {
                    this.busy = false;
                    this.connected = state == ConnectionState::Connected;
                    match categories {
                        Ok(categories) => this.categories = categories,
                        Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                    }
                }
                match changed {
                    Ok(()) => done(this, cx),
                    Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Asks Mercado Livre off the UI thread and shows the answer in the
    /// picker.
    fn ask(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        question: impl FnOnce(&dyn DemandSource) -> Result<Answer, PlatformError> + Send + 'static,
    ) {
        let Some(source) = mercado_livre::adapter(cx) else {
            return;
        };
        self.busy = true;
        self.outcome = None;
        let working = cx
            .background_executor()
            .spawn(async move { question(source.as_ref()) });
        cx.spawn_in(window, async move |this, cx| {
            let answer = working.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match (answer, &mut this.editing) {
                    (Ok(answer), Some(editing)) => match answer {
                        Answer::Top(top) => editing.browsing = Browsing::Top(top),
                        Answer::Inside(node) => editing.browsing = Browsing::Inside(node),
                        Answer::Found(found) => editing.found = Some(found),
                    },
                    (Ok(_), None) => {}
                    (Err(error), _) => {
                        this.outcome = Some(Outcome::Failed(mercado_livre::failure(&error).into()))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open_top(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ask(window, cx, |source| source.categories().map(Answer::Top));
    }

    fn open(&mut self, category: String, window: &mut Window, cx: &mut Context<Self>) {
        self.ask(window, cx, move |source| {
            source.category(&category).map(Answer::Inside)
        });
    }

    fn find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let words = self.words.read(cx).value().trim().to_owned();
        if words.is_empty() {
            self.outcome = Some(Outcome::Failed(
                "Digite uma palavra para buscar, como moedor de café.".into(),
            ));
            cx.notify();
            return;
        }
        self.ask(window, cx, move |source| {
            source.find_categories(&words).map(Answer::Found)
        });
    }

    fn start(
        &mut self,
        category: Option<&MascateCategory>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (name, keywords) = category.map_or((String::new(), String::new()), |category| {
            (category.name.clone(), keywords_text(&category.keywords))
        });
        self.name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.keywords
            .update(cx, |input, cx| input.set_value(keywords, window, cx));
        self.words
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.editing = Some(Editing {
            id: category.map(|category| category.id),
            chosen: category.map_or_else(Vec::new, |c| c.channel_categories.clone()),
            browsing: Browsing::Closed,
            found: None,
        });
        self.removing = None;
        self.outcome = None;
        if self.connected {
            self.open_top(window, cx);
        }
        cx.notify();
    }

    fn choose(&mut self, category: DemandCategory, cx: &mut Context<Self>) {
        if let Some(editing) = &mut self.editing
            && !editing.chosen.iter().any(|known| known.id == category.id)
        {
            editing.chosen.push(category);
        }
        cx.notify();
    }

    fn drop_choice(&mut self, category: &str, cx: &mut Context<Self>) {
        if let Some(editing) = &mut self.editing {
            editing.chosen.retain(|known| known.id != category);
        }
        cx.notify();
    }

    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = &self.editing else {
            return;
        };
        let draft = MascateCategoryDraft {
            name: self.name.read(cx).value().to_string(),
            channel_categories: editing.chosen.clone(),
            keywords: keywords_from(&self.keywords.read(cx).value()),
        };
        let id = editing.id;
        let text = format!(
            "Categoria {} salva. O próximo Sync lê os mais vendidos dela.",
            draft.name.trim()
        );
        self.outcome = None;
        self.save(
            window,
            cx,
            move |catalog| async move {
                match id {
                    Some(id) => catalog.edit_mascate_category(id, draft).await.map(|_| ()),
                    None => catalog.create_mascate_category(draft).await.map(|_| ()),
                }
            },
            move |this, cx| {
                this.editing = None;
                this.outcome = Some(Outcome::Done(text.into()));
                cx.emit(CategoriesChanged);
            },
        );
    }

    fn remove(&mut self, category: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.removing = None;
        self.outcome = None;
        self.save(
            window,
            cx,
            move |catalog| async move { catalog.remove_mascate_category(category).await },
            |this, cx| {
                this.outcome = Some(Outcome::Done("Categoria removida.".into()));
                cx.emit(CategoriesChanged);
            },
        );
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let mut list = v_flex().gap_2();
        if self.categories.is_empty() {
            list = list.child(div().text_sm().text_color(t.text2).child(
                "Nenhuma categoria ainda. Crie uma para cada assunto que você quer garimpar, \
                 como Café ou Pets, e escolha as categorias do Mercado Livre que entram nela, \
                 de qualquer nível.",
            ));
        }
        for (index, category) in self.categories.iter().enumerate() {
            let editable = category.clone();
            let id = category.id;
            let confirming = self.removing == Some(id);
            list = list.child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .p_3()
                    .rounded(t.radius_lg)
                    .border(t.border_width)
                    .border_color(t.frame)
                    .bg(t.surface)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(div().font_medium().child(category.name.clone()))
                            .child(h_flex().flex_wrap().gap_1().children(
                                category.channel_categories.iter().map(|channel| {
                                    kit::tag(channel.name.clone(), t.accent_text, cx)
                                }),
                            ))
                            .when(!category.keywords.is_empty(), |column| {
                                column.child(div().text_xs().text_color(t.text2).child(format!(
                                    "Só títulos com: {}",
                                    keywords_text(&category.keywords)
                                )))
                            }),
                    )
                    .when(!confirming, |row| {
                        row.child(
                            Button::new(("edit-mascate-category", index))
                                .label("Editar")
                                .ghost()
                                .small()
                                .disabled(self.busy || self.editing.is_some())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.start(Some(&editable), window, cx)
                                })),
                        )
                        .child(
                            Button::new(("remove-mascate-category", index))
                                .label("Remover")
                                .ghost()
                                .small()
                                .disabled(self.busy || self.editing.is_some())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.removing = Some(id);
                                    cx.notify();
                                })),
                        )
                    })
                    .when(confirming, |row| {
                        row.child(
                            div()
                                .text_sm()
                                .text_color(t.text2)
                                .child(format!("Remover {}?", category.name)),
                        )
                        .child(
                            Button::new(("confirm-remove-mascate-category", index))
                                .label("Remover")
                                .danger()
                                .small()
                                .disabled(self.busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.remove(id, window, cx)
                                })),
                        )
                        .child(
                            Button::new(("keep-mascate-category", index))
                                .label("Manter")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.removing = None;
                                    cx.notify();
                                })),
                        )
                    }),
            );
        }
        if self.editing.is_none() {
            list = list.child(
                h_flex().child(
                    Button::new("new-mascate-category")
                        .label("Nova categoria")
                        .outline()
                        .small()
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.start(None, window, cx))),
                ),
            );
        }
        list.into_any_element()
    }

    fn render_editor(&self, editing: &Editing, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, hint: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .flex_1()
                .min_w_0()
                .child(div().text_xs().text_color(t.text2).child(label))
                .child(Input::new(input).small())
                .child(div().text_xs().text_color(t.text2).child(hint))
        };
        let chosen = if editing.chosen.is_empty() {
            div()
                .text_sm()
                .text_color(t.text2)
                .child("Nenhuma ainda: busque por uma palavra ou navegue pelas categorias abaixo.")
                .into_any_element()
        } else {
            h_flex()
                .flex_wrap()
                .gap_1()
                .children(editing.chosen.iter().enumerate().map(|(index, category)| {
                    let id = category.id.clone();
                    Button::new(("drop-channel-category", index))
                        .label(format!("{} ✕", category.name))
                        .primary()
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| this.drop_choice(&id, cx)))
                }))
                .into_any_element()
        };
        v_flex()
            .gap_3()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.accent)
            .bg(t.raised)
            .child(div().font_medium().child(if editing.id.is_some() {
                "Editar categoria"
            } else {
                "Nova categoria"
            }))
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .items_start()
                    .child(field("Nome", "Serve para qualquer assunto.", &self.name))
                    .child(field(
                        "Palavras-chave (opcional)",
                        "Separadas por vírgula. Com elas, só ficam os produtos com uma delas \
                         no título; sem elas, ficam todos.",
                        &self.keywords,
                    )),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div().text_xs().text_color(t.text2).child(
                            "Categorias do Mercado Livre que entram nela (clique para tirar)",
                        ),
                    )
                    .child(chosen),
            )
            .child(self.render_picker(editing, cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("save-mascate-category")
                            .label("Salvar categoria")
                            .primary()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.finish(window, cx))),
                    )
                    .child(
                        Button::new("cancel-mascate-category")
                            .label("Cancelar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.editing = None;
                                this.outcome = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_picker(&self, editing: &Editing, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        if !self.connected {
            return div()
                .text_sm()
                .text_color(t.text2)
                .child(
                    "Conecte o Mercado Livre em Configurações › Conexões para buscar e \
                     escolher as categorias dele.",
                )
                .into_any_element();
        }
        let chosen = |id: &str| editing.chosen.iter().any(|known| known.id == id);
        let row = |key: &'static str,
                   index: usize,
                   category: &DemandCategory,
                   path: Option<String>,
                   can_open: bool,
                   cx: &mut Context<Self>| {
            let picked = category.clone();
            let opened = category.id.clone();
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().child(category.name.clone()))
                        .children(path.map(|path| {
                            div().text_xs().text_color(t.text2).truncate().child(path)
                        })),
                )
                .child(
                    Button::new((key, index * 2))
                        .label(if chosen(&category.id) {
                            "Adicionada"
                        } else {
                            "Adicionar"
                        })
                        .outline()
                        .small()
                        .disabled(chosen(&category.id))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.choose(picked.clone(), cx)),
                        ),
                )
                .when(can_open, |row| {
                    row.child(
                        Button::new((key, index * 2 + 1))
                            .label("Subcategorias")
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open(opened.clone(), window, cx)
                            })),
                    )
                })
        };
        let search = h_flex()
            .gap_2()
            .items_end()
            .child(
                v_flex()
                    .gap_1()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text2)
                            .child("Buscar categoria do Mercado Livre por palavra"),
                    )
                    .child(Input::new(&self.words).small()),
            )
            .child(
                Button::new("find-channel-categories")
                    .label("Buscar")
                    .outline()
                    .small()
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, window, cx| this.find(window, cx))),
            );
        let found =
            editing.found.as_ref().map(|found| {
                let mut list = v_flex().gap_1();
                if found.is_empty() {
                    list =
                        list.child(div().text_sm().text_color(t.text2).child(
                            "O Mercado Livre não sugeriu nenhuma categoria para essa busca.",
                        ));
                }
                for (index, node) in found.iter().enumerate() {
                    list = list.child(row(
                        "found-channel-category",
                        index,
                        &node.category,
                        Some(node.path_text()),
                        !node.children.is_empty(),
                        cx,
                    ));
                }
                list
            });
        let browse = match &editing.browsing {
            Browsing::Closed => v_flex().child(
                h_flex().child(
                    Button::new("browse-channel-categories")
                        .label("Navegar pelas categorias")
                        .ghost()
                        .small()
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.open_top(window, cx))),
                ),
            ),
            Browsing::Top(top) => {
                let mut list = v_flex().gap_1().child(
                    div()
                        .text_xs()
                        .text_color(t.text2)
                        .child("Todas as categorias do Mercado Livre"),
                );
                for (index, category) in top.iter().enumerate() {
                    list = list.child(row("top-channel-category", index, category, None, true, cx));
                }
                list
            }
            Browsing::Inside(node) => {
                let mut trail = h_flex().flex_wrap().gap_1().items_center().child(
                    Button::new("trail-top")
                        .label("Todas")
                        .ghost()
                        .small()
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.open_top(window, cx))),
                );
                for (index, step) in node.path.iter().enumerate() {
                    let id = step.id.clone();
                    trail = trail.child(div().text_color(t.text2).child("›")).child(
                        Button::new(("trail-step", index))
                            .label(step.name.clone())
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open(id.clone(), window, cx)
                            })),
                    );
                }
                trail = trail.child(div().text_color(t.text2).child("›")).child(
                    div()
                        .text_sm()
                        .font_medium()
                        .child(node.category.name.clone()),
                );
                let mut list = v_flex().gap_1().child(trail).child(row(
                    "this-channel-category",
                    0,
                    &node.category,
                    None,
                    false,
                    cx,
                ));
                if node.children.is_empty() {
                    list = list.child(
                        div()
                            .text_xs()
                            .text_color(t.text2)
                            .child("Esta categoria não tem subcategorias."),
                    );
                } else {
                    list = list.child(div().text_xs().text_color(t.text2).child("Subcategorias"));
                }
                for (index, child) in node.children.iter().enumerate() {
                    list = list.child(row("child-channel-category", index, child, None, true, cx));
                }
                list
            }
        };
        v_flex()
            .gap_3()
            .child(search)
            .children(found)
            .child(browse)
            .into_any_element()
    }
}

impl Render for MascateCategoriesSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut section = v_flex()
            .gap_2()
            .child(kit::section_heading("Suas categorias"))
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .child(self.render_list(cx));
        if let Some(editing) = &self.editing {
            section = section.child(self.render_editor(editing, cx));
        }
        section
    }
}

/// The keywords the owner typed, separated by commas, semicolons or lines.
fn keywords_from(text: &str) -> Vec<String> {
    text.split([',', ';', '\n'])
        .map(str::trim)
        .filter(|keyword| !keyword.is_empty())
        .map(Into::into)
        .collect()
}

/// Keywords as the owner types them.
fn keywords_text(keywords: &[String]) -> String {
    keywords.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_are_typed_separated_by_commas_and_shown_the_same_way() {
        let keywords = keywords_from(" coado, moedor manual;;\nprensa francesa ,");

        assert_eq!(keywords, ["coado", "moedor manual", "prensa francesa"]);
        assert_eq!(
            keywords_text(&keywords),
            "coado, moedor manual, prensa francesa"
        );
        assert!(keywords_from(" , ").is_empty());
    }
}
