//! Rascunhos de anúncio (#16), on the Anúncios screen: a draft from a
//! Product with the category Mercado Livre predicts, the category's
//! attributes, pictures from the Product's folder and the suggested price;
//! the checklist of what is missing, Mercado Livre's validator, and
//! publishing, only on the owner's click.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::select::Select;
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, EventEmitter, Window, div, px};
use mascate_catalog::{Catalog, Product};
use mascate_commerce::{
    AttributeValue, CatalogProduct, CategoryPrediction, Check, ChecklistItem, Condition,
    CostSource, DraftEdit, DraftPrice, DraftStart, Listing, ListingDraft, ListingError, Listings,
    MIN_PICTURES, PricingError, Requirement, is_blocked, is_picture,
};
use mascate_kernel::{ListingType, Money, RecordId, parse_amount};

use crate::appearance::look;
use crate::catalog::{self, product_choice};
use crate::forms::{Outcome, Picker, amount_text, input, notice, picker, refill};
use crate::kit;
use crate::listings;
use crate::mercado_livre;
use crate::pricing;
use crate::stock;

/// A draft went out to Mercado Livre: the listings need reading again.
pub struct DraftPublished;

/// The owner asked for a draft of the Product, from its sheet.
pub struct StartDraft(pub RecordId);

/// Everything the section shows, read in one go off the UI thread.
struct Snapshot {
    drafts: Vec<ListingDraft>,
    products: Vec<Product>,
}

/// The draft open for editing, below its row.
struct Editor {
    draft: RecordId,
    title: Entity<InputState>,
    description: Entity<TextareaState>,
    price: Entity<InputState>,
    quantity: Entity<InputState>,
    warranty: Entity<InputState>,
    listing_type: ListingType,
    condition: Condition,
    /// One field per attribute, by the attribute's id.
    attributes: Vec<(String, Entity<InputState>)>,
    /// The picture files in the Product's folder.
    folder: Vec<PathBuf>,
    /// The ones chosen, in order: the first is the cover.
    pictures: Vec<PathBuf>,
    /// The suggested price, or why there is none.
    price_suggestion: Option<Result<DraftPrice, String>>,
    /// Categories Mercado Livre predicts for the title, once asked.
    predictions: Option<Vec<CategoryPrediction>>,
    /// The checklist with Mercado Livre's validator, until the next change.
    validated: Option<Vec<ChecklistItem>>,
}

pub struct DraftsSection {
    drafts: Vec<ListingDraft>,
    products: Vec<Product>,
    product_pick: Picker,
    editor: Option<Editor>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
}

impl EventEmitter<DraftPublished> for DraftsSection {}

