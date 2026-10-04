//! Oportunidades (#11): Supplier Offers crossed with Mercado Livre demand,
//! ranked by score, with the Estimated Margin after the channel's Fees; the
//! best sellers of the categories the owner follows; the assumptions behind
//! the margins; and the steps from an Opportunity to a Product (the #9 flow)
//! or to the bin, with a reason.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Entity, EventEmitter, Global, SharedString, Window, div, px};
use mascate_catalog::{
    Catalog, CatalogError, CategoryBestSellers, DemandCategory, DemandSource, DemandSync,
    DiscoverySettings, Opportunity, OpportunityFilter, Product, Sku,
};
use mascate_finance::Taxes;
use mascate_integrations::{Connection, ConnectionState};
use mascate_kernel::{Currency, ListingType, Money, Percentage, RecordId, Timestamp, parse_amount};

use crate::appearance::look;
use crate::catalog::{self, NO_DATABASE, failure};
use crate::connections::AppConnections;
use crate::forms::{Outcome, input, notice};
use crate::kit;
use crate::layout;
use crate::mercado_livre;
use crate::offers::OpenProduct;
use crate::parts::ScreenParts;

/// The tax rate setting, shared by every screen that works out a margin.
pub struct AppTaxes(pub Arc<Taxes>);

impl Global for AppTaxes {}

/// The step an Opportunity's row is in, below it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    CreateProduct,
    Dismiss,
}

/// Everything the screen shows, read in one go off the UI thread.
struct Snapshot {
    settings: DiscoverySettings,
    tax: Percentage,
    followed: Vec<DemandCategory>,
    opportunities: Vec<Opportunity>,
    best_sellers: Vec<CategoryBestSellers>,
    dismissed: u64,
    last_sync: Option<Timestamp>,
    products: Vec<Product>,
    connection: ConnectionState,
}

/// What a background change hands back for the screen to act on.
enum Changed {
    Nothing,
    Synced(DemandSync),
    Categories(Vec<DemandCategory>),
    Product(Product),
    Sku(Sku),
    Saved(&'static str),
}

/// Why a change failed, as the owner reads it.
enum Failure {
    Catalog(CatalogError),
    Text(String),
}

impl From<CatalogError> for Failure {
    fn from(error: CatalogError) -> Self {
        Failure::Catalog(error)
    }
}

pub struct OpportunitiesScreen {
    settings: DiscoverySettings,
    listing_type: ListingType,
    fee: Entity<InputState>,
    shipping: Entity<InputState>,
    tax: Entity<InputState>,
    min_margin: Entity<InputState>,
    min_price: Entity<InputState>,
    max_price: Entity<InputState>,
    category: Option<String>,
    filter: OpportunityFilter,
    followed: Vec<DemandCategory>,
    /// The categories Mercado Livre offers, with the ones ticked, while the
    /// owner picks which to follow.
    picking: Option<(Vec<DemandCategory>, Vec<String>)>,
    opportunities: Vec<Opportunity>,
    best_sellers: Vec<CategoryBestSellers>,
    dismissed: u64,
    last_sync: Option<Timestamp>,
    products: Vec<Product>,
    connection: Option<ConnectionState>,
    /// The Opportunity whose row is open, and for what.
    step: Option<(RecordId, Step)>,
    product_name: Entity<InputState>,
    product_sku: Entity<InputState>,
    reason: Entity<InputState>,
    busy: bool,
    syncing: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    /// The outcome of the last action on the whole screen.
    outcome: Option<Outcome>,
    /// The outcome of the last step on a row.
    row_outcome: Option<Outcome>,
}

impl EventEmitter<OpenProduct> for OpportunitiesScreen {}

impl OpportunitiesScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut screen = Self {
            settings: DiscoverySettings::default(),
            listing_type: ListingType::default(),
            fee: input("14", window, cx),
            shipping: input("20,00", window, cx),
            tax: input("0", window, cx),
            min_margin: input("Ex.: 20", window, cx),
            min_price: input("De", window, cx),
            max_price: input("Até", window, cx),
            category: None,
            filter: OpportunityFilter::default(),
            followed: Vec::new(),
            picking: None,
            opportunities: Vec::new(),
            best_sellers: Vec::new(),
            dismissed: 0,
            last_sync: None,
            products: Vec::new(),
            connection: None,
            step: None,
            product_name: input("Nome do produto", window, cx),
            product_sku: input("SKU", window, cx),
            reason: input("Por que não serve? Ex.: produto diferente", window, cx),
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
        self.run(
            window,
            cx,
            |_| async { Ok(Changed::Nothing) },
            |_, _, _, _| {},
        );
    }

