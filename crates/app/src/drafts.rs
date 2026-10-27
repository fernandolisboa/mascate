//! Rascunhos de anúncio (#16), on the Anúncios screen: a draft from a
//! Product with the category Mercado Livre predicts, the category's
//! attributes, pictures from the Product's folder and the suggested price;
//! the checklist of what is missing, the channel's validator, and
//! publishing, only on the owner's click. Title and description can be
//! written with AI (#17) into the fields, for the owner to review. On
//! Amazon a draft is an offer on a product its catalog already has, found by
//! the GTIN (#128): the owner confirms condition, price and units.

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
    AttributeValue, CatalogItem, CatalogMatch, CatalogProduct, CategoryPrediction, Check,
    ChecklistItem, Condition, CostSource, DraftEdit, DraftKind, DraftPrice, DraftStart,
    DraftVariation, Listing, ListingDraft, ListingError, ListingPublisher, Listings,
    MAX_VARIATIONS, MIN_PICTURES, PricingError, Requirement, SalesChannel, is_blocked, is_picture,
    takes_variations,
};
use mascate_kernel::{ListingType, Money, RecordId, SalesChannelKind, parse_amount};
use mascate_marketing::{BriefAttribute, CopyBrief, GeneratedCopy};

use crate::appearance::look;
use crate::catalog::{self, product_choice};
use crate::forms::{Outcome, Picker, amount_text, input, notice, picker, refill};
use crate::kit;
use crate::listing_copy;
use crate::listings;
use crate::mercado_livre;
use crate::preferences;
use crate::pricing;
use crate::sales_channels;
use crate::stock;
use crate::work_context;

/// A draft went out to its Sales Channel: the listings need reading again.
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
    channel: SalesChannelKind,
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
    /// The barcode a catalog offer looks its product up by.
    gtin: Entity<InputState>,
    /// The products of the channel's catalog with that barcode, when more
    /// than one came back, for the owner to pick.
    found: Option<Vec<CatalogItem>>,
    /// The checklist with the channel's validator, until the next change.
    validated: Option<Vec<ChecklistItem>>,
    /// The last title and description written with AI, as it came.
    written: Option<GeneratedCopy>,
    /// How the last writing with AI went, shown by its button.
    writing: Option<Outcome>,
    /// What sets the variations apart, as "Cor".
    variation_tier: Entity<InputState>,
    /// One per variation, the draft's own Product first; none for a
    /// listing without variations.
    variations: Vec<VariationRow>,
    /// The Product the next variation is of.
    variation_pick: Picker,
}

/// A variation as typed: a Product with its option and its units.
struct VariationRow {
    product: RecordId,
    seller_sku: String,
    name: String,
    option: Entity<InputState>,
    units: Entity<InputState>,
}