impl DraftsSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            drafts: Vec::new(),
            products: Vec::new(),
            product_pick: picker(window, cx),
            editor: None,
            busy: false,
            reads: 0,
            outcome: None,
        }
    }

    /// Reads the drafts again, as when the screen comes into view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx, |_, _| async { Ok(None) }, |_, _: (), _, _| {});
    }

    /// Runs `change` off the UI thread, then reads the drafts again and
    /// hands what the change returned to `then`.
    fn run<T, F, Fut>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: F,
        then: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) where
        T: Send + 'static,
        F: FnOnce(Arc<Listings>, Arc<Catalog>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<T>, String>> + Send,
    {
        let (Some(listings), Some(catalog)) = (listings::listings(cx), catalog::catalog(cx)) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(listings.clone(), catalog.clone()).await;
            let read = async {
                Ok::<_, String>(Snapshot {
                    drafts: listings.drafts().await.map_err(|e| listings::failure(&e))?,
                    products: catalog.products().await.map_err(|e| catalog::failure(&e))?,
                })
            }
            .await;
            (changed, read)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (changed, read_back) = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reads == read {
                    this.busy = false;
                    match read_back {
                        Ok(snapshot) => this.show(snapshot, window, cx),
                        Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                    }
                }
                match changed {
                    Ok(Some(value)) => then(this, value, window, cx),
                    Ok(None) => {}
                    Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn show(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        refill(
            &self.product_pick,
            snapshot.products.iter().map(product_choice).collect(),
            window,
            cx,
        );
        if self.editor.as_ref().is_some_and(|editor| {
            !snapshot
                .drafts
                .iter()
                .any(|draft| draft.id == editor.draft && draft.published.is_none())
        }) {
            self.editor = None;
        }
        self.drafts = snapshot.drafts;
        self.products = snapshot.products;
    }

    fn connected(&self, cx: &App) -> bool {
        mercado_livre::adapter(cx).is_some()
            && cx
                .global::<crate::connections::AppConnections>()
                .0
                .state(mascate_integrations::Connection::MercadoLivre)
                == mascate_integrations::ConnectionState::Connected
    }

    fn draft(&self, id: RecordId) -> Option<&ListingDraft> {
        self.drafts.iter().find(|draft| draft.id == id)
    }

    /// A new draft of `product`, with its units on hand, its folder's
    /// pictures and the suggested price; it opens for editing.
    pub fn start(&mut self, product: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(channel), Some(pricing), Some(taxes), Some(inventory)) = (
            mercado_livre::adapter(cx),
            pricing::pricing(cx),
            pricing::taxes(cx),
            stock::inventory(cx),
        ) else {
            return;
        };
        self.outcome = None;
        self.editor = None;
        self.run(
            window,
            cx,
            move |listings, catalog| async move {
                let found = catalog
                    .product(product)
                    .await
                    .map_err(|e| catalog::failure(&e))?;
                let pictures = catalog
                    .files(product)
                    .await
                    .map_err(|e| catalog::failure(&e))?
                    .into_iter()
                    .map(|file| file.path)
                    .filter(|path| is_picture(path))
                    .collect();
                let on_hand = inventory
                    .stock()
                    .await
                    .map_err(|e| stock::inventory_failure(&e))?
                    .products
                    .into_iter()
                    .find(|stock| stock.product == product)
                    .map_or(0, |stock| stock.valuation.quantity());
                let draft = listings
                    .create_draft(
                        &CatalogProduct {
                            id: found.id,
                            sku: found.sku.to_string(),
                            name: found.name.clone(),
                        },
                        DraftStart {
                            available_quantity: u32::try_from(on_hand.max(0)).unwrap_or(u32::MAX),
                            pictures,
                        },
                        channel.as_ref(),
                    )
                    .await
                    .map_err(|e| failure(&e))?;
                // The suggested price, when there is one, is where the
                // draft starts; the owner changes it at will.
                let assumptions = pricing::assumptions(&catalog, &taxes).await?;
                let offer = catalog
                    .cheapest_offer(product)
                    .await
                    .map_err(|e| catalog::failure(&e))?;
                if let Ok(priced) = pricing
                    .draft_price(draft.id, channel.as_ref(), assumptions, offer)
                    .await
                {
                    let edit = DraftEdit {
                        price: priced.suggested.price,
                        ..edit_of(&draft)
                    };
                    listings
                        .save_draft(draft.id, edit)
                        .await
                        .map_err(|e| failure(&e))?;
                }
                Ok(Some(draft.id))
            },
            |this, draft: RecordId, window, cx| {
                this.outcome = Some(Outcome::Done(
                    "Rascunho criado. Revise o que o checklist pede e publique quando quiser; \
                     nada vai ao Mercado Livre antes disso."
                        .into(),
                ));
                this.open(draft, window, cx);
            },
        );
    }

    fn start_picked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.product_pick.read(cx).selected_value().copied() {
            Some(product) => self.start(product, window, cx),
            None => {
                self.outcome = Some(Outcome::Failed("Escolha o produto do rascunho.".into()));
                cx.notify();
            }
        }
    }

    /// Opens the draft for editing: its fields, its Product's pictures and
    /// the suggested price.
    fn open(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.draft(id).cloned() else {
            return;
        };
        let title = input("Título do anúncio", window, cx);
        let description = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 14)
                .placeholder("Descrição: o que é, o que vem na caixa, medidas, garantia.")
        });
        let price = input("0,00", window, cx);
        let quantity = input("0", window, cx);
        let warranty = input("Ex.: 90 dias", window, cx);
        title.update(cx, |field, cx| {
            field.set_value(draft.title.clone(), window, cx)
        });
        description.update(cx, |field, cx| {
            field.set_value(draft.description.clone(), window, cx)
        });
        price.update(cx, |field, cx| {
            field.set_value(price_text(draft.price), window, cx)
        });
        quantity.update(cx, |field, cx| {
            field.set_value(draft.available_quantity.to_string(), window, cx)
        });
        warranty.update(cx, |field, cx| {
            field.set_value(draft.warranty.clone(), window, cx)
        });
        let attributes = draft
            .attributes
            .iter()
            .map(|attribute| {
                let field = input("", window, cx);
                field.update(cx, |field, cx| {
                    field.set_value(attribute.value.clone(), window, cx)
                });
                (attribute.id.clone(), field)
            })
            .collect();
        self.editor = Some(Editor {
            draft: id,
            title,
            description,
            price,
            quantity,
            warranty,
            listing_type: draft.listing_type,
            condition: draft.condition,
            attributes,
            folder: Vec::new(),
            pictures: draft.pictures.iter().map(|p| p.path.clone()).collect(),
            price_suggestion: None,
            predictions: None,
            validated: None,
        });
        self.read_folder_and_price(id, window, cx);
        cx.notify();
    }

    /// The Product's pictures and the suggested price at the draft's
    /// category and type, off the UI thread.
    fn read_folder_and_price(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(channel), Some(pricing), Some(taxes), Some(catalog), Some(draft)) = (
            mercado_livre::adapter(cx),
            pricing::pricing(cx),
            pricing::taxes(cx),
            catalog::catalog(cx),
            self.draft(id).cloned(),
        ) else {
            return;
        };
        let reading = cx.background_executor().spawn(async move {
            let folder: Vec<PathBuf> = catalog
                .files(draft.product)
                .await
                .map(|files| {
                    files
                        .into_iter()
                        .map(|file| file.path)
                        .filter(|path| is_picture(path))
                        .collect()
                })
                .unwrap_or_default();
            let price = async {
                let assumptions = pricing::assumptions(&catalog, &taxes).await?;
                let offer = catalog
                    .cheapest_offer(draft.product)
                    .await
                    .map_err(|e| catalog::failure(&e))?;
                pricing
                    .draft_price(id, channel.as_ref(), assumptions, offer)
                    .await
                    .map_err(|e| price_failure(&e))
            }
            .await;
            (folder, price)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (folder, price) = reading.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(editor) = this.editor.as_mut().filter(|e| e.draft == id) {
                    editor.folder = folder;
                    editor.price_suggestion = Some(price);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self.outcome = None;
        cx.notify();
    }

    /// The draft as typed; `None` while the price or the units are not
    /// numbers.
    fn typed(&self, cx: &App) -> Option<(RecordId, DraftEdit)> {
        let editor = self.editor.as_ref()?;
        let draft = self.draft(editor.draft)?;
        let price = parse_amount(&editor.price.read(cx).value())
            .filter(|amount| !amount.is_sign_negative())?;
        let quantity = editor
            .quantity
            .read(cx)
            .value()
            .trim()
            .parse::<u32>()
            .ok()?;
        Some((
            editor.draft,
            DraftEdit {
                title: editor.title.read(cx).value().to_string(),
                description: editor.description.read(cx).value().to_string(),
                listing_type: editor.listing_type,
                condition: editor.condition,
                price: Money::new(price, draft.price.currency()),
                available_quantity: quantity,
                warranty: editor.warranty.read(cx).value().to_string(),
                attributes: editor
                    .attributes
                    .iter()
                    .map(|(id, field)| AttributeValue {
                        id: id.clone(),
                        value: field.read(cx).value().to_string(),
                    })
                    .collect(),
                pictures: editor.pictures.clone(),
            },
        ))
    }

    /// Saves what was typed, then runs `next` with the saved draft.
    fn save_then<T, F, Fut>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        next: F,
        then: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) where
        T: Send + 'static,
        F: FnOnce(Arc<Listings>, ListingDraft) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, String>> + Send,
    {
        let Some((id, edit)) = self.typed(cx) else {
            self.outcome = Some(Outcome::Failed(
                "Digite o preço como 89,90 e o estoque como um número inteiro.".into(),
            ));
            cx.notify();
            return;
        };
        self.outcome = None;
        if let Some(editor) = self.editor.as_mut() {
            editor.validated = None;
        }
        self.run(
            window,
            cx,
            move |listings, _| async move {
                let saved = listings
                    .save_draft(id, edit)
                    .await
                    .map_err(|e| failure(&e))?;
                next(listings, saved).await.map(Some)
            },
            then,
        );
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_then(
            window,
            cx,
            |_, saved| async move { Ok(saved) },
            |this, saved: ListingDraft, window, cx| {
                this.outcome = Some(Outcome::Done("Rascunho salvo.".into()));
                // The price depends on the Listing Type.
                this.read_folder_and_price(saved.id, window, cx);
            },
        );
    }

    /// Saves, uploads the pictures and asks Mercado Livre's validator;
    /// nothing is published.
    fn validate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(channel) = mercado_livre::adapter(cx) else {
            return;
        };
        self.save_then(
            window,
            cx,
            move |listings, saved| async move {
                listings
                    .validate_draft(saved.id, channel.as_ref())
                    .await
                    .map_err(|e| failure(&e))
            },
            |this, checklist: Vec<ChecklistItem>, _, _| {
                this.outcome = Some(if is_blocked(&checklist) {
                    Outcome::Failed("O checklist ainda tem itens que impedem publicar.".into())
                } else {
                    Outcome::Done(
                        "O Mercado Livre validou o anúncio; ele pode ser publicado.".into(),
                    )
                });
                if let Some(editor) = this.editor.as_mut() {
                    editor.validated = Some(checklist);
                }
            },
        );
    }

    /// Saves and publishes: the owner's click is the only way a draft
    /// reaches Mercado Livre.
    fn publish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(channel) = mercado_livre::adapter(cx) else {
            return;
        };
        self.save_then(
            window,
            cx,
            move |listings, saved| async move {
                listings
                    .publish(saved.id, channel.as_ref())
                    .await
                    .map(Ok)
                    .map_err(PublishFailure::from)
                    .or_else(|failure| match failure {
                        PublishFailure::Blocked(checklist) => Ok(Err(checklist)),
                        PublishFailure::Other(text) => Err(text),
                    })
            },
            |this, published: Result<Listing, Vec<ChecklistItem>>, _, cx| match published {
                Ok(listing) => {
                    this.editor = None;
                    this.outcome = Some(Outcome::Done(published_text(&listing).into()));
                    cx.emit(DraftPublished);
                }
                Err(checklist) => {
                    this.outcome = Some(Outcome::Failed(
                        "Ainda não dá para publicar: veja o checklist.".into(),
                    ));
                    if let Some(editor) = this.editor.as_mut() {
                        editor.validated = Some(checklist);
                    }
                }
            },
        );
    }

    /// Sends what is left of a draft already in Mercado Livre: its
    /// description.
    fn finish(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(channel) = mercado_livre::adapter(cx) else {
            return;
        };
        self.outcome = None;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                listings
                    .publish(id, channel.as_ref())
                    .await
                    .map(Some)
                    .map_err(|e| failure(&e))
            },
            |this, listing: Listing, _, cx| {
                this.outcome = Some(Outcome::Done(published_text(&listing).into()));
                cx.emit(DraftPublished);
            },
        );
    }

    fn discard(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.outcome = None;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                listings
                    .discard_draft(id)
                    .await
                    .map(Some)
                    .map_err(|e| failure(&e))
            },
            |this, (), _, _| {
                this.editor = None;
                this.outcome = Some(Outcome::Done("Rascunho descartado.".into()));
            },
        );
    }

    /// Asks Mercado Livre which categories fit the title as typed.
    fn predict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(channel), Some(editor)) = (mercado_livre::adapter(cx), self.editor.as_ref())
        else {
            return;
        };
        let title = editor.title.read(cx).value().to_string();
        self.outcome = None;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                listings
                    .predict_categories(&title, channel.as_ref())
                    .map(Some)
                    .map_err(|e| failure(&e))
            },
            |this, predictions: Vec<CategoryPrediction>, _, _| {
                if predictions.is_empty() {
                    this.outcome = Some(Outcome::Failed(
                        "O Mercado Livre não sugeriu categoria para esse título; detalhe mais o \
                         título e tente de novo."
                            .into(),
                    ));
                }
                if let Some(editor) = this.editor.as_mut() {
                    editor.predictions = Some(predictions);
                }
            },
        );
    }

    /// Saves, then moves the draft to `prediction`'s category with its
    /// attributes; the editor opens again on the new fields.
    fn choose(
        &mut self,
        prediction: CategoryPrediction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(channel) = mercado_livre::adapter(cx) else {
            return;
        };
        self.save_then(
            window,
            cx,
            move |listings, saved| async move {
                listings
                    .choose_category(
                        saved.id,
                        prediction.category,
                        &prediction.attributes,
                        channel.as_ref(),
                    )
                    .await
                    .map_err(|e| failure(&e))
            },
            |this, moved: ListingDraft, window, cx| {
                this.outcome = Some(Outcome::Done(
                    format!(
                        "Categoria trocada para {}.",
                        moved
                            .category
                            .as_ref()
                            .map_or("—", |category| category.name.as_str())
                    )
                    .into(),
                ));
                this.open(moved.id, window, cx);
            },
        );
    }

    fn toggle_picture(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(editor) = self.editor.as_mut() {
            match editor.pictures.iter().position(|chosen| *chosen == path) {
                Some(at) => {
                    editor.pictures.remove(at);
                }
                None => editor.pictures.push(path),
            }
            editor.validated = None;
        }
        cx.notify();
    }

    fn use_suggested_price(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = &self.editor
            && let Some(Ok(priced)) = &editor.price_suggestion
        {
            let text = price_text(priced.suggested.price);
            editor
                .price
                .update(cx, |field, cx| field.set_value(text, window, cx));
        }
        cx.notify();
    }

    fn render_new(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .flex_wrap()
            .gap_2()
            .items_center()
            .child(
                div().w(px(320.)).child(
                    Select::new(&self.product_pick)
                        .search_placeholder("Buscar")
                        .small()
                        .placeholder("Produto do anúncio"),
                ),
            )
            .child(
                Button::new("new-draft")
                    .label("Novo rascunho")
                    .icon(IconName::Plus)
                    .outline()
                    .small()
                    .loading(self.busy && self.editor.is_none())
                    .disabled(self.busy || !self.connected(cx) || self.products.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| this.start_picked(window, cx))),
            )
            .into_any_element()
    }

    fn render_draft(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let draft = &self.drafts[index];
        let id = draft.id;
        let open = self.editor.as_ref().is_some_and(|e| e.draft == id);
        let checklist = draft.checklist();
        let missing = checklist.iter().filter(|item| item.blocks).count();
        let mut facts = vec![draft.seller_sku.clone()];
        facts.push(
            draft
                .category
                .as_ref()
                .map_or("sem categoria".to_owned(), |c| c.name.clone()),
        );
        facts.push(draft.listing_type.name().to_owned());
        if draft.price.amount() > rust_decimal::Decimal::ZERO {
            facts.push(draft.price.rounded().to_pt_br());
        }
        facts.push(format!("{} fotos", draft.pictures.len()));
        let state = match (&draft.published, missing) {
            (Some(_), _) => kit::tag("publicado, falta a descrição", t.danger, cx),
            (None, 0) => kit::tag("pronto para publicar", t.success, cx),
            (None, 1) => kit::tag("falta 1 item", t.danger, cx),
            (None, n) => kit::tag(format!("faltam {n} itens"), t.danger, cx),
        };
        let actions = match &draft.published {
            Some(_) => h_flex().child(
                Button::new(("finish-draft", index))
                    .label("Enviar a descrição")
                    .primary()
                    .small()
                    .loading(self.busy)
                    .disabled(self.busy || !self.connected(cx))
                    .on_click(cx.listener(move |this, _, window, cx| this.finish(id, window, cx))),
            ),
            None if open => h_flex(),
            None => h_flex().gap_2().child(
                Button::new(("edit-draft", index))
                    .label("Editar")
                    .outline()
                    .small()
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.outcome = None;
                        this.open(id, window, cx)
                    })),
            ),
        };
        v_flex()
            .gap_3()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if open { t.accent_edge } else { t.frame })
            .bg(t.surface)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .font_medium()
                                            .truncate()
                                            .child(draft.title.clone()),
                                    )
                                    .child(state),
                            )
                            .child(div().text_xs().text_color(t.text2).child(facts.join(" · "))),
                    )
                    .child(actions),
            )
            .children(
                self.editor
                    .as_ref()
                    .filter(|editor| editor.draft == id)
                    .map(|editor| self.render_editor(editor, draft, cx)),
            )
            .into_any_element()
    }

    fn render_editor(
        &self,
        editor: &Editor,
        draft: &ListingDraft,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let label = |text: &'static str| div().text_xs().font_medium().child(text);
        let field = |text: &'static str, width: f32, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .w(px(width))
                .child(label(text))
                .child(Input::new(input).small())
        };
        let title_length = editor.title.read(cx).value().chars().count();
        let choice = |id: &'static str, text: &'static str, on: bool| {
            let button = Button::new(id).label(text).small();
            if on {
                button.primary()
            } else {
                button.outline()
            }
        };

        let category = v_flex()
            .gap_1()
            .child(label("Categoria"))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .flex_wrap()
                    .child(div().text_sm().child(match &draft.category {
                        Some(category) => format!("{} ({})", category.name, category.id),
                        None => "Nenhuma ainda".to_owned(),
                    }))
                    .child(
                        Button::new("predict-category")
                            .label("Sugerir pelo título")
                            .ghost()
                            .small()
                            .disabled(self.busy || !self.connected(cx))
                            .on_click(cx.listener(|this, _, window, cx| this.predict(window, cx))),
                    ),
            )
            .children(editor.predictions.as_ref().map(|predictions| {
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .children(predictions.iter().enumerate().map(|(index, prediction)| {
                        let chosen = prediction.clone();
                        Button::new(("prediction", index))
                            .label(prediction.category.name.clone())
                            .outline()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.choose(chosen.clone(), window, cx)
                            }))
                    }))
            }));

        let kind =
            h_flex()
                .gap_4()
                .flex_wrap()
                .child(
                    v_flex().gap_1().child(label("Tipo")).child(
                        h_flex()
                            .gap_1()
                            .child(
                                choice(
                                    "type-classic",
                                    "Clássico",
                                    editor.listing_type == ListingType::Classic,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| this.set_type(ListingType::Classic, cx),
                                )),
                            )
                            .child(
                                choice(
                                    "type-premium",
                                    "Premium",
                                    editor.listing_type == ListingType::Premium,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| this.set_type(ListingType::Premium, cx),
                                )),
                            ),
                    ),
                )
                .child(
                    v_flex().gap_1().child(label("Condição")).child(
                        h_flex()
                            .gap_1()
                            .child(
                                choice("condition-new", "Novo", editor.condition == Condition::New)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_condition(Condition::New, cx)
                                    })),
                            )
                            .child(
                                choice(
                                    "condition-used",
                                    "Usado",
                                    editor.condition == Condition::Used,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| this.set_condition(Condition::Used, cx),
                                )),
                            ),
                    ),
                );

        let suggestion = match &editor.price_suggestion {
            None => Some(
                div()
                    .text_xs()
                    .text_color(t.text2)
                    .child("Calculando o preço sugerido…")
                    .into_any_element(),
            ),
            Some(Ok(priced)) => Some(
                h_flex()
                    .gap_2()
                    .items_center()
                    .flex_wrap()
                    .child(div().text_xs().text_color(t.text2).child(format!(
                        "Sugerido: {} para a margem alvo de {} ({}), custo {} {}.",
                        priced.suggested.price.to_pt_br(),
                        priced.target.margin.to_pt_br(),
                        if priced.target.own {
                            "do produto"
                        } else {
                            "padrão"
                        },
                        priced.suggested.cost.to_pt_br(),
                        match priced.cost_from {
                            CostSource::AverageCost => "pelo custo médio do estoque",
                            CostSource::SupplierOffer => "pela oferta mais barata do fornecedor",
                        }
                    )))
                    .child(
                        Button::new("use-suggested-price")
                            .label("Usar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.use_suggested_price(window, cx)
                            })),
                    )
                    .into_any_element(),
            ),
            Some(Err(why)) => Some(
                div()
                    .text_xs()
                    .text_color(t.text2)
                    .child(format!("Sem preço sugerido. {why}"))
                    .into_any_element(),
            ),
        };

        let attributes =
            v_flex()
                .gap_1()
                .child(label("Atributos da categoria"))
                .when(draft.attributes.is_empty(), |list| {
                    list.child(
                        div()
                            .text_xs()
                            .text_color(t.text2)
                            .child("Escolha a categoria para ver os atributos dela."),
                    )
                })
                .child(h_flex().flex_wrap().gap_2().children(
                    draft.attributes.iter().zip(&editor.attributes).map(
                        |(attribute, (_, input))| {
                            v_flex()
                                .gap_1()
                                .w(px(220.))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        // Same height with or without a tag.
                                        .min_h(px(26.))
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_medium()
                                                .truncate()
                                                .child(attribute.name.clone()),
                                        )
                                        .children(match attribute.requirement {
                                            Requirement::Required => {
                                                Some(kit::tag("obrigatório", t.danger, cx))
                                            }
                                            Requirement::Recommended => {
                                                Some(kit::tag("recomendado", t.text2, cx))
                                            }
                                            Requirement::Optional => None,
                                        }),
                                )
                                .child(Input::new(input).small())
                        },
                    ),
                ));

        let pictures = v_flex()
            .gap_1()
            .child(label("Fotos da pasta do produto"))
            .child(div().text_xs().text_color(t.text2).child(format!(
                "Marque pelo menos {MIN_PICTURES}, em JPG ou PNG de até 10 MB; a primeira marcada é a capa."
            )))
            .when(editor.folder.is_empty(), |list| {
                list.child(div().text_xs().text_color(t.text2).child(
                    "Nenhuma foto na pasta do produto. Arraste fotos para a ficha dele em Produtos.",
                ))
            })
            .children(editor.folder.iter().enumerate().map(|(index, path)| {
                let position = editor.pictures.iter().position(|chosen| chosen == path);
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let text = match position {
                    Some(0) => format!("{name} (capa)"),
                    Some(at) => format!("{name} ({}ª)", at + 1),
                    None => name,
                };
                let this = cx.entity().downgrade();
                let path = path.clone();
                Checkbox::new(("picture", index))
                    .label(text)
                    .checked(position.is_some())
                    .on_click(move |_, _, cx| {
                        let _ = this.update(cx, |this, cx| this.toggle_picture(path.clone(), cx));
                    })
            }));

        let checklist = editor
            .validated
            .clone()
            .unwrap_or_else(|| draft.checklist());
        let checklist_card = v_flex()
            .gap_1()
            .child(label(if editor.validated.is_some() {
                "Checklist com o validador do Mercado Livre"
            } else {
                "Checklist (salve para atualizar)"
            }))
            .when(checklist.is_empty(), |list| {
                list.child(
                    div()
                        .text_sm()
                        .text_color(t.success)
                        .child("Nada falta. Valide no Mercado Livre ou publique."),
                )
            })
            .children(checklist.iter().map(|item| {
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(kit::tag(
                        if item.blocks { "falta" } else { "aviso" },
                        if item.blocks { t.danger } else { t.text2 },
                        cx,
                    ))
                    .child(div().text_sm().child(check_text(&item.check)))
            }));

        let can_publish = !self.busy && self.connected(cx);
        v_flex()
            .gap_3()
            .pt_3()
            .border_t(t.border_width)
            .border_color(t.frame)
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex().gap_2().child(label("Título")).child(
                            div()
                                .text_xs()
                                .text_color(t.text2)
                                .child(format!("{title_length} caracteres")),
                        ),
                    )
                    .child(Input::new(&editor.title).small()),
            )
            .child(category)
            .child(kind)
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(field("Preço (R$)", 120., &editor.price))
                    .child(field("Estoque", 100., &editor.quantity))
                    .child(field("Garantia do vendedor", 180., &editor.warranty)),
            )
            .children(suggestion)
            .child(attributes)
            .child(pictures)
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Descrição"))
                    .child(Textarea::new(&editor.description).small()),
            )
            .child(checklist_card)
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("publish-draft")
                            .label("Publicar no Mercado Livre")
                            .icon(IconName::Upload)
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(!can_publish || is_blocked(&checklist))
                            .on_click(cx.listener(|this, _, window, cx| this.publish(window, cx))),
                    )
                    .child(
                        Button::new("validate-draft")
                            .label("Validar no Mercado Livre")
                            .outline()
                            .small()
                            .disabled(!can_publish || draft.category.is_none())
                            .on_click(cx.listener(|this, _, window, cx| this.validate(window, cx))),
                    )
                    .child(
                        Button::new("save-draft")
                            .label("Salvar")
                            .outline()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .child(
                        Button::new("discard-draft")
                            .label("Descartar")
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click({
                                let id = draft.id;
                                cx.listener(move |this, _, window, cx| this.discard(id, window, cx))
                            }),
                    )
                    .child(
                        Button::new("close-draft")
                            .label("Fechar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            )
            .into_any_element()
    }

    fn set_type(&mut self, listing_type: ListingType, cx: &mut Context<Self>) {
        if let Some(editor) = self.editor.as_mut() {
            editor.listing_type = listing_type;
            editor.validated = None;
        }
        cx.notify();
    }

    fn set_condition(&mut self, condition: Condition, cx: &mut Context<Self>) {
        if let Some(editor) = self.editor.as_mut() {
            editor.condition = condition;
            editor.validated = None;
        }
        cx.notify();
    }
}

impl Render for DraftsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let heading = match self.drafts.len() {
            0 => "Rascunhos".to_owned(),
            count => format!("Rascunhos ({count})"),
        };
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .items_baseline()
                    .child(kit::section_heading(heading))
                    .child(div().text_xs().text_color(t.text2).child(
                        "anúncios novos montados a partir de um produto; nada vai ao Mercado \
                         Livre antes de você publicar",
                    )),
            )
            .child(self.render_new(cx))
            .when(self.editor.is_none(), |section| {
                section.children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            })
            .children((0..self.drafts.len()).map(|index| self.render_draft(index, cx)))
    }
}

