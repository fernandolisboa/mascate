//! Buscar na Shopee (#12), on the Ofertas screen: searches the Shopee
//! Affiliate Open API for items or shops and keeps the chosen items as
//! Supplier Offers. Off, with the reason, while the Connection waits for
//! Shopee's approval; registering offers by hand keeps working below it.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Entity, EventEmitter, Subscription, Window, div, px};
use mascate_catalog::{
    Catalog, CatalogError, FoundOffer, FoundShop, KeptOffers, OfferOrder, OfferSearch, OfferSource,
    ProductSource, SearchedOffer,
};
use mascate_integrations::{Connection, ConnectionState};
use rust_decimal::Decimal;

use crate::appearance::look;
use crate::catalog;
use crate::connections::AppConnections;
use crate::forms::{Outcome, input, notice};
use crate::kit;
use crate::shopee;

/// Tells the Ofertas screen that offers were kept, so it reads them again.
pub struct OffersKept;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Items,
    Shops,
}

/// What a search brought back, appended to the list on later pages.
enum Results {
    Items(Vec<SearchedOffer>),
    Shops(Vec<FoundShop>),
}

pub struct ShopeeSearch {
    keyword: Entity<InputState>,
    category: Entity<InputState>,
    mode: Mode,
    order: OfferOrder,
    /// The shop the item search is narrowed to.
    shop: Option<FoundShop>,
    connection: Option<ConnectionState>,
    items: Vec<SearchedOffer>,
    shops: Vec<FoundShop>,
    /// The last page read, 0 before a search.
    page: u32,
    more: bool,
    busy: bool,
    /// Numbers each search, so a slow one never lands over a newer one.
    searches: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<OffersKept> for ShopeeSearch {}

impl ShopeeSearch {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let keyword = input("O que procurar, como fone bluetooth", window, cx);
        let category = input("Número ou link da categoria", window, cx);
        let subscriptions = [&keyword, &category]
            .into_iter()
            .map(|field| {
                cx.subscribe_in(field, window, |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.search(1, window, cx);
                    }
                })
            })
            .collect();
        let mut search = Self {
            keyword,
            category,
            mode: Mode::Items,
            order: OfferOrder::default(),
            shop: None,
            connection: None,
            items: Vec::new(),
            shops: Vec::new(),
            page: 0,
            more: false,
            busy: false,
            searches: 0,
            outcome: None,
            _subscriptions: subscriptions,
        };
        search.refresh(cx);
        search
    }

    /// Reads the Connection's state again, off the UI thread because the
    /// system store may ask to be unlocked.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let connections = cx.global::<AppConnections>().0.clone();
        let reading = cx
            .background_executor()
            .spawn(async move { connections.state(Connection::ShopeeAffiliates) });
        cx.spawn(async move |this, cx| {
            let state = reading.await;
            let _ = this.update(cx, |this, cx| {
                this.connection = Some(state);
                cx.notify();
            });
        })
        .detach();
    }

    fn off(&self) -> Option<&'static str> {
        self.connection.as_ref().and_then(shopee::search_off)
    }

    /// Searches for page `page`: the first replaces the list, the next ones
    /// add to it.
    fn search(&mut self, page: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.off().is_some() || self.connection.is_none() {
            return;
        }
        let (Some(catalog), Some(source)) = (catalog::catalog(cx), shopee::adapter(cx)) else {
            return;
        };
        let keyword = self.keyword.read(cx).value().trim().to_owned();
        let category = self.category.read(cx).value().trim().to_owned();
        if self.mode == Mode::Shops && keyword.is_empty() {
            self.outcome = Some(Outcome::Failed("Digite parte do nome da loja.".into()));
            cx.notify();
            return;
        }
        if self.mode == Mode::Items
            && keyword.is_empty()
            && category.is_empty()
            && self.shop.is_none()
        {
            self.outcome = Some(Outcome::Failed(
                "Digite o que procurar, uma categoria ou escolha uma loja.".into(),
            ));
            cx.notify();
            return;
        }
        let mode = self.mode;
        let search = OfferSearch {
            keyword: keyword.clone(),
            category: (!category.is_empty()).then_some(category),
            shop: self.shop.as_ref().map(|shop| shop.id.clone()),
            order: self.order,
            page,
        };
        self.searches += 1;
        let searching = self.searches;
        self.busy = true;
        self.outcome = None;
        let working = cx.background_executor().spawn(async move {
            match mode {
                Mode::Items => catalog
                    .search_offers(source.as_ref(), &search)
                    .await
                    .map(|found| (Results::Items(found.items), found.more))
                    .map_err(|error| reading(&error)),
                Mode::Shops => source
                    .search_shops(&keyword, page)
                    .map(|found| (Results::Shops(found.items), found.more))
                    .map_err(|error| shopee::failure(&error)),
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let found = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.searches != searching {
                    return;
                }
                this.busy = false;
                match found {
                    Ok((results, more)) => {
                        if page == 1 {
                            this.items.clear();
                            this.shops.clear();
                        }
                        match results {
                            Results::Items(items) => this.items.extend(items),
                            Results::Shops(shops) => this.shops.extend(shops),
                        }
                        this.page = page;
                        this.more = more;
                    }
                    Err(text) => this.outcome = Some(Outcome::Failed(text.into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Keeps `offers` as Supplier Offers, then marks the list again.
    fn keep(&mut self, offers: Vec<FoundOffer>, cx: &mut Context<Self>) {
        let Some(catalog) = catalog::catalog(cx) else {
            return;
        };
        let listed: Vec<FoundOffer> = self.items.iter().map(|item| item.found.clone()).collect();
        self.searches += 1;
        let searching = self.searches;
        self.busy = true;
        self.outcome = None;
        let working = cx.background_executor().spawn(async move {
            keep(&catalog, &offers, listed)
                .await
                .map_err(|error| reading(&error))
        });
        cx.spawn(async move |this, cx| {
            let kept = working.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match kept {
                    Ok((report, items)) => {
                        if this.searches == searching {
                            this.items = items;
                        }
                        this.outcome = Some(Outcome::Done(kept_text(&report).into()));
                        cx.emit(OffersKept);
                    }
                    Err(text) => this.outcome = Some(Outcome::Failed(text.into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn show_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            self.page = 0;
            self.more = false;
            self.outcome = None;
            cx.notify();
        }
    }

    fn view_shop(&mut self, shop: FoundShop, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = Mode::Items;
        self.shop = Some(shop);
        self.keyword
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.search(1, window, cx);
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let off = self.off().is_some() || self.connection.is_none();
        let mode = |value: Mode, label: &'static str, id: &'static str| {
            let on = self.mode == value;
            Button::new(id)
                .label(label)
                .small()
                .map(|button| {
                    if on {
                        button.primary()
                    } else {
                        button.outline()
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| this.show_mode(value, cx)))
        };
        let order = |value: OfferOrder| {
            let on = self.order == value;
            Button::new(("order", value as usize))
                .label(order_name(value))
                .small()
                .map(|button| if on { button.primary() } else { button.ghost() })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.order = value;
                    cx.notify();
                }))
        };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(mode(Mode::Items, "Produtos", "mode-items"))
                    .child(mode(Mode::Shops, "Lojas", "mode-shops"))
                    .child(div().w(px(320.)).child(Input::new(&self.keyword).small()))
                    .when(self.mode == Mode::Items, |row| {
                        row.child(div().w(px(240.)).child(Input::new(&self.category).small()))
                    })
                    .child(
                        Button::new("search-shopee")
                            .label("Buscar")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(off || self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.search(1, window, cx)),
                            ),
                    ),
            )
            .when(self.mode == Mode::Items, |controls| {
                controls.child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .items_center()
                        .child(
                            div()
                                .text_sm()
                                .text_color(look(cx).tokens.text2)
                                .child("Ordem:"),
                        )
                        .children(OfferOrder::ALL.map(order))
                        .children(self.shop.as_ref().map(|shop| {
                            h_flex()
                                .gap_1()
                                .items_center()
                                .ml_2()
                                .child(kit::tag(
                                    format!("Só da loja {}", shop.name),
                                    look(cx).tokens.accent_text,
                                    cx,
                                ))
                                .child(
                                    Button::new("clear-shop")
                                        .label("Todas as lojas")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.shop = None;
                                            cx.notify();
                                        })),
                                )
                        })),
                )
            })
            .into_any_element()
    }

    fn render_item(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let item = &self.items[index];
        let found = &item.found;
        let link = found.link.clone();
        let mut facts = vec![found.shop.name.clone(), sold_text(found.sales)];
        if let Some(commission) = found.commission {
            facts.push(format!("comissão {}", commission.to_pt_br()));
        }
        if let Some(rating) = found.rating.filter(|rating| !rating.is_zero()) {
            facts.push(format!("nota {}", stars_text(rating)));
        }
        let price = if found.highest_price.amount() > found.price.amount() {
            format!(
                "{} a {}",
                found.price.to_pt_br(),
                found.highest_price.to_pt_br()
            )
        } else {
            found.price.to_pt_br()
        };
        let action = if item.up_to_date() {
            kit::tag("Nas ofertas", t.success, cx).into_any_element()
        } else {
            let offer = found.clone();
            let label = match &item.kept {
                Some(kept) => format!("Guardar preço novo (era {})", kept.price.to_pt_br()),
                None => "Guardar nas ofertas".to_owned(),
            };
            Button::new(("keep-offer", index))
                .label(label)
                .outline()
                .small()
                .disabled(self.busy)
                .on_click(cx.listener(move |this, _, _, cx| this.keep(vec![offer.clone()], cx)))
                .into_any_element()
        };
        h_flex()
            .gap_3()
            .items_start()
            .justify_between()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .gap_0p5()
                    .child(div().font_medium().child(found.title.clone()))
                    .child(div().text_xs().text_color(t.text2).child(facts.join(" · ")))
                    .child(
                        div()
                            .id(("shopee-item-link", index))
                            .text_xs()
                            .text_color(t.accent_text)
                            .truncate()
                            .cursor_pointer()
                            .child(link.clone())
                            .on_click(move |_, _, cx| cx.open_url(&link)),
                    ),
            )
            .child(
                v_flex()
                    .flex_none()
                    .items_end()
                    .gap_1()
                    .child(div().font_semibold().child(price))
                    .child(action),
            )
            .into_any_element()
    }

    fn render_shop(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let shop = &self.shops[index];
        let link = shop.link.clone();
        let mut facts = Vec::new();
        if let Some(commission) = shop.commission {
            facts.push(format!("comissão {}", commission.to_pt_br()));
        }
        if let Some(rating) = shop.rating.filter(|rating| !rating.is_zero()) {
            facts.push(format!("nota {}", stars_text(rating)));
        }
        let chosen = shop.clone();
        h_flex()
            .gap_3()
            .items_center()
            .justify_between()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .gap_0p5()
                    .child(div().font_medium().child(shop.name.clone()))
                    .when(!facts.is_empty(), |column| {
                        column.child(div().text_xs().text_color(t.text2).child(facts.join(" · ")))
                    })
                    .child(
                        div()
                            .id(("shopee-shop-link", index))
                            .text_xs()
                            .text_color(t.accent_text)
                            .truncate()
                            .cursor_pointer()
                            .child(link.clone())
                            .on_click(move |_, _, cx| cx.open_url(&link)),
                    ),
            )
            .child(
                Button::new(("view-shop", index))
                    .label("Ver produtos")
                    .outline()
                    .small()
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.view_shop(chosen.clone(), window, cx)
                    })),
            )
            .into_any_element()
    }

    fn render_results(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.page == 0 {
            return None;
        }
        let t = look(cx).tokens;
        let count = match self.mode {
            Mode::Items => self.items.len(),
            Mode::Shops => self.shops.len(),
        };
        let pending: Vec<FoundOffer> = self
            .items
            .iter()
            .filter(|item| !item.up_to_date())
            .map(|item| item.found.clone())
            .collect();
        let list = v_flex()
            .gap_2()
            .when(count == 0, |list| {
                list.child(
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child("A Shopee não achou nada com essa busca."),
                )
            })
            .when(self.mode == Mode::Items && count > 0, |list| {
                list.child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .items_center()
                        .child(div().flex_1().text_xs().text_color(t.text2).child(
                            "A Shopee não diz quanto custa a entrega até você: a oferta guardada \
                             entra com frete zero. Registre o mesmo link à mão com o frete para \
                             corrigir.",
                        ))
                        .when(!pending.is_empty(), |row| {
                            let count = pending.len();
                            row.child(
                                Button::new("keep-all")
                                    .label(format!("Guardar todas ({count})"))
                                    .outline()
                                    .small()
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.keep(pending.clone(), cx)
                                    })),
                            )
                        }),
                )
            })
            .map(|list| match self.mode {
                Mode::Items => list.children((0..count).map(|index| self.render_item(index, cx))),
                Mode::Shops => list.children((0..count).map(|index| self.render_shop(index, cx))),
            })
            .when(self.more, |list| {
                let next = self.page + 1;
                list.child(
                    Button::new("more-results")
                        .label("Mais resultados")
                        .ghost()
                        .small()
                        .loading(self.busy)
                        .disabled(self.busy)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.search(next, window, cx)),
                        ),
                )
            });
        Some(list.into_any_element())
    }
}