pub struct DraftsSection {
    /// Where a new draft goes.
    channel: SalesChannelKind,
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
            channel: preferences::work_context(cx)
                .sales_channel()
                .unwrap_or(SalesChannelKind::MercadoLivre),
            drafts: Vec::new(),
            products: Vec::new(),
            product_pick: picker(window, cx),
            editor: None,
            busy: false,
            reads: 0,
            outcome: None,
        }
    }

    /// Follows a new Work Context: a new draft goes to its channel, and an
    /// editor open on another channel closes without sending; the draft
    /// stays saved.
    pub fn follow_context(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let context = preferences::work_context(cx);
        if let Some(kind) = context.sales_channel() {
            self.channel = kind;
        }
        work_context::close_outside(&mut self.editor, context, |editor| Some(editor.channel));
        cx.notify();
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

    fn draft(&self, id: RecordId) -> Option<&ListingDraft> {
        self.drafts.iter().find(|draft| draft.id == id)
    }

    /// Where the open draft goes, with its channel.
    fn editor_ports(&self, cx: &App) -> Option<(RecordId, SalesChannelKind, Ports)> {
        let editor = self.editor.as_ref()?;
        Some((editor.draft, editor.channel, ports(cx, editor.channel)?))
    }

    /// A new draft of `product`, with its units on hand, its folder's
    /// pictures and the suggested price; it opens for editing.
    pub fn start(&mut self, product: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let kind = self.channel;
        let (Some((publisher, channel)), Some(pricing), Some(taxes), Some(inventory)) = (
            ports(cx, kind),
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
                        publisher.as_ref(),
                    )
                    .await
                    .map_err(|e| failure(kind, &e))?;
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
                        .map_err(|e| failure(kind, &e))?;
                }
                Ok(Some(draft.id))
            },
            move |this, draft: RecordId, window, cx| {
                this.outcome = Some(Outcome::Done(
                    match DraftKind::on(kind) {
                        DraftKind::OwnPage => format!(
                            "Rascunho criado. Revise o que o checklist pede e publique quando \
                             quiser; nada vai {} antes disso.",
                            sales_channels::to(kind)
                        ),
                        DraftKind::CatalogOffer => format!(
                            "Rascunho criado. Busque pelo GTIN o produto {} que você vai \
                             oferecer; nada vai {} antes de você publicar.",
                            sales_channels::of_channel(kind),
                            sales_channels::to(kind)
                        ),
                    }
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
        let gtin = input("Ex.: 7891234567895", window, cx);
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
        let variation_tier = input("Ex.: Cor", window, cx);
        variation_tier.update(cx, |field, cx| {
            field.set_value(draft.variation_tier.clone(), window, cx)
        });
        let variations = draft
            .variations
            .iter()
            .map(|variation| {
                let name = self
                    .products
                    .iter()
                    .find(|product| product.id == variation.product)
                    .map_or_else(|| variation.seller_sku.clone(), |p| p.name.clone());
                variation_row(variation, name, window, cx)
            })
            .collect();
        let variation_pick = picker(window, cx);
        refill(
            &variation_pick,
            self.products.iter().map(product_choice).collect(),
            window,
            cx,
        );
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
            channel: draft.channel,
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
            gtin,
            found: None,
            validated: None,
            written: None,
            writing: None,
            variation_tier,
            variations,
            variation_pick,
        });
        self.read_folder_and_price(id, window, cx);
        cx.notify();
    }

    /// The Product's pictures and the suggested price at the draft's
    /// category and type, off the UI thread.
    fn read_folder_and_price(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.draft(id).cloned() else {
            return;
        };
        let kind = draft.channel;
        let (Some((_, channel)), Some(pricing), Some(taxes), Some(catalog)) = (
            ports(cx, kind),
            pricing::pricing(cx),
            pricing::taxes(cx),
            catalog::catalog(cx),
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
                    .map_err(|e| price_failure(&e, kind))
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
        let typed_price = editor.price.read(cx).value();
        // No price yet is zero, which the checklist asks for.
        let price = if typed_price.trim().is_empty() {
            rust_decimal::Decimal::ZERO
        } else {
            parse_amount(&typed_price).filter(|amount| !amount.is_sign_negative())?
        };
        let quantity = editor
            .quantity
            .read(cx)
            .value()
            .trim()
            .parse::<u32>()
            .ok()?;
        let variations = editor
            .variations
            .iter()
            .map(|row| {
                Some(DraftVariation {
                    product: row.product,
                    seller_sku: row.seller_sku.clone(),
                    option: row.option.read(cx).value().trim().to_owned(),
                    available_quantity: row.units.read(cx).value().trim().parse().ok()?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
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
                variation_tier: editor.variation_tier.read(cx).value().trim().to_owned(),
                variations,
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
        let Some(kind) = self.editor.as_ref().map(|editor| editor.channel) else {
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
                    .map_err(|e| failure(kind, &e))?;
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

    /// Saves, uploads the pictures and asks the channel's validator;
    /// nothing is published.
    fn validate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, kind, (publisher, _))) = self.editor_ports(cx) else {
            return;
        };
        self.save_then(
            window,
            cx,
            move |listings, saved| async move {
                listings
                    .validate_draft(saved.id, publisher.as_ref())
                    .await
                    .map_err(|e| failure(kind, &e))
            },
            move |this, checklist: Vec<ChecklistItem>, _, _| {
                this.outcome = Some(if is_blocked(&checklist) {
                    Outcome::Failed("O checklist ainda tem itens que impedem publicar.".into())
                } else {
                    Outcome::Done(
                        format!(
                            "{} validou o anúncio; ele pode ser publicado.",
                            sales_channels::opening(kind)
                        )
                        .into(),
                    )
                });
                if let Some(editor) = this.editor.as_mut() {
                    editor.validated = Some(checklist);
                }
            },
        );
    }

    /// Saves and publishes: the owner's click is the only way a draft
    /// reaches its channel.
    fn publish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, kind, (publisher, _))) = self.editor_ports(cx) else {
            return;
        };
        self.save_then(
            window,
            cx,
            move |listings, saved| async move {
                listings
                    .publish(saved.id, publisher.as_ref())
                    .await
                    .map(Ok)
                    .map_err(|e| PublishFailure::of(kind, e))
                    .or_else(|failure| match failure {
                        PublishFailure::Blocked(checklist) => Ok(Err(checklist)),
                        PublishFailure::Other(text) => Err(text),
                    })
            },
            move |this, published: Result<Listing, Vec<ChecklistItem>>, _, cx| match published {
                Ok(listing) => {
                    this.editor = None;
                    this.outcome = Some(Outcome::Done(published_text(&listing, kind).into()));
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

    /// Sends what is left of a draft already in its channel: its variations,
    /// then its description.
    fn finish(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.draft(id).map(|draft| draft.channel) else {
            return;
        };
        let Some((publisher, _)) = ports(cx, kind) else {
            return;
        };
        self.outcome = None;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                listings
                    .publish(id, publisher.as_ref())
                    .await
                    .map(Some)
                    .map_err(|e| failure(kind, &e))
            },
            move |this, listing: Listing, _, cx| {
                this.outcome = Some(Outcome::Done(published_text(&listing, kind).into()));
                cx.emit(DraftPublished);
            },
        );
    }

    fn discard(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.draft(id).map(|draft| draft.channel) else {
            return;
        };
        self.outcome = None;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                listings
                    .discard_draft(id)
                    .await
                    .map(Some)
                    .map_err(|e| failure(kind, &e))
            },
            |this, (), _, _| {
                this.editor = None;
                this.outcome = Some(Outcome::Done("Rascunho descartado.".into()));
            },
        );
    }

    /// Asks the draft's channel which categories fit the title as typed.
    fn predict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some((_, kind, (publisher, _))), Some(editor)) =
            (self.editor_ports(cx), self.editor.as_ref())
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
                    .predict_categories(&title, publisher.as_ref())
                    .map(Some)
                    .map_err(|e| failure(kind, &e))
            },
            move |this, predictions: Vec<CategoryPrediction>, _, _| {
                if predictions.is_empty() {
                    this.outcome = Some(Outcome::Failed(
                        format!(
                            "{} não sugeriu categoria para esse título; detalhe mais o título e \
                             tente de novo.",
                            sales_channels::opening(kind)
                        )
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
        let Some((_, kind, (publisher, _))) = self.editor_ports(cx) else {
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
                        publisher.as_ref(),
                    )
                    .await
                    .map_err(|e| failure(kind, &e))
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

    /// Saves, then looks the GTIN typed up in the channel's catalog: one
    /// product and the draft offers it; several and the owner picks one;
    /// none and nothing changes.
    fn find_catalog_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some((_, kind, (publisher, _))), Some(editor)) =
            (self.editor_ports(cx), self.editor.as_ref())
        else {
            return;
        };
        let gtin = editor.gtin.read(cx).value().to_string();
        if let Some(editor) = self.editor.as_mut() {
            editor.found = None;
        }
        self.save_then(
            window,
            cx,
            move |listings, saved| async move {
                listings
                    .match_catalog_item(saved.id, &gtin, publisher.as_ref())
                    .await
                    .map_err(|e| failure(kind, &e))
            },
            move |this, matched: CatalogMatch, window, cx| match matched {
                CatalogMatch::NotFound => {
                    this.outcome = Some(Outcome::Failed(
                        format!(
                            "{} não tem produto com esse GTIN. Confira os números na embalagem; \
                             cadastrar um produto novo {} fica fora do app, pelo Seller Central.",
                            sales_channels::opening(kind),
                            sales_channels::on(kind)
                        )
                        .into(),
                    ));
                }
                CatalogMatch::Offered(draft) => {
                    this.outcome = Some(Outcome::Done(offered_text(&draft, kind).into()));
                    this.open(draft.id, window, cx);
                }
                CatalogMatch::Several(items) => {
                    this.outcome = Some(Outcome::Done(
                        format!(
                            "{} tem {} produtos com esse GTIN; escolha o que você vende.",
                            sales_channels::opening(kind),
                            items.len()
                        )
                        .into(),
                    ));
                    if let Some(editor) = this.editor.as_mut() {
                        editor.found = Some(items);
                    }
                }
            },
        );
    }

    /// Saves, then makes the draft an offer on `item`.
    fn offer(&mut self, item: CatalogItem, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, kind, (publisher, _))) = self.editor_ports(cx) else {
            return;
        };
        self.save_then(
            window,
            cx,
            move |listings, saved| async move {
                listings
                    .offer_catalog_item(saved.id, item, publisher.as_ref())
                    .await
                    .map_err(|e| failure(kind, &e))
            },
            move |this, offered: ListingDraft, window, cx| {
                this.outcome = Some(Outcome::Done(offered_text(&offered, kind).into()));
                this.open(offered.id, window, cx);
            },
        );
    }

    /// Writes a title and a description with AI from the Product, its
    /// Supplier Offers and the category's attributes as typed, and puts them
    /// in the fields. Nothing is saved: the owner reviews first.
    fn write_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(channel), Some((copy, writer)), Some(editor)) = (
            mercado_livre::adapter(cx),
            listing_copy::listing_copy(cx),
            self.editor.as_ref(),
        ) else {
            return;
        };
        let Some(draft) = self.draft(editor.draft) else {
            return;
        };
        let Some(category) = draft.category.clone() else {
            return;
        };
        let attributes: Vec<BriefAttribute> = draft
            .attributes
            .iter()
            .zip(&editor.attributes)
            .map(|(attribute, (_, field))| BriefAttribute {
                name: attribute.name.clone(),
                value: field.read(cx).value().to_string(),
            })
            .collect();
        let product = draft.product;
        let condition = editor.condition.name().to_owned();
        self.outcome = None;
        if let Some(editor) = self.editor.as_mut() {
            editor.writing = None;
        }
        self.run(
            window,
            cx,
            move |_, catalog| async move {
                let name = catalog
                    .product(product)
                    .await
                    .map_err(|e| catalog::failure(&e))?
                    .name;
                let offers = catalog
                    .product_offers(product)
                    .await
                    .map_err(|e| catalog::failure(&e))?
                    .into_iter()
                    .map(|history| history.latest.title)
                    .collect();
                let brief = CopyBrief {
                    product: name,
                    offers,
                    category: category.id,
                    category_name: category.name,
                    condition,
                    attributes,
                };
                let written = copy.write(channel.as_ref(), writer.as_ref(), &brief).await;
                Ok(Some(written.map_err(|e| listing_copy::failure(&e))))
            },
            |this, written: Result<GeneratedCopy, String>, window, cx| {
                let Some(editor) = this.editor.as_mut() else {
                    return;
                };
                editor.writing = Some(match written {
                    Ok(written) => {
                        editor.title.update(cx, |field, cx| {
                            field.set_value(written.title.clone(), window, cx)
                        });
                        editor.description.update(cx, |field, cx| {
                            field.set_value(written.description.clone(), window, cx)
                        });
                        editor.validated = None;
                        editor.written = Some(written);
                        Outcome::Done(
                            "Título e descrição escritos com IA. Revise e edite antes de salvar; \
                             nada vai ao Mercado Livre sem o seu clique em publicar."
                                .into(),
                        )
                    }
                    Err(error) => Outcome::Failed(error.into()),
                });
            },
        );
    }

    /// Adds the Product picked as a variation, with its units on hand; the
    /// first one brings in the draft's own Product ahead of it, with the
    /// units typed.
    fn add_variation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        let Some(picked) = editor.variation_pick.read(cx).selected_value().copied() else {
            self.outcome = Some(Outcome::Failed("Escolha o produto da variação.".into()));
            cx.notify();
            return;
        };
        if editor.variations.iter().any(|row| row.product == picked) {
            self.outcome = Some(Outcome::Failed(
                "Esse produto já é uma variação do rascunho.".into(),
            ));
            cx.notify();
            return;
        }
        let (Some(draft), Some(inventory)) = (self.draft(editor.draft), stock::inventory(cx))
        else {
            return;
        };
        let own = draft.product;
        let id = editor.draft;
        let own_units = editor.quantity.read(cx).value().trim().parse().unwrap_or(0);
        let variation = |product: RecordId, available_quantity: u32| {
            self.products
                .iter()
                .find(|found| found.id == product)
                .map(|found| {
                    (
                        DraftVariation {
                            product,
                            seller_sku: found.sku.to_string(),
                            option: String::new(),
                            available_quantity,
                        },
                        found.name.clone(),
                    )
                })
        };
        let mut added: Vec<_> = editor
            .variations
            .is_empty()
            .then(|| variation(own, own_units))
            .flatten()
            .into_iter()
            .collect();
        let Some(picked_variation) = (picked != own).then(|| variation(picked, 0)).flatten() else {
            self.add_rows(id, added, window, cx);
            return;
        };
        self.outcome = None;
        let reading = cx.background_executor().spawn(async move {
            inventory
                .stock()
                .await
                .map_err(|e| stock::inventory_failure(&e))
                .map(|stock| {
                    stock
                        .products
                        .into_iter()
                        .find(|stock| stock.product == picked)
                        .map_or(0, |stock| stock.valuation.quantity())
                })
        });
        cx.spawn_in(window, async move |this, cx| {
            let on_hand = reading.await;
            let _ = this.update_in(cx, |this, window, cx| match on_hand {
                Ok(on_hand) => {
                    let (mut picked, name) = picked_variation;
                    picked.available_quantity = u32::try_from(on_hand.max(0)).unwrap_or(u32::MAX);
                    added.push((picked, name));
                    this.add_rows(id, added, window, cx);
                }
                Err(error) => {
                    this.outcome = Some(Outcome::Failed(error.into()));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn add_rows(
        &mut self,
        id: RecordId,
        added: Vec<(DraftVariation, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.editor.as_mut().filter(|editor| editor.draft == id) else {
            return;
        };
        for (variation, name) in added {
            let row = variation_row(&variation, name, window, cx);
            editor.variations.push(row);
        }
        editor.validated = None;
        cx.notify();
    }

    /// Takes a variation out; with only the draft's own Product left, the
    /// listing goes back to having none, with that Product's units.
    fn remove_variation(&mut self, product: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        editor.variations.retain(|row| row.product != product);
        if let [own] = editor.variations.as_slice() {
            let units = own.units.read(cx).value().to_string();
            editor
                .quantity
                .update(cx, |field, cx| field.set_value(units, window, cx));
            editor.variations.clear();
        }
        editor.validated = None;
        cx.notify();
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
        let t = look(cx).tokens;
        // A Platform's context fixes where a new draft goes.
        let channels = match preferences::work_context(cx).sales_channel() {
            None => publishing_channels(cx),
            Some(kind) if publishing_channels(cx).contains(&kind) => Vec::new(),
            Some(kind) => {
                return div()
                    .text_sm()
                    .text_color(t.text2)
                    .child(format!(
                        "O app ainda não publica anúncios novos {}: crie no painel do canal e \
                         sincronize para trazê-los.",
                        sales_channels::on(kind)
                    ))
                    .into_any_element();
            }
        };
        h_flex()
            .flex_wrap()
            .gap_2()
            .items_center()
            .when(channels.len() > 1, |row| {
                row.child(
                    h_flex()
                        .gap_1()
                        .children(channels.into_iter().enumerate().map(|(index, kind)| {
                            let button = Button::new(("draft-channel", index))
                                .label(kind.name())
                                .small()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.channel = kind;
                                    cx.notify();
                                }));
                            if kind == self.channel {
                                button.primary()
                            } else {
                                button.outline()
                            }
                        })),
                )
            })
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
                    .disabled(
                        self.busy
                            || !sales_channels::is_connected(cx, self.channel)
                            || self.products.is_empty(),
                    )
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
        let connected = sales_channels::is_connected(cx, draft.channel);
        let mut facts = vec![draft.channel.name().to_owned(), draft.seller_sku.clone()];
        match draft.kind() {
            DraftKind::OwnPage => {
                facts.push(
                    draft
                        .category
                        .as_ref()
                        .map_or("sem categoria".to_owned(), |c| c.name.clone()),
                );
                if takes_listing_type(draft.channel) {
                    facts.push(draft.listing_type.name().to_owned());
                }
                if !draft.variations.is_empty() {
                    facts.push(format!("{} variações", draft.variations.len()));
                }
            }
            DraftKind::CatalogOffer => facts.push(match catalog_id(draft) {
                Some(id) => format!("ASIN {id}"),
                None => "sem produto escolhido".to_owned(),
            }),
        }
        if draft.price.amount() > rust_decimal::Decimal::ZERO {
            facts.push(draft.price.rounded().to_pt_br());
        }
        if draft.kind() == DraftKind::OwnPage {
            facts.push(format!("{} fotos", draft.pictures.len()));
        }
        let state = match (&draft.published, missing) {
            (Some(_), _) if draft.variations_pending => {
                kit::tag("publicado, faltam as variações", t.danger, cx)
            }
            (Some(_), _) => kit::tag("publicado, falta a descrição", t.danger, cx),
            (None, 0) => kit::tag("pronto para publicar", t.success, cx),
            (None, 1) => kit::tag("falta 1 item", t.danger, cx),
            (None, n) => kit::tag(format!("faltam {n} itens"), t.danger, cx),
        };
        let actions = match &draft.published {
            Some(_) => h_flex().child(
                Button::new(("finish-draft", index))
                    .label(if draft.variations_pending {
                        "Enviar as variações"
                    } else {
                        "Enviar a descrição"
                    })
                    .primary()
                    .small()
                    .loading(self.busy)
                    .disabled(self.busy || !connected)
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
        let of_channel = sales_channels::of_channel(draft.channel);
        let on_channel = sales_channels::on(draft.channel);
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
                            .disabled(self.busy || !sales_channels::is_connected(cx, draft.channel))
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

        let listing_type = v_flex().gap_1().child(label("Tipo")).child(
            h_flex()
                .gap_1()
                .child(
                    choice(
                        "type-classic",
                        "Clássico",
                        editor.listing_type == ListingType::Classic,
                    )
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_type(ListingType::Classic, cx)),
                    ),
                )
                .child(
                    choice(
                        "type-premium",
                        "Premium",
                        editor.listing_type == ListingType::Premium,
                    )
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_type(ListingType::Premium, cx)),
                    ),
                ),
        );
        let condition = v_flex().gap_1().child(label("Condição")).child(
            h_flex()
                .gap_1()
                .child(
                    choice("condition-new", "Novo", editor.condition == Condition::New).on_click(
                        cx.listener(|this, _, _, cx| this.set_condition(Condition::New, cx)),
                    ),
                )
                .child(
                    choice(
                        "condition-used",
                        "Usado",
                        editor.condition == Condition::Used,
                    )
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_condition(Condition::Used, cx)),
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
            .child(
                div()
                    .text_xs()
                    .font_medium()
                    .child(if editor.validated.is_some() {
                        format!("Checklist com o validador {of_channel}")
                    } else {
                        "Checklist (salve para atualizar)".to_owned()
                    }),
            )
            .when(checklist.is_empty(), |list| {
                list.child(
                    div()
                        .text_sm()
                        .text_color(t.success)
                        .child(format!("Nada falta. Valide {on_channel} ou publique.")),
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
                    .child(
                        div()
                            .text_sm()
                            .child(check_text(&item.check, draft.channel)),
                    )
            }));

        let can_publish = !self.busy && sales_channels::is_connected(cx, draft.channel);
        let actions = h_flex()
            .gap_2()
            .flex_wrap()
            .child(
                Button::new("publish-draft")
                    .label(format!("Publicar {on_channel}"))
                    .icon(IconName::Upload)
                    .primary()
                    .small()
                    .loading(self.busy)
                    .disabled(!can_publish || is_blocked(&checklist))
                    .on_click(cx.listener(|this, _, window, cx| this.publish(window, cx))),
            )
            .child(
                Button::new("validate-draft")
                    .label(format!("Validar {on_channel}"))
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
            );
        let prices = h_flex()
            .gap_2()
            .flex_wrap()
            .child(field("Preço (R$)", 120., &editor.price))
            .when(editor.variations.is_empty(), |prices| {
                prices.child(field("Estoque", 100., &editor.quantity))
            });
        if draft.kind() == DraftKind::CatalogOffer {
            return v_flex()
                .gap_3()
                .pt_3()
                .border_t(t.border_width)
                .border_color(t.frame)
                .child(self.render_catalog(editor, draft, cx))
                .child(condition)
                .child(prices)
                .children(suggestion)
                .child(checklist_card)
                .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
                .child(actions)
                .into_any_element();
        }
        let title_count = match &editor.written {
            Some(written) => format!(
                "{title_length} de {} caracteres",
                written.rules.max_title_chars
            ),
            None => format!("{title_length} caracteres"),
        };
        v_flex()
            .gap_3()
            .pt_3()
            .border_t(t.border_width)
            .border_color(t.frame)
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(label("Título"))
                            .child(div().text_xs().text_color(t.text2).child(title_count)),
                    )
                    .child(Input::new(&editor.title).small()),
            )
            .child(self.render_writing(editor, draft, cx))
            .child(category)
            .child(
                h_flex()
                    .gap_4()
                    .flex_wrap()
                    .when(takes_listing_type(draft.channel), |row| {
                        row.child(listing_type)
                    })
                    .child(condition),
            )
            .child(prices.when(takes_listing_type(draft.channel), |prices| {
                prices.child(field("Garantia do vendedor", 180., &editor.warranty))
            }))
            .children(suggestion)
            .when(takes_variations(draft.channel), |form| {
                form.child(self.render_variations(editor, draft, cx))
            })
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
            .child(actions)
            .into_any_element()
    }

    /// The product of the channel's catalog the offer joins, and the search
    /// by GTIN that finds it.
    fn render_catalog(
        &self,
        editor: &Editor,
        draft: &ListingDraft,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let kind = draft.channel;
        let chosen = match (catalog_id(draft), &draft.category) {
            (Some(id), Some(category)) => v_flex()
                .gap_0p5()
                .child(div().text_sm().font_medium().child(draft.title.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text2)
                        .child(format!("ASIN {id} · {}", category.name)),
                ),
            _ => v_flex().child(div().text_xs().text_color(t.text2).child(format!(
                "Digite o GTIN (o código de barras da embalagem) e busque o produto {}. Ele já \
                 tem fotos e descrição; você confirma a condição, o preço e o estoque.",
                sales_channels::of_channel(kind)
            ))),
        };
        let connected = sales_channels::is_connected(cx, kind);
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_medium()
                    .child(format!("Produto {}", sales_channels::of_channel(kind))),
            )
            .child(chosen)
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .flex_wrap()
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(220.))
                            .child(div().text_xs().font_medium().child("GTIN"))
                            .child(Input::new(&editor.gtin).small()),
                    )
                    .child(
                        Button::new("find-catalog-item")
                            .label(format!("Buscar {}", sales_channels::on(kind)))
                            .icon(IconName::Search)
                            .outline()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy || !connected)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.find_catalog_item(window, cx)
                            })),
                    ),
            )
            .children(editor.found.as_ref().map(|items| {
                v_flex()
                    .gap_2()
                    .children(items.iter().enumerate().map(|(index, item)| {
                        let picked = item.clone();
                        let mut facts =
                            vec![format!("ASIN {}", item.id), item.category.name.clone()];
                        if let Some(brand) = &item.brand {
                            facts.insert(0, brand.clone());
                        }
                        h_flex()
                            .gap_3()
                            .items_center()
                            .p_2()
                            .rounded(t.radius_lg)
                            .border(t.border_width)
                            .border_color(t.frame)
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .flex_1()
                                    .gap_0p5()
                                    .child(div().text_sm().truncate().child(item.title.clone()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(t.text2)
                                            .child(facts.join(" · ")),
                                    ),
                            )
                            .child(
                                Button::new(("offer-catalog-item", index))
                                    .label("Oferecer este")
                                    .outline()
                                    .small()
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.offer(picked.clone(), window, cx)
                                    })),
                            )
                    }))
            }))
            .into_any_element()
    }

    /// The Products sold as options of one listing, each with its option
    /// and units, and the picker that adds another.
    fn render_variations(
        &self,
        editor: &Editor,
        draft: &ListingDraft,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let heading =
            |text: &'static str, width: f32| div().w(px(width)).text_xs().font_medium().child(text);
        let rows = editor.variations.iter().enumerate().map(|(index, row)| {
            let product = row.product;
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    v_flex()
                        .w(px(260.))
                        .min_w_0()
                        .child(div().text_sm().truncate().child(row.name.clone()))
                        .child(div().text_xs().text_color(t.text2).child(
                            if product == draft.product {
                                format!("{} · produto do rascunho", row.seller_sku)
                            } else {
                                row.seller_sku.clone()
                            },
                        )),
                )
                .child(div().w(px(160.)).child(Input::new(&row.option).small()))
                .child(div().w(px(90.)).child(Input::new(&row.units).small()))
                .when(product != draft.product, |line| {
                    line.child(
                        Button::new(("remove-variation", index))
                            .label("Tirar")
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_variation(product, window, cx)
                            })),
                    )
                })
        });
        v_flex()
            .gap_2()
            .child(div().text_xs().font_medium().child("Variações"))
            .child(div().text_xs().text_color(t.text2).child(format!(
                "Opcional. Cada variação é um produto seu, com SKU e estoque próprios, e {} \
                 mostra todas num anúncio só, como cores ou tamanhos.",
                sales_channels::the(draft.channel)
            )))
            .when(!editor.variations.is_empty(), |list| {
                list.child(
                    v_flex()
                        .gap_1()
                        .w(px(220.))
                        .child(div().text_xs().font_medium().child("O que muda entre elas"))
                        .child(Input::new(&editor.variation_tier).small()),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(heading("Produto", 260.))
                        .child(heading("Opção", 160.))
                        .child(heading("Estoque", 90.)),
                )
                .children(rows)
            })
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div().w(px(320.)).child(
                            Select::new(&editor.variation_pick)
                                .search_placeholder("Buscar")
                                .small()
                                .placeholder("Produto da variação"),
                        ),
                    )
                    .child(
                        Button::new("add-variation")
                            .label("Adicionar variação")
                            .icon(IconName::Plus)
                            .outline()
                            .small()
                            .disabled(self.busy || editor.variations.len() >= MAX_VARIATIONS)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_variation(window, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// The button that writes title and description with AI, and what
    /// it needs or what the last text carried.
    fn render_writing(
        &self,
        editor: &Editor,
        draft: &ListingDraft,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let off = listing_copy::writing_off(
            &listing_copy::anthropic_state(cx),
            "gerar título e descrição",
        )
        .or_else(|| {
            (draft.channel != SalesChannelKind::MercadoLivre).then(|| {
                format!(
                    "Por enquanto a IA escreve só para o Mercado Livre, com o limite do título \
                         e os termos em alta das categorias dele; {} escreva o título e a \
                         descrição você mesmo.",
                    sales_channels::on(draft.channel)
                )
            })
        })
        .or_else(|| {
            (!sales_channels::is_connected(cx, SalesChannelKind::MercadoLivre)).then(|| {
                "Conecte a conta do Mercado Livre em Configurações › Conexões: o limite do \
                     título e os termos em alta vêm da categoria."
                    .to_owned()
            })
        })
        .or_else(|| {
            draft.category.is_none().then(|| {
                "Escolha a categoria antes de gerar: o limite do título e os termos em alta \
                     vêm dela."
                    .to_owned()
            })
        });
        let hint = match (&off, &editor.written) {
            (Some(off), _) => off.clone(),
            (None, Some(written)) => written_text(written),
            (None, None) => "A IA escreve com o nome do produto, os títulos das ofertas e a ficha \
                             técnica abaixo, e o texto entra nos campos para você revisar."
                .to_owned(),
        };
        v_flex()
            .gap_1()
            .child(
                h_flex().child(
                    Button::new("write-copy")
                        .label(if editor.written.is_some() {
                            "Gerar de novo com IA"
                        } else {
                            "Gerar título e descrição com IA"
                        })
                        .outline()
                        .small()
                        .loading(self.busy)
                        .disabled(self.busy || off.is_some())
                        .on_click(cx.listener(|this, _, window, cx| this.write_copy(window, cx))),
                ),
            )
            .child(div().text_xs().text_color(t.text2).child(hint))
            .children(editor.writing.as_ref().map(|outcome| notice(outcome, cx)))
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
        let context = preferences::work_context(cx);
        let shown: Vec<usize> = (0..self.drafts.len())
            .filter(|index| context.shows_channel(self.drafts[*index].channel))
            .collect();
        let heading = match shown.len() {
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
                        "anúncios novos montados a partir de um produto; nada vai ao canal \
                         antes de você publicar",
                    )),
            )
            .child(self.render_new(cx))
            .when(self.editor.is_none(), |section| {
                section.children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            })
            .children(shown.into_iter().map(|index| self.render_draft(index, cx)))
    }
}