    /// Runs `change` off the UI thread, then reads the screen's data again
    /// and hands what the change returned to `then`.
    fn run<F, Fut>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: F,
        then: impl FnOnce(&mut Self, Changed, &mut Window, &mut Context<Self>) + 'static,
    ) where
        F: FnOnce(Arc<Catalog>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Changed, Failure>> + Send,
    {
        let (Some(catalog), Some(taxes)) = (
            catalog::catalog(cx),
            cx.try_global::<AppTaxes>().map(|t| t.0.clone()),
        ) else {
            return;
        };
        let connections = cx.global::<AppConnections>().0.clone();
        let filter = self.filter.clone();
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(catalog.clone()).await;
            let read = async {
                let tax = taxes
                    .rate()
                    .await
                    .map_err(|error| Failure::Text(error.to_string()))?;
                Ok::<_, Failure>(Snapshot {
                    settings: catalog.discovery_settings().await?,
                    followed: catalog.demand_categories().await?,
                    opportunities: catalog.opportunities(&filter, tax).await?,
                    best_sellers: catalog.best_sellers().await?,
                    dismissed: catalog.dismissed_opportunities().await?,
                    last_sync: catalog.last_demand_sync().await?,
                    products: catalog.products().await?,
                    connection: connections.state(Connection::MercadoLivre),
                    tax,
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
                        Err(error) => this.outcome = Some(Outcome::Failed(reading(&error).into())),
                    }
                }
                match changed {
                    Ok(changed) => then(this, changed, window, cx),
                    Err(error) => this.failed(&error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Shows a failure where the owner acted: the open row, or the screen.
    fn failed(&mut self, error: &Failure) {
        let text = Outcome::Failed(reading(error).into());
        if self.step.is_some() {
            self.row_outcome = Some(text);
        } else {
            self.outcome = Some(text);
        }
    }

    fn show(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        // The fields show what is saved until the owner types over them.
        if self.settings != snapshot.settings || self.reads == 1 {
            self.listing_type = snapshot.settings.listing_type;
            let fee = percent_text(snapshot.settings.estimated_fee);
            let shipping = amount_text(snapshot.settings.estimated_shipping);
            self.fee
                .update(cx, |input, cx| input.set_value(fee, window, cx));
            self.shipping
                .update(cx, |input, cx| input.set_value(shipping, window, cx));
        }
        let tax = percent_text(snapshot.tax);
        if self.tax.read(cx).value().trim().is_empty() || self.reads == 1 {
            self.tax
                .update(cx, |input, cx| input.set_value(tax, window, cx));
        }
        if self
            .step
            .is_some_and(|(open, _)| !snapshot.opportunities.iter().any(|o| o.id == open))
        {
            self.step = None;
        }
        self.settings = snapshot.settings;
        self.followed = snapshot.followed;
        self.opportunities = snapshot.opportunities;
        self.best_sellers = snapshot.best_sellers;
        self.dismissed = snapshot.dismissed;
        self.last_sync = snapshot.last_sync;
        self.products = snapshot.products;
        self.connection = Some(snapshot.connection);
    }

    fn connected(&self) -> bool {
        self.connection == Some(ConnectionState::Connected)
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = mercado_livre::adapter(cx) else {
            return;
        };
        self.outcome = None;
        self.step = None;
        self.syncing = true;
        self.run(
            window,
            cx,
            move |catalog| async move {
                Ok(Changed::Synced(catalog.sync_demand(source.as_ref()).await?))
            },
            |this, changed, _, _| {
                if let Changed::Synced(report) = changed {
                    this.outcome = Some(Outcome::Done(synced(&report).into()));
                }
            },
        );
    }

    fn pick_categories(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = mercado_livre::adapter(cx) else {
            return;
        };
        self.outcome = None;
        self.run(
            window,
            cx,
            move |_| async move {
                source
                    .categories()
                    .map(Changed::Categories)
                    .map_err(|error| Failure::Catalog(error.into()))
            },
            |this, changed, _, _| {
                if let Changed::Categories(offered) = changed {
                    let ticked = this.followed.iter().map(|c| c.id.clone()).collect();
                    this.picking = Some((offered, ticked));
                }
            },
        );
    }

    fn save_categories(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((offered, ticked)) = self.picking.take() else {
            return;
        };
        let chosen: Vec<DemandCategory> = offered
            .into_iter()
            .filter(|category| ticked.contains(&category.id))
            .collect();
        self.run(
            window,
            cx,
            move |catalog| async move {
                catalog.follow_categories(&chosen).await?;
                Ok(Changed::Saved(
                    "Categorias salvas. O próximo Sync lê os mais vendidos delas.",
                ))
            },
            |this, changed, _, _| {
                if let Changed::Saved(text) = changed {
                    this.outcome = Some(Outcome::Done(text.into()));
                }
            },
        );
    }

    fn save_assumptions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fee = Percentage::parse(&self.fee.read(cx).value());
        let tax = Percentage::parse(&self.tax.read(cx).value());
        let shipping = parse_amount(&self.shipping.read(cx).value())
            .filter(|amount| !amount.is_sign_negative())
            .map(|amount| Money::new(amount, Currency::Brl));
        let (Some(fee), Some(tax), Some(shipping)) = (fee, tax, shipping) else {
            self.outcome = Some(Outcome::Failed(
                "Digite tarifa e imposto como porcentagem de 0 a 100 (ex.: 14 ou 12,5) e o \
                 frete como 20,00."
                    .into(),
            ));
            cx.notify();
            return;
        };
        let settings = DiscoverySettings {
            listing_type: self.listing_type,
            estimated_fee: fee,
            estimated_shipping: shipping,
        };
        let Some(taxes) = cx.try_global::<AppTaxes>().map(|t| t.0.clone()) else {
            return;
        };
        self.outcome = None;
        self.run(
            window,
            cx,
            move |catalog| async move {
                catalog.save_discovery_settings(settings).await?;
                taxes
                    .save_rate(tax)
                    .await
                    .map_err(|error| Failure::Text(error.to_string()))?;
                Ok(Changed::Saved(
                    "Premissas salvas; as margens abaixo já usam os valores novos.",
                ))
            },
            |this, changed, _, _| {
                if let Changed::Saved(text) = changed {
                    this.outcome = Some(Outcome::Done(text.into()));
                }
            },
        );
    }

    fn apply_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let number = |input: &Entity<InputState>| {
            let text = input.read(cx).value().to_string();
            if text.trim().is_empty() {
                Ok(None)
            } else {
                parse_amount(text.trim_end_matches('%')).map(Some).ok_or(())
            }
        };
        let (Ok(min_margin), Ok(min_price), Ok(max_price)) = (
            number(&self.min_margin),
            number(&self.min_price),
            number(&self.max_price),
        ) else {
            self.outcome = Some(Outcome::Failed(
                "Digite os filtros como números: margem 20, preço 49,90.".into(),
            ));
            cx.notify();
            return;
        };
        self.filter = OpportunityFilter {
            min_margin,
            category: self.category.clone(),
            min_price,
            max_price,
        };
        self.outcome = None;
        self.refresh(window, cx);
    }

    fn clear_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [&self.min_margin, &self.min_price, &self.max_price] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.category = None;
        self.filter = OpportunityFilter::default();
        self.refresh(window, cx);
    }

    fn open_step(
        &mut self,
        opportunity: RecordId,
        step: Step,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step = Some((opportunity, step));
        self.row_outcome = None;
        self.outcome = None;
        let Some(found) = self.opportunities.iter().find(|o| o.id == opportunity) else {
            return;
        };
        match step {
            Step::CreateProduct => {
                let name = found.product_name.clone();
                self.product_name
                    .update(cx, |input, cx| input.set_value(name.clone(), window, cx));
                self.product_sku
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.run(
                    window,
                    cx,
                    move |catalog| async move { Ok(Changed::Sku(catalog.suggest_sku(&name).await?)) },
                    |this, changed, window, cx| {
                        if let Changed::Sku(sku) = changed {
                            this.product_sku.update(cx, |input, cx| {
                                input.set_value(sku.to_string(), window, cx)
                            });
                        }
                    },
                );
            }
            Step::Dismiss => {
                self.reason
                    .update(cx, |input, cx| input.set_value("", window, cx));
            }
        }
        cx.notify();
    }

    /// The #9 flow: the Opportunity's offer becomes a Product with its folder.
    fn create_product(&mut self, offer: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.product_name.read(cx).value().to_string();
        let sku = self.product_sku.read(cx).value().to_string();
        self.row_outcome = None;
        self.run(
            window,
            cx,
            move |catalog| async move {
                Ok(Changed::Product(
                    catalog.create_product(offer, &name, &sku).await?,
                ))
            },
            |this, changed, _, _| {
                if let Changed::Product(product) = changed {
                    this.step = None;
                    this.outcome = Some(Outcome::Done(
                        format!(
                            "Produto {} criado, com a pasta para os arquivos dele.",
                            product.sku
                        )
                        .into(),
                    ));
                }
            },
        );
    }

    fn dismiss(&mut self, opportunity: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let reason = self.reason.read(cx).value().to_string();
        self.row_outcome = None;
        self.run(
            window,
            cx,
            move |catalog| async move {
                catalog.dismiss_opportunity(opportunity, &reason).await?;
                Ok(Changed::Saved(
                    "Oportunidade descartada; ela não volta nos próximos Syncs.",
                ))
            },
            |this, changed, _, _| {
                if let Changed::Saved(text) = changed {
                    this.step = None;
                    this.outcome = Some(Outcome::Done(text.into()));
                }
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
                Button::new("sync-demand")
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

    fn render_assumptions(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, width: f32, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .w(px(width))
                .child(div().text_sm().font_medium().child(label))
                .child(Input::new(input).small())
        };
        let listing = |kind: ListingType| {
            let on = self.listing_type == kind;
            Button::new(("listing-type", kind as usize))
                .label(kind.name())
                .small()
                .map(|button| {
                    if on {
                        button.primary()
                    } else {
                        button.outline()
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.listing_type = kind;
                    cx.notify();
                }))
        };
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(kit::section_heading("Premissas do cálculo"))
            .child(div().text_sm().text_color(t.text2).child(
                "A tarifa vem do Mercado Livre para a categoria e o preço; sem ela, vale a \
                 tarifa estimada. O frete estimado entra a partir de R$ 79,00, quando o frete \
                 grátis é obrigatório. O imposto vale para todos os cálculos de margem.",
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_end()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_sm().font_medium().child("Tipo de anúncio"))
                            .child(h_flex().gap_1().children(ListingType::ALL.map(listing))),
                    )
                    .child(field("Tarifa estimada (%)", 150., &self.fee))
                    .child(field("Frete estimado (R$)", 150., &self.shipping))
                    .child(field("Imposto (%)", 120., &self.tax))
                    .child(
                        Button::new("save-assumptions")
                            .label("Salvar premissas")
                            .outline()
                            .small()
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.save_assumptions(window, cx)
                                }),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_categories(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let body = match &self.picking {
            Some((offered, ticked)) => v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .children(offered.iter().enumerate().map(|(index, category)| {
                            let on = ticked.contains(&category.id);
                            let id = category.id.clone();
                            Button::new(("pick-category", index))
                                .label(category.name.clone())
                                .small()
                                .map(|b| if on { b.primary() } else { b.outline() })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some((_, ticked)) = &mut this.picking {
                                        match ticked.iter().position(|known| *known == id) {
                                            Some(at) => {
                                                ticked.remove(at);
                                            }
                                            None => ticked.push(id.clone()),
                                        }
                                    }
                                    cx.notify();
                                }))
                        })),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("save-categories")
                                .label("Salvar categorias")
                                .primary()
                                .small()
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_categories(window, cx)
                                })),
                        )
                        .child(
                            Button::new("cancel-categories")
                                .label("Cancelar")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.picking = None;
                                    cx.notify();
                                })),
                        ),
                ),
            None => v_flex().gap_2().child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .when(self.followed.is_empty(), |row| {
                        row.child(div().text_sm().text_color(t.text2).child(
                            "Nenhuma categoria ainda. Escolha as categorias cujos mais vendidos \
                             você quer acompanhar.",
                        ))
                    })
                    .children(
                        self.followed
                            .iter()
                            .map(|category| kit::tag(category.name.clone(), t.accent_text, cx)),
                    )
                    .child(
                        Button::new("pick-categories")
                            .label("Escolher categorias")
                            .ghost()
                            .small()
                            .disabled(self.busy || !self.connected())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_categories(window, cx)),
                            ),
                    ),
            ),
        };
        v_flex()
            .gap_2()
            .child(kit::section_heading("Categorias acompanhadas"))
            .child(body)
            .into_any_element()
    }

    fn render_filters(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let mut categories: Vec<DemandCategory> = self.followed.clone();
        for opportunity in &self.opportunities {
            if let Some(category) = &opportunity.category
                && !categories.iter().any(|known| known.id == category.id)
            {
                categories.push(category.clone());
            }
        }
        if let Some(wanted) = &self.filter.category
            && !categories.iter().any(|known| &known.id == wanted)
        {
            categories.push(DemandCategory {
                id: wanted.clone(),
                name: wanted.clone(),
            });
        }
        let chip = |index: usize, label: SharedString, value: Option<String>| {
            let on = self.category == value;
            Button::new(("category-filter", index))
                .label(label)
                .small()
                .map(|b| if on { b.primary() } else { b.outline() })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.category = value.clone();
                    this.apply_filters(window, cx);
                }))
        };
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .w(px(130.))
                .child(div().text_xs().text_color(t.text2).child(label))
                .child(Input::new(input).small())
        };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .child(chip(0, "Todas as categorias".into(), None))
                    .children(categories.iter().enumerate().map(|(index, category)| {
                        chip(
                            index + 1,
                            category.name.clone().into(),
                            Some(category.id.clone()),
                        )
                    })),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_end()
                    .child(field("Margem mínima (%)", &self.min_margin))
                    .child(field("Preço de (R$)", &self.min_price))
                    .child(field("Preço até (R$)", &self.max_price))
                    .child(
                        Button::new("apply-filters")
                            .label("Filtrar")
                            .outline()
                            .small()
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.apply_filters(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("clear-filters")
                            .label("Limpar")
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.clear_filters(window, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_opportunity(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let opportunity = &self.opportunities[index];
        let id = opportunity.id;
        let offer = opportunity.offer.id;
        let step = self
            .step
            .filter(|(open, _)| *open == id)
            .map(|(_, step)| step);
        let product = opportunity
            .offer
            .product
            .and_then(|product| self.products.iter().find(|p| p.id == product));
        let margin_ink = if opportunity.margin.amount.is_negative() {
            t.danger
        } else {
            t.success
        };
        let estimated = |estimated: bool| if estimated { " (estimado)" } else { "" };
        let breakdown = format!(
            "{} médio − tarifa {}{} − frete {}{} − imposto {} − custo {}",
            opportunity.sale_price.to_pt_br(),
            opportunity.sale_fee.amount.to_pt_br(),
            estimated(opportunity.sale_fee.estimated),
            opportunity.shipping.amount.to_pt_br(),
            estimated(
                opportunity.shipping.estimated && !opportunity.shipping.amount.amount().is_zero()
            ),
            opportunity.tax.to_pt_br(),
            opportunity.cost.to_pt_br(),
        );
        let mut tags = h_flex().flex_wrap().gap_1();
        if let Some(position) = opportunity.best_seller {
            tags = tags.child(kit::tag(
                format!("{position}º mais vendido"),
                t.accent_text,
                cx,
            ));
        }
        if let Some(category) = &opportunity.category {
            tags = tags.child(kit::tag(category.name.clone(), t.text2, cx));
        }
        tags = tags
            .child(kit::tag(
                competitors_text(opportunity.competitors),
                t.text2,
                cx,
            ))
            .child(kit::tag(
                format!(
                    "{} a {}",
                    opportunity.lowest_price.to_pt_br(),
                    opportunity.highest_price.to_pt_br()
                ),
                t.text2,
                cx,
            ))
            .child(kit::tag(
                format!(
                    "pontuação {}",
                    opportunity
                        .score
                        .round_dp(1)
                        .normalize()
                        .to_string()
                        .replace('.', ",")
                ),
                t.text2,
                cx,
            ));

        let actions = h_flex()
            .gap_2()
            .flex_wrap()
            .items_center()
            .map(|row| match (product, step) {
                (Some(product), _) => {
                    let product_id = product.id;
                    row.child(kit::tag(product.sku.to_string(), t.accent_text, cx))
                        .child(
                            Button::new(("open-product", index))
                                .label("Abrir ficha")
                                .icon(IconName::ChevronRight)
                                .ghost()
                                .small()
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(OpenProduct(product_id));
                                })),
                        )
                }
                (None, None) => row.child(
                    Button::new(("create-product", index))
                        .label("Criar produto")
                        .outline()
                        .small()
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_step(id, Step::CreateProduct, window, cx)
                        })),
                ),
                (None, Some(_)) => row,
            })
            .when(step.is_none(), |row| {
                row.child(
                    Button::new(("dismiss", index))
                        .label("Descartar")
                        .ghost()
                        .small()
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_step(id, Step::Dismiss, window, cx)
                        })),
                )
            });

        let cancel = Button::new(("cancel-step", index))
            .label("Cancelar")
            .ghost()
            .small()
            .on_click(cx.listener(|this, _, _, cx| {
                this.step = None;
                this.row_outcome = None;
                cx.notify();
            }));
        let step_form =
            step.map(|step| {
                let row = h_flex().flex_wrap().gap_2().items_end();
                match step {
                    Step::CreateProduct => row
                        .child(
                            v_flex()
                                .gap_1()
                                .w(px(300.))
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
                                .label("Criar")
                                .primary()
                                .small()
                                .loading(self.busy)
                                .disabled(self.busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.create_product(offer, window, cx)
                                })),
                        )
                        .child(cancel),
                    Step::Dismiss => row
                        .child(
                            v_flex()
                                .gap_1()
                                .w(px(420.))
                                .child(div().text_sm().font_medium().child("Motivo"))
                                .child(Input::new(&self.reason).small()),
                        )
                        .child(
                            Button::new(("confirm-dismiss", index))
                                .label("Descartar")
                                .danger()
                                .small()
                                .loading(self.busy)
                                .disabled(self.busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.dismiss(id, window, cx)
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
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .justify_between()
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .gap_0p5()
                            .child(div().font_medium().child(opportunity.product_name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.text2)
                                    .truncate()
                                    .child(format!(
                                        "Oferta: {} · {}",
                                        opportunity.offer.title, opportunity.offer.supplier.name
                                    )),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_none()
                            .items_end()
                            .gap_0p5()
                            .child(
                                div()
                                    .font_semibold()
                                    .text_color(margin_ink)
                                    .child(opportunity.margin.amount.to_pt_br()),
                            )
                            .child(div().text_xs().text_color(margin_ink).child(format!(
                                "margem estimada {}",
                                opportunity.margin.percent_to_pt_br()
                            ))),
                    ),
            )
            .child(tags)
            .child(div().text_xs().text_color(t.text2).child(breakdown))
            .child(actions)
            .children(step_form)
            .when(step.is_some(), |card| {
                card.children(self.row_outcome.as_ref().map(|outcome| notice(outcome, cx)))
            })
            .into_any_element()
    }

    fn render_best_sellers(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let mut section = v_flex()
            .gap_3()
            .child(kit::section_heading("Mais vendidos no Mercado Livre"));
        if self.best_sellers.iter().all(|c| c.best_sellers.is_empty()) {
            section = section.child(div().text_sm().text_color(t.text2).child(
                "Os 20 mais vendidos de cada categoria acompanhada aparecem aqui depois do \
                 Sync.",
            ));
        }
        for category in self
            .best_sellers
            .iter()
            .filter(|c| !c.best_sellers.is_empty())
        {
            section = section.child(
                v_flex()
                    .gap_1()
                    .p_3()
                    .rounded(t.radius_lg)
                    .border(t.border_width)
                    .border_color(t.frame)
                    .bg(t.surface)
                    .child(div().font_medium().child(category.category.name.clone()))
                    .children(category.best_sellers.iter().map(|ranked| {
                        let seller = &ranked.best_seller;
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_sm()
                            .child(
                                div()
                                    .w(px(28.))
                                    .flex_none()
                                    .text_color(t.text2)
                                    .child(format!("{}º", seller.position)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(seller.title.clone()),
                            )
                            .children(seller.price.map(|price| {
                                div()
                                    .flex_none()
                                    .text_color(t.text2)
                                    .child(price.to_pt_br())
                            }))
                            .child(if ranked.has_opportunity {
                                kit::tag("com oferta", t.success, cx)
                            } else {
                                kit::tag("sem oferta", t.text2, cx)
                            })
                    })),
            );
        }
        section.into_any_element()
    }
}

impl Render for OpportunitiesScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let mut parts = ScreenParts::new("Oportunidades");
        if catalog::catalog(cx).is_none() {
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
        let heading = match self.opportunities.len() {
            0 => "Oportunidades".to_owned(),
            1 => "1 oportunidade".to_owned(),
            count => format!("{count} oportunidades"),
        };
        let empty = if self.filter == OpportunityFilter::default() {
            "Nenhuma oportunidade ainda. Registre ofertas em Ofertas e sincronize: cada oferta \
             é comparada com os produtos do catálogo do Mercado Livre que combinam com o título \
             dela."
        } else {
            "Nenhuma oportunidade passa nesses filtros."
        };
        parts.content.push(
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .gap_2()
                        .items_baseline()
                        .child(kit::section_heading(heading))
                        .child(div().text_xs().text_color(t.text2).child(
                            "ordenadas pela pontuação: margem estimada, mais vendido e concorrência",
                        )),
                )
                .child(self.render_filters(cx))
                .when(self.opportunities.is_empty(), |list| {
                    list.child(div().text_sm().text_color(t.text2).child(empty))
                })
                .children((0..self.opportunities.len()).map(|index| self.render_opportunity(index, cx)))
                .when(self.dismissed > 0, |list| {
                    list.child(div().text_xs().text_color(t.text2).child(match self.dismissed {
                        1 => "1 oportunidade descartada não aparece mais.".to_owned(),
                        count => format!("{count} oportunidades descartadas não aparecem mais."),
                    }))
                })
                .into_any_element(),
        );
        parts.content.push(self.render_best_sellers(cx));
        parts.content.push(self.render_categories(cx));
        parts.content.push(self.render_assumptions(cx));
        layout::screen(parts, cx)
    }
}