impl Render for ShopeeSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(kit::section_heading("Buscar na Shopee"))
            .children(self.off().map(|why| kit::info_notice(why, cx)))
            .child(self.render_controls(cx))
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .children(self.render_results(cx))
    }
}

/// Keeps `offers` and marks `listed` again with what is kept now.
async fn keep(
    catalog: &Arc<Catalog>,
    offers: &[FoundOffer],
    listed: Vec<FoundOffer>,
) -> Result<(KeptOffers, Vec<SearchedOffer>), CatalogError> {
    let report = catalog
        .keep_found_offers(ProductSource::ShopeeAffiliate, offers)
        .await?;
    let items = catalog
        .with_kept(ProductSource::ShopeeAffiliate, listed)
        .await?;
    Ok((report, items))
}

/// Why a search or a keep failed, as the owner reads it.
fn reading(error: &CatalogError) -> String {
    match error {
        CatalogError::Platform(error) => shopee::failure(error),
        error => catalog::failure(error),
    }
}

fn kept_text(report: &KeptOffers) -> String {
    let mut parts = Vec::new();
    match report.added {
        0 => {}
        1 => parts.push("1 oferta guardada".to_owned()),
        added => parts.push(format!("{added} ofertas guardadas")),
    }
    match report.unchanged {
        0 => {}
        1 => parts.push("1 já estava nas ofertas".to_owned()),
        unchanged => parts.push(format!("{unchanged} já estavam nas ofertas")),
    }
    match report.skipped {
        0 => {}
        1 => parts.push("1 sem título, link ou preço ficou de fora".to_owned()),
        skipped => parts.push(format!(
            "{skipped} sem título, link ou preço ficaram de fora"
        )),
    }
    let mut text = if parts.is_empty() {
        "Nada para guardar.".to_owned()
    } else {
        format!("{}.", parts.join("; "))
    };
    if report.added > 0 {
        text.push_str(" As novas entram nas Oportunidades no próximo Sync.");
    }
    text
}