/// What the owner typed, before any change: the draft's own values.
fn edit_of(draft: &ListingDraft) -> DraftEdit {
    DraftEdit {
        title: draft.title.clone(),
        description: draft.description.clone(),
        listing_type: draft.listing_type,
        condition: draft.condition,
        price: draft.price,
        available_quantity: draft.available_quantity,
        warranty: draft.warranty.clone(),
        attributes: Vec::new(),
        pictures: draft.pictures.iter().map(|p| p.path.clone()).collect(),
    }
}

/// "89,90"; empty for no price yet.
fn price_text(price: Money) -> String {
    if price.amount().is_zero() {
        String::new()
    } else {
        amount_text(price)
    }
}

fn published_text(listing: &Listing) -> String {
    format!(
        "Anúncio {} publicado no Mercado Livre e vinculado ao produto; ele já aparece na lista \
         abaixo.",
        listing.listed.id
    )
}

/// What a checklist item asks for, as the owner reads it.
fn check_text(check: &Check) -> String {
    match check {
        Check::Title => "Escreva o título.".into(),
        Check::Category => "Escolha a categoria: use Sugerir pelo título.".into(),
        Check::Price => "Digite o preço.".into(),
        Check::Attribute { name, .. } => format!("Preencha {name}."),
        Check::Pictures { selected } => format!(
            "Marque pelo menos {MIN_PICTURES} fotos; {} marcada{}.",
            selected,
            if *selected == 1 { "" } else { "s" }
        ),
        Check::Description => {
            "Sem descrição: o anúncio sai, mas uma descrição evita perguntas.".into()
        }
        Check::Stock => "Estoque zero: o anúncio nasce pausado.".into(),
        Check::Channel(message) => format!("Mercado Livre: {message}"),
    }
}

