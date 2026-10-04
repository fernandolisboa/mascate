//! Anúncios (#15): the owner's Mercado Livre listings, synced, and the
//! queue of the ones still without a Product: confirm the suggested one,
//! pick another, or make a Product from the listing.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::select::Select;
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, EventEmitter, Global, Hsla, Window, div, px};
use mascate_catalog::{Catalog, Product};
use mascate_commerce::{
    CatalogProduct, Listing, ListingError, ListingStatus, ListingSync, ListingToLink, Listings,
    SuggestedBy,
};
use mascate_integrations::{Connection, ConnectionState};
use mascate_kernel::{RecordId, Timestamp};

use crate::appearance::{Tokens, look};
use crate::catalog::{self, NO_DATABASE, product_choice};
use crate::connections::AppConnections;
use crate::forms::{Outcome, Picker, input, notice, picker, refill};
use crate::kit;
use crate::layout;
use crate::mercado_livre;
use crate::offers::OpenProduct;
use crate::parts::ScreenParts;

/// The app's Listings; absent when the database did not open.
pub struct AppListings(pub Arc<Listings>);

impl Global for AppListings {}

fn listings(cx: &App) -> Option<Arc<Listings>> {
    cx.try_global::<AppListings>().map(|app| app.0.clone())
}

/// The step a listing to link is in, below it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    PickProduct,
    CreateProduct,
}

/// Everything the screen shows, read in one go off the UI thread.
struct Snapshot {
    listings: Vec<Listing>,
    to_link: Vec<ListingToLink>,
    products: Vec<Product>,
    last_sync: Option<Timestamp>,
    connection: ConnectionState,
}

pub struct ListingsScreen {
    listings: Vec<Listing>,
    to_link: Vec<ListingToLink>,
    products: Vec<Product>,
    last_sync: Option<Timestamp>,
    connection: Option<ConnectionState>,
    /// The listing to link whose row is open, and for what.
    step: Option<(RecordId, Step)>,
    product_pick: Picker,
    product_name: Entity<InputState>,
    product_sku: Entity<InputState>,
    busy: bool,
    syncing: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    /// The outcome of the last action on the whole screen.
    outcome: Option<Outcome>,
    /// The outcome of the last step on a row.
    row_outcome: Option<Outcome>,
}

impl EventEmitter<OpenProduct> for ListingsScreen {}