/// Mercado Livre's Classic and Premium, with the seller's warranty; the
/// other channels have neither.
fn takes_listing_type(kind: SalesChannelKind) -> bool {
    kind == SalesChannelKind::MercadoLivre
}

fn variation_row(
    variation: &DraftVariation,
    name: String,
    window: &mut Window,
    cx: &mut Context<DraftsSection>,
) -> VariationRow {
    let option = input("Ex.: Azul", window, cx);
    option.update(cx, |field, cx| {
        field.set_value(variation.option.clone(), window, cx)
    });
    let units = input("0", window, cx);
    units.update(cx, |field, cx| {
        field.set_value(variation.available_quantity.to_string(), window, cx)
    });
    VariationRow {
        product: variation.product,
        seller_sku: variation.seller_sku.clone(),
        name,
        option,
        units,
    }
}

/// A channel's publisher and its Sales Channel, which prices the draft.
type Ports = (Arc<dyn ListingPublisher>, Arc<dyn SalesChannel>);

fn ports(cx: &App, kind: SalesChannelKind) -> Option<Ports> {
    let channel = sales_channels::of(cx, kind)?;
    Some((channel.publisher?, channel.listings))
}

/// The channels a draft can go to, in the app's order.
fn publishing_channels(cx: &App) -> Vec<SalesChannelKind> {
    cx.try_global::<sales_channels::SalesChannels>()
        .map(|channels| {
            channels
                .0
                .iter()
                .filter(|channel| channel.publisher.is_some())
                .map(|channel| channel.kind())
                .collect()
        })
        .unwrap_or_default()
}