/// Why a draft action failed, as the owner reads it.
pub(crate) fn failure(error: &ListingError) -> String {
    match error {
        ListingError::Blocked(_) => "Ainda não dá para publicar: veja o checklist.".into(),
        ListingError::Published(_) => {
            "Esse rascunho já foi publicado; mude o anúncio pela lista de anúncios.".into()
        }
        ListingError::UnknownDraft(_) => {
            "Esse rascunho não existe mais; a lista foi atualizada.".into()
        }
        ListingError::Picture { path, reason } => format!(
            "A foto {} não pode ir ao Mercado Livre ({reason}). Desmarque-a ou troque o arquivo.",
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        ),
        ListingError::DescriptionNotSent(_, error) => format!(
            "O anúncio foi publicado, mas a descrição não foi: {} Use Enviar a descrição no \
             rascunho.",
            mercado_livre::failure(error)
        ),
        other => listings::failure(other),
    }
}

/// A publish refused by the checklist, which the editor shows, or any other
/// failure.
enum PublishFailure {
    Blocked(Vec<ChecklistItem>),
    Other(String),
}

impl From<ListingError> for PublishFailure {
    fn from(error: ListingError) -> Self {
        match error {
            ListingError::Blocked(checklist) => PublishFailure::Blocked(checklist),
            other => PublishFailure::Other(failure(&other)),
        }
    }
}

/// Why a draft has no suggested price, as the owner reads it.
fn price_failure(error: &PricingError) -> String {
    match error {
        PricingError::NoFeeBasis => "Escolha a categoria para o app saber a tarifa.".into(),
        PricingError::NoCost(_) => "O produto não tem custo médio nem oferta de fornecedor; \
             digite o preço."
            .into(),
        PricingError::Unreachable => "Nenhum preço chega à margem alvo: tarifa, imposto e \
             margem alvo somam 100% ou mais."
            .into(),
        PricingError::Unsettled => "A tarifa do Mercado Livre mudou a cada preço consultado; \
             digite o preço."
            .into(),
        PricingError::Platform(error) => mercado_livre::failure(error),
        PricingError::Listing(error) => failure(error),
        other => format!("Não consegui calcular: {other}"),
    }
}