fn order_name(order: OfferOrder) -> &'static str {
    match order {
        OfferOrder::Sales => "Mais vendidos",
        OfferOrder::Relevance => "Relevância",
        OfferOrder::LowestPrice => "Menor preço",
        OfferOrder::HighestCommission => "Maior comissão",
    }
}

/// "12.400 vendidos".
fn sold_text(sales: u32) -> String {
    let digits = sales.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push('.');
        }
        grouped.push(digit);
    }
    match sales {
        1 => "1 vendido".into(),
        _ => format!("{grouped} vendidos"),
    }
}

/// "4,9".
fn stars_text(stars: Decimal) -> String {
    stars.round_dp(1).normalize().to_string().replace('.', ",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sales_read_with_thousands_grouped() {
        assert_eq!(sold_text(0), "0 vendidos");
        assert_eq!(sold_text(1), "1 vendido");
        assert_eq!(sold_text(830), "830 vendidos");
        assert_eq!(sold_text(12_400), "12.400 vendidos");
        assert_eq!(sold_text(1_234_567), "1.234.567 vendidos");
    }

    #[test]
    fn kept_offers_read_as_a_sentence() {
        let report = KeptOffers {
            added: 2,
            unchanged: 1,
            skipped: 0,
        };
        assert_eq!(
            kept_text(&report),
            "2 ofertas guardadas; 1 já estava nas ofertas. As novas entram nas Oportunidades \
             no próximo Sync."
        );
        assert_eq!(kept_text(&KeptOffers::default()), "Nada para guardar.");
    }
}