/// The id, in the channel's catalog, of the product a catalog offer joins:
/// the value its product filled first.
fn catalog_id(draft: &ListingDraft) -> Option<&str> {
    draft
        .attributes
        .first()
        .map(|attribute| attribute.value.as_str())
        .filter(|id| !id.is_empty())
}

fn offered_text(draft: &ListingDraft, kind: SalesChannelKind) -> String {
    format!(
        "O rascunho oferece {} (ASIN {}) {}. Confirme a condição, o preço e o estoque e publique.",
        draft.title,
        catalog_id(draft).unwrap_or("—"),
        sales_channels::on(kind)
    )
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
        variation_tier: draft.variation_tier.clone(),
        variations: draft.variations.clone(),
    }
}

/// "89,90"; empty for no price yet.
/// What the last text written with AI carried, for the owner reviewing it.
fn written_text(written: &GeneratedCopy) -> String {
    let limit = format!(
        "Limite do título nesta categoria: {} caracteres.",
        written.rules.max_title_chars
    );
    let trends = if written.trends_in_title.is_empty() {
        if written.trends.is_empty() {
            "O Mercado Livre não informou termos em alta nesta categoria.".to_owned()
        } else {
            "Nenhum termo em alta da categoria descreve este produto no título.".to_owned()
        }
    } else {
        format!(
            "Termos em alta no título: {}.",
            written.trends_in_title.join(", ")
        )
    };
    let cut = if written.title_cut {
        " O título veio longo duas vezes e foi cortado na última palavra que cabe; confira."
    } else {
        ""
    };
    format!("{limit} {trends}{cut}")
}