impl ListingsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut screen = Self {
            listings: Vec::new(),
            to_link: Vec::new(),
            products: Vec::new(),
            last_sync: None,
            connection: None,
            step: None,
            product_pick: picker(window, cx),
            product_name: input("Nome do produto", window, cx),
            product_sku: input("SKU", window, cx),
            busy: false,
            syncing: false,
            reads: 0,
            outcome: None,
            row_outcome: None,
        };
        screen.refresh(window, cx);
        screen
    }

    /// Reads everything again, as when the screen comes into view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx, |_, _| async { Ok(None) }, |_, _: (), _, _| {});
    }

    /// Runs `change` off the UI thread, then reads the screen's data again
    /// and hands what the change returned to `then`.
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
        let (Some(listings), Some(catalog)) = (listings(cx), catalog::catalog(cx)) else {
            return;
        };
        let connections = cx.global::<AppConnections>().0.clone();
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(listings.clone(), catalog.clone()).await;
            let read = async {
                let products = catalog.products().await.map_err(|e| catalog::failure(&e))?;
                let known: Vec<CatalogProduct> = products.iter().map(catalog_product).collect();
                Ok::<_, String>(Snapshot {
                    listings: listings.listings().await.map_err(|e| failure(&e))?,
                    to_link: listings.to_link(&known).await.map_err(|e| failure(&e))?,
                    last_sync: listings.last_sync().await.map_err(|e| failure(&e))?,
                    connection: connections.state(Connection::MercadoLivre),
                    products,
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
                    this.syncing = false;
                    match read_back {
                        Ok(snapshot) => this.show(snapshot, window, cx),
                        Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                    }
                }
                match changed {
                    Ok(Some(value)) => then(this, value, window, cx),
                    Ok(None) => {}
                    Err(error) => this.failed(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Shows a failure where the owner acted: the open row, or the screen.
    fn failed(&mut self, error: String) {
        let text = Outcome::Failed(error.into());
        if self.step.is_some() {
            self.row_outcome = Some(text);
        } else {
            self.outcome = Some(text);
        }
    }

    fn show(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        refill(
            &self.product_pick,
            snapshot.products.iter().map(product_choice).collect(),
            window,
            cx,
        );
        if self.step.is_some_and(|(open, _)| {
            !snapshot
                .to_link
                .iter()
                .any(|entry| entry.listing.id == open)
        }) {
            self.step = None;
        }
        self.listings = snapshot.listings;
        self.to_link = snapshot.to_link;
        self.products = snapshot.products;
        self.last_sync = snapshot.last_sync;
        self.connection = Some(snapshot.connection);
    }

    fn connected(&self) -> bool {
        self.connection == Some(ConnectionState::Connected)
    }

    fn product(&self, id: RecordId) -> Option<&Product> {
        self.products.iter().find(|product| product.id == id)
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(channel) = mercado_livre::adapter(cx) else {
            return;
        };
        self.outcome = None;
        self.step = None;
        self.syncing = true;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                listings
                    .sync(channel.as_ref())
                    .await
                    .map(Some)
                    .map_err(|e| failure(&e))
            },
            |this, report: ListingSync, _, _| {
                this.outcome = Some(Outcome::Done(synced(&report).into()));
            },
        );
    }

    fn open_step(
        &mut self,
        listing: RecordId,
        step: Step,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step = Some((listing, step));
        self.row_outcome = None;
        self.outcome = None;
        let Some(entry) = self
            .to_link
            .iter()
            .find(|entry| entry.listing.id == listing)
        else {
            return;
        };
        match step {
            Step::PickProduct => {
                let suggested = entry.suggestion.map(|suggestion| suggestion.product);
                self.product_pick.update(cx, |select, cx| match suggested {
                    Some(product) => select.set_selected_value(&product, window, cx),
                    None => select.set_selected_index(None, window, cx),
                });
            }
            Step::CreateProduct => {
                let listed = &entry.listing.listed;
                let name = match &listed.variation {
                    Some(variation) => format!("{} ({})", listed.title, variation.name),
                    None => listed.title.clone(),
                };
                let seller_sku = listed.seller_sku.clone();
                self.product_name
                    .update(cx, |input, cx| input.set_value(name.clone(), window, cx));
                self.product_sku.update(cx, |input, cx| {
                    input.set_value(seller_sku.clone().unwrap_or_default(), window, cx)
                });
                // The seller SKU already names the Product in the channel;
                // without one, the catalog suggests a free SKU.
                if seller_sku.is_none() {
                    self.run(
                        window,
                        cx,
                        move |_, catalog| async move {
                            catalog
                                .suggest_sku(&name)
                                .await
                                .map(Some)
                                .map_err(|e| catalog::failure(&e))
                        },
                        |this, sku: mascate_catalog::Sku, window, cx| {
                            this.product_sku.update(cx, |input, cx| {
                                input.set_value(sku.to_string(), window, cx)
                            });
                        },
                    );
                }
            }
        }
        cx.notify();
    }

    fn close_step(&mut self, cx: &mut Context<Self>) {
        self.step = None;
        self.row_outcome = None;
        cx.notify();
    }

    fn link(
        &mut self,
        listing: RecordId,
        product: RecordId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .product(product)
            .map(|product| format!("{} · {}", product.sku, product.name))
            .unwrap_or_default();
        self.row_outcome = None;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                listings
                    .link(listing, product)
                    .await
                    .map(Some)
                    .map_err(|e| failure(&e))
            },
            move |this, _: Listing, _, _| {
                this.step = None;
                this.outcome = Some(Outcome::Done(format!("Anúncio vinculado a {name}.").into()));
            },
        );
    }

    fn link_picked(&mut self, listing: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        match self.product_pick.read(cx).selected_value().copied() {
            Some(product) => self.link(listing, product, window, cx),
            None => {
                self.row_outcome = Some(Outcome::Failed("Escolha o produto.".into()));
                cx.notify();
            }
        }
    }

    /// A new Product, with its folder, linked to the listing it came from.
    fn create_product(&mut self, listing: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.product_name.read(cx).value().to_string();
        let sku = self.product_sku.read(cx).value().to_string();
        self.row_outcome = None;
        self.run(
            window,
            cx,
            move |listings, catalog| async move {
                let product = catalog
                    .add_product(&name, &sku)
                    .await
                    .map_err(|e| catalog::failure(&e))?;
                listings.link(listing, product.id).await.map_err(|e| {
                    format!(
                        "O produto {} foi criado, mas não consegui vinculá-lo ao anúncio: {}",
                        product.sku,
                        failure(&e)
                    )
                })?;
                Ok(Some(product))
            },
            |this, product: Product, _, _| {
                this.step = None;
                this.outcome = Some(Outcome::Done(
                    format!(
                        "Produto {} criado, com a pasta para os arquivos dele, e vinculado ao anúncio.",
                        product.sku
                    )
                    .into(),
                ));
            },
        );
    }

    fn unlink(&mut self, listing: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.outcome = None;
        self.step = None;
        self.run(
            window,
            cx,
            move |listings, _| async move {
                listings
                    .unlink(listing)
                    .await
                    .map(Some)
                    .map_err(|e| failure(&e))
            },
            |this, _: Listing, _, _| {
                this.outcome = Some(Outcome::Done(
                    "Anúncio desvinculado do produto; ele voltou para a fila a vincular.".into(),
                ));
            },
        );
    }

    fn render_sync(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let when = match self.last_sync {
            Some(at) => format!("Último Sync: {}", catalog::day_and_time(at)),
            None => "Nenhum Sync ainda.".into(),
        };
        h_flex()
            .gap_2()
            .items_center()
            .child(div().text_xs().text_color(t.text2).child(when))
            .child(
                Button::new("sync-listings")
                    .label("Sincronizar")
                    .icon(IconName::RefreshCw)
                    .primary()
                    .small()
                    .loading(self.syncing)
                    .disabled(self.busy || !self.connected())
                    .on_click(cx.listener(|this, _, window, cx| this.sync(window, cx))),
            )
            .into_any_element()
    }

    /// Title, variation and the facts the channel reports, with the link.
    fn summary(&self, id: &str, listing: &Listing, cx: &App) -> gpui_kit::Div {
        let t = look(cx).tokens;
        let listed = &listing.listed;
        let mut facts = vec![
            listed.id.clone(),
            listed.price.to_pt_br(),
            stock_text(listed.available_quantity),
        ];
        facts.extend(listed.listing_type.map(|kind| kind.name().to_owned()));
        facts.extend(
            listed
                .seller_sku
                .as_ref()
                .map(|sku| format!("SKU no ML: {sku}")),
        );
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
                            .child(listed.title.clone()),
                    )
                    .child(kit::tag(
                        status_name(listed.status),
                        status_ink(listed.status, &t),
                        cx,
                    )),
            )
            .children(
                listed
                    .variation
                    .as_ref()
                    .map(|variation| div().text_sm().child(variation.name.clone())),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_x_2()
                    .text_xs()
                    .text_color(t.text2)
                    .child(facts.join(" · "))
                    .children(listed.link.clone().map(|link| {
                        div()
                            .id(gpui_kit::SharedString::from(format!("open-{id}")))
                            .flex()
                            .gap_1()
                            .items_center()
                            .text_color(t.accent_text)
                            .cursor_pointer()
                            .child("Abrir no Mercado Livre")
                            .child(
                                gpui_kit::component::Icon::new(IconName::ExternalLink)
                                    .size(px(12.)),
                            )
                            .on_click(move |_, _, cx| cx.open_url(&link))
                    })),
            )
    }

    fn render_to_link(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let entry = &self.to_link[index];
        let id = entry.listing.id;
        let step = self
            .step
            .filter(|(open, _)| *open == id)
            .map(|(_, step)| step);
        let suggested = entry
            .suggestion
            .and_then(|suggestion| Some((suggestion, self.product(suggestion.product)?)));

        let suggestion = suggested.map(|(suggestion, product)| {
            let product_id = product.id;
            h_flex()
                .flex_wrap()
                .gap_2()
                .items_center()
                .text_sm()
                .child(div().text_color(t.text2).child("Sugestão:"))
                .child(kit::tag(product.sku.to_string(), t.accent_text, cx))
                .child(div().child(product.name.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text2)
                        .child(match suggestion.by {
                            SuggestedBy::SellerSku => "pelo SKU do anúncio",
                            SuggestedBy::Title => "pelo título",
                        }),
                )
                .when(step.is_none(), |row| {
                    row.child(
                        Button::new(("confirm-suggestion", index))
                            .label("Confirmar")
                            .icon(IconName::Check)
                            .primary()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.link(id, product_id, window, cx)
                            })),
                    )
                })
        });

        let actions = h_flex()
            .gap_2()
            .flex_wrap()
            .when(suggested.is_none(), |row| {
                row.child(
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child("Sem sugestão de produto."),
                )
            })
            .child(
                Button::new(("pick-product", index))
                    .label(if suggested.is_some() {
                        "Escolher outro"
                    } else {
                        "Escolher produto"
                    })
                    .outline()
                    .small()
                    .disabled(self.busy || self.products.is_empty())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_step(id, Step::PickProduct, window, cx)
                    })),
            )
            .child(
                Button::new(("create-product", index))
                    .label("Criar produto")
                    .outline()
                    .small()
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_step(id, Step::CreateProduct, window, cx)
                    })),
            );

        let cancel = Button::new(("cancel-step", index))
            .label("Cancelar")
            .ghost()
            .small()
            .on_click(cx.listener(|this, _, _, cx| this.close_step(cx)));
        let step_form = step.map(|step| {
            let row = h_flex().flex_wrap().gap_2().items_end();
            match step {
                Step::PickProduct => row
                    .child(
                        div().w(px(320.)).child(
                            Select::new(&self.product_pick)
                                .search_placeholder("Buscar")
                                .small()
                                .placeholder("Produto"),
                        ),
                    )
                    .child(
                        Button::new(("confirm-link", index))
                            .label("Vincular")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.link_picked(id, window, cx)
                            })),
                    )
                    .child(cancel),
                Step::CreateProduct => row
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(320.))
                            .child(div().text_sm().font_medium().child("Nome do produto"))
                            .child(Input::new(&self.product_name).small()),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(200.))
                            .child(div().text_sm().font_medium().child("SKU"))
                            .child(Input::new(&self.product_sku).small()),
                    )
                    .child(
                        Button::new(("confirm-create", index))
                            .label("Criar e vincular")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.create_product(id, window, cx)
                            })),
                    )
                    .child(cancel),
            }
        });

        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if step.is_some() {
                t.accent_edge
            } else {
                t.frame
            })
            .bg(t.surface)
            .child(self.summary(&format!("to-link-{index}"), &entry.listing, cx))
            .children(suggestion)
            .child(actions)
            .children(step_form)
            .when(step.is_some(), |card| {
                card.children(self.row_outcome.as_ref().map(|outcome| notice(outcome, cx)))
            })
            .into_any_element()
    }

    fn render_listing(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let listing = &self.listings[index];
        let id = listing.id;
        let product = listing.product.map(|id| (id, self.product(id)));
        let linked = h_flex()
            .flex_none()
            .gap_2()
            .items_center()
            .map(|row| match product {
                Some((product_id, found)) => row
                    .child(kit::tag(
                        found
                            .map(|product| product.sku.to_string())
                            .unwrap_or_else(|| "produto removido".into()),
                        t.accent_text,
                        cx,
                    ))
                    .when(found.is_some(), |row| {
                        row.child(
                            Button::new(("open-product", index))
                                .label("Abrir ficha")
                                .icon(IconName::ChevronRight)
                                .ghost()
                                .small()
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(OpenProduct(product_id));
                                })),
                        )
                    })
                    .child(
                        Button::new(("unlink", index))
                            .label("Desvincular")
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.unlink(id, window, cx)),
                            ),
                    ),
                None => row.child(kit::tag("sem produto", t.text2, cx)),
            });
        h_flex()
            .gap_3()
            .items_center()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(self.summary(&format!("listing-{index}"), listing, cx))
            .child(linked)
            .into_any_element()
    }
}