/// "14", "12,5": a rate as the owner types it.
fn percent_text(rate: Percentage) -> String {
    rate.percent().normalize().to_string().replace('.', ",")
}

/// "20,00": an amount as the owner types it.
fn amount_text(amount: Money) -> String {
    format!("{:.2}", amount.rounded().amount()).replace('.', ",")
}

fn competitors_text(count: u32) -> String {
    match count {
        1 => "1 concorrente".into(),
        count => format!("{count} concorrentes"),
    }
}

/// What the Sync read, in one line.
fn synced(report: &DemandSync) -> String {
    let mut text = format!(
        "Sync concluído: {} mais vendidos e {} oportunidades de {} ofertas.",
        report.best_sellers, report.opportunities, report.offers
    );
    if report.unmatched > 0 {
        text.push_str(&format!(
            " {} sem produto correspondente no catálogo do Mercado Livre.",
            report.unmatched
        ));
    }
    if report.other_currency > 0 {
        text.push_str(&format!(
            " {} em outra moeda ficaram de fora.",
            report.other_currency
        ));
    }
    text
}

fn reading(error: &Failure) -> String {
    match error {
        Failure::Text(text) => format!("Não consegui ler ou gravar no banco: {text}"),
        Failure::Catalog(CatalogError::Platform(error)) => mercado_livre::failure(error),
        Failure::Catalog(CatalogError::MissingReason) => {
            "Diga por que a oportunidade não serve antes de descartar.".into()
        }
        Failure::Catalog(error) => failure(error),
    }
}