fn price_text(price: Money) -> String {
    if price.amount().is_zero() {
        String::new()
    } else {
        amount_text(price)
    }
}

fn published_text(listing: &Listing, kind: SalesChannelKind) -> String {
    match DraftKind::on(kind) {
        DraftKind::OwnPage => format!(
            "Anúncio {} publicado {} e vinculado ao produto; ele já aparece na lista abaixo.",
            listing.listed.id,
            sales_channels::on(kind)
        ),
        DraftKind::CatalogOffer => format!(
            "Oferta {} enviada {} e vinculada ao produto. {} leva alguns minutos para pô-la à \
             venda; o próximo Sync mostra o status dela na lista abaixo.",
            listing.listed.id,
            sales_channels::to(kind),
            sales_channels::opening(kind)
        ),
    }
}

/// What a checklist item asks for, as the owner reads it.
fn check_text(check: &Check, kind: SalesChannelKind) -> String {
    match check {
        Check::Title => "Escreva o título.".into(),
        Check::Category => "Escolha a categoria: use Sugerir pelo título.".into(),
        Check::CatalogItem => format!(
            "Busque pelo GTIN o produto {} que o anúncio oferece.",
            sales_channels::of_channel(kind)
        ),
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
        Check::Stock => match DraftKind::on(kind) {
            DraftKind::OwnPage => "Estoque zero: o anúncio nasce pausado.".into(),
            DraftKind::CatalogOffer => "Estoque zero: a oferta nasce fora de venda.".into(),
        },
        Check::VariationTier => {
            "Dê nome ao que muda entre as variações, como Cor ou Tamanho.".into()
        }
        Check::VariationOption { seller_sku } => {
            format!("Escreva a opção da variação {seller_sku}, como Azul.")
        }
        Check::RepeatedOption(option) => {
            format!("Duas variações têm a opção {option}; cada uma precisa da sua.")
        }
        Check::TooManyVariations => format!("Use até {MAX_VARIATIONS} variações por anúncio."),
        Check::Channel(message) => format!("{}: {message}", kind.name()),
    }
}