impl Render for ListingsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let mut parts = ScreenParts::new("Anúncios");
        if listings(cx).is_none() || catalog::catalog(cx).is_none() {
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
        if self.step.is_none() {
            parts
                .notices
                .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        }

        if !self.to_link.is_empty() {
            parts.content.push(
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .child(kit::section_heading(format!(
                                "A vincular ({})",
                                self.to_link.len()
                            )))
                            .child(div().text_xs().text_color(t.text2).child(
                                "anúncios sem produto: confirme a sugestão, escolha um produto \
                                 ou crie um a partir do anúncio",
                            )),
                    )
                    .children((0..self.to_link.len()).map(|index| self.render_to_link(index, cx)))
                    .into_any_element(),
            );
        }

        let heading = match self.listings.len() {
            0 => "Anúncios no Mercado Livre".to_owned(),
            1 => "1 anúncio no Mercado Livre".to_owned(),
            count => format!("{count} anúncios no Mercado Livre"),
        };
        parts.content.push(
            v_flex()
                .gap_3()
                .child(kit::section_heading(heading))
                .when(self.listings.is_empty(), |list| {
                    list.child(div().text_sm().text_color(t.text2).child(
                        "Nenhum anúncio ainda. Sincronize para trazer os anúncios ativos e \
                         pausados da sua conta; cada variação vira um anúncio próprio, vinculado ao \
                         produto que ela vende.",
                    ))
                })
                .children((0..self.listings.len()).map(|index| self.render_listing(index, cx)))
                .into_any_element(),
        );
        layout::screen(parts, cx)
    }
}