/// Why an action on a draft for `kind` failed, as the owner reads it.
pub(crate) fn failure(kind: SalesChannelKind, error: &ListingError) -> String {
    match error {
        ListingError::Blocked(_) => "Ainda não dá para publicar: veja o checklist.".into(),
        ListingError::Published(_) => {
            "Esse rascunho já foi publicado; mude o anúncio pela lista de anúncios.".into()
        }
        ListingError::UnknownDraft(_) => {
            "Esse rascunho não existe mais; a lista foi atualizada.".into()
        }
        ListingError::Picture { path, reason } => format!(
            "A foto {} não pode ir {} ({reason}). Desmarque-a ou troque o arquivo.",
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            sales_channels::to(kind)
        ),
        ListingError::InvalidGtin => "Digite o GTIN só com os números do código de barras: 8, 12, \
             13 ou 14 dígitos, como na embalagem. O último dígito confere os outros; se ele não \
             bate, algum número foi digitado errado."
            .into(),
        ListingError::NotACatalogOffer(_) => {
            "Esse rascunho é um anúncio próprio; a busca pelo GTIN é para ofertas da Amazon.".into()
        }
        ListingError::DescriptionNotSent(_, error) => format!(
            "O anúncio foi publicado, mas a descrição não foi: {} Use Enviar a descrição no \
             rascunho.",
            sales_channels::failure(kind, error)
        ),
        ListingError::VariationsNotSent(_, error) => format!(
            "O anúncio foi publicado fora de venda, mas as variações não foram: {} Use Enviar as \
             variações no rascunho; ele só vai à venda com elas.",
            sales_channels::failure(kind, error)
        ),
        ListingError::NoVariations(channel) => format!(
            "{} não recebe variações pelo app; tire as variações do rascunho.",
            sales_channels::opening(*channel)
        ),
        ListingError::RepeatedVariation(_) => "Cada produto entra uma vez só nas variações.".into(),
        ListingError::VariationsWithoutTheDraftProduct => {
            "As variações precisam incluir o produto do rascunho.".into()
        }
        other => listings::channel_failure(kind, other),
    }
}

/// A publish refused by the checklist, which the editor shows, or any other
/// failure.
enum PublishFailure {
    Blocked(Vec<ChecklistItem>),
    Other(String),
}

impl PublishFailure {
    fn of(kind: SalesChannelKind, error: ListingError) -> Self {
        match error {
            ListingError::Blocked(checklist) => PublishFailure::Blocked(checklist),
            other => PublishFailure::Other(failure(kind, &other)),
        }
    }
}

/// Why a draft has no suggested price, as the owner reads it.
fn price_failure(error: &PricingError, kind: SalesChannelKind) -> String {
    match error {
        PricingError::NoFeeBasis => match DraftKind::on(kind) {
            DraftKind::OwnPage => "Escolha a categoria para o app saber a tarifa.".into(),
            DraftKind::CatalogOffer => format!(
                "Busque o produto {} pelo GTIN para o app saber a tarifa.",
                sales_channels::of_channel(kind)
            ),
        },
        PricingError::NoCost(_) => "O produto não tem custo médio nem oferta de fornecedor; \
             digite o preço."
            .into(),
        PricingError::Unreachable => "Nenhum preço chega à margem alvo: tarifa, imposto e \
             margem alvo somam 100% ou mais."
            .into(),
        PricingError::Unsettled => format!(
            "A tarifa {} mudou a cada preço consultado; digite o preço.",
            sales_channels::of_channel(kind)
        ),
        PricingError::Platform(error) => sales_channels::failure(kind, error),
        PricingError::Listing(error) => failure(kind, error),
        other => format!("Não consegui calcular: {other}"),
    }
}