fn catalog_product(product: &Product) -> CatalogProduct {
    CatalogProduct {
        id: product.id,
        sku: product.sku.to_string(),
        name: product.name.clone(),
    }
}

fn status_name(status: ListingStatus) -> &'static str {
    match status {
        ListingStatus::Active => "Ativo",
        ListingStatus::Paused => "Pausado",
        ListingStatus::Closed => "Encerrado",
        ListingStatus::UnderReview => "Em revisão",
        ListingStatus::Inactive => "Inativo",
    }
}

fn status_ink(status: ListingStatus, t: &Tokens) -> Hsla {
    match status {
        ListingStatus::Active => t.success,
        ListingStatus::UnderReview | ListingStatus::Inactive => t.danger,
        ListingStatus::Paused | ListingStatus::Closed => t.text2,
    }
}

fn stock_text(units: u32) -> String {
    match units {
        0 => "sem estoque no ML".into(),
        1 => "1 em estoque no ML".into(),
        units => format!("{units} em estoque no ML"),
    }
}

/// What the Sync did, in one line.
fn synced(report: &ListingSync) -> String {
    let mut text = format!(
        "Sync concluído: {} anúncios lidos, {} novos e {} com mudanças.",
        report.read, report.new, report.changed
    );
    if report.gone > 0 {
        text.push_str(&format!(
            " {} que o Mercado Livre não tem mais ficaram como encerrados.",
            report.gone
        ));
    }
    text
}

fn failure(error: &ListingError) -> String {
    match error {
        ListingError::UnknownListing(_) => {
            "Esse anúncio não existe mais; a lista foi atualizada.".into()
        }
        ListingError::Platform(error) => mercado_livre::failure(error),
        ListingError::Unreadable(_) | ListingError::Sql(_) => {
            format!("Não consegui ler ou gravar no banco: {error}")
        }
    }
}
