//! Buscar no site (#152), on the Ofertas screen: searches a site for items
//! and keeps the chosen ones as Supplier Offers. With the site's official
//! search and its Connection connected, through the API (#235); else by
//! crawler, shown only while the site's Restricted Feature is on, and while
//! the site keeps the crawler blocked, says until when and searches nothing.

use std::sync::Arc;

use chrono::Utc;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Entity, EventEmitter, Subscription, Window, div, px};
use mascate_catalog::{
    CatalogError, FoundOffer, OfferOrder, OfferSearch, OfferSource, SearchedOffer,
};
use mascate_integrations::ConnectionState;
use mascate_platform::FlagError;

use crate::appearance::look;
use crate::catalog;
use crate::connections::AppConnections;
use crate::crawlers::{self, CrawledSite, crawlers};
use crate::forms::{Outcome, input, notice};
use crate::found_offers;
use crate::kit;
use crate::restricted::AppFlags;
use crate::shopee_search::OffersKept;

pub struct CrawlerSearch {
    site: CrawledSite,
    keyword: Entity<InputState>,
    order: OfferOrder,
    /// The official search while its Connection is connected, else the
    /// crawler while its flag is on.
    source: Option<Arc<dyn OfferSource>>,
    /// Whether `source` is the official search.
    official: bool,
    items: Vec<SearchedOffer>,
    /// The last page read, 0 before a search.
    page: u32,
    more: bool,
    busy: bool,
    /// Numbers each search and flag read, so a slow one never lands over a
    /// newer one.
    searches: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<OffersKept> for CrawlerSearch {}

impl CrawlerSearch {
    pub fn new(site: CrawledSite, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let keyword = input("O que procurar, como fone bluetooth", window, cx);
        let subscriptions = vec![cx.subscribe_in(
            &keyword,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.search(1, window, cx);
                }
            },
        )];
        let mut search = Self {
            site,
            keyword,
            order: OfferOrder::Relevance,
            source: None,
            official: false,
            items: Vec::new(),
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

    /// Whether the site can be searched, as last read: its official search
    /// connected or its crawler's flag on.
    pub fn is_on(&self) -> bool {
        self.source.is_some()
    }

    /// The site, as Ofertas lists it.
    pub fn site(&self) -> &CrawledSite {
        &self.site
    }

    /// Reads the site's Connection and flag again: the official search
    /// while connected, else the crawler while its flag is on; neither, it
    /// drops what it found.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (Some(app), Some(engine)) = (
            cx.try_global::<AppFlags>(),
            crawlers(cx).map(|crawlers| crawlers.engine.clone()),
        ) else {
            return;
        };
        let flags = app.flags.clone();
        let site = self.site.clone();
        let official = site.official.and_then(|official| {
            let connections = cx.try_global::<AppConnections>()?.0.clone();
            Some(((official.adapter)(cx)?, official.connection, connections))
        });
        self.searches += 1;
        let reading = self.searches;
        let working = cx.background_executor().spawn(async move {
            if let Some((source, connection, connections)) = official
                && connections.state(connection) == ConnectionState::Connected
            {
                return Ok(Some((source, true)));
            }
            crawlers::source_if_on(&flags, &site, engine)
                .await
                .map(|crawler| crawler.map(|source| (source, false)))
        });
        cx.spawn(async move |this, cx| {
            let source: Result<_, FlagError> = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.searches != reading {
                    return;
                }
                let source = source.ok().flatten();
                this.official = source.as_ref().is_some_and(|(_, official)| *official);
                this.source = source.map(|(source, _)| source);
                if let Some(source) = &this.source {
                    if !source.orders().contains(&this.order) {
                        this.order = source.orders().first().copied().unwrap_or_default();
                    }
                } else {
                    this.items.clear();
                    this.page = 0;
                    this.outcome = None;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Searches for page `page`: the first replaces the list, the next ones
    /// add to it.
    fn search(&mut self, page: u32, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(catalog), Some(source)) = (catalog::catalog(cx), self.source.clone()) else {
            return;
        };
        if source.blocked_until().is_some() {
            cx.notify();
            return;
        }
        let keyword = self.keyword.read(cx).value().trim().to_owned();
        if keyword.is_empty() {
            self.outcome = Some(Outcome::Failed("Digite o que procurar.".into()));
            cx.notify();
            return;
        }
        let search = OfferSearch {
            keyword,
            order: self.order,
            page,
            ..OfferSearch::default()
        };
        self.searches += 1;
        let searching = self.searches;
        self.busy = true;
        self.outcome = None;
        let site = self.site.clone();
        let official = self.official;
        let working = cx.background_executor().spawn(async move {
            catalog
                .search_offers(source.as_ref(), &search)
                .await
                .map_err(|error| reading(&site, official, &error))
        });
        cx.spawn_in(window, async move |this, cx| {
            let found = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.searches != searching {
                    return;
                }
                this.busy = false;
                match found {
                    Ok(found) => {
                        if page == 1 {
                            this.items.clear();
                        }
                        this.items.extend(found.items);
                        this.page = page;
                        this.more = found.more;
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
        let (Some(catalog), Some(source)) = (catalog::catalog(cx), self.source.clone()) else {
            return;
        };
        let listed: Vec<FoundOffer> = self.items.iter().map(|item| item.found.clone()).collect();
        self.searches += 1;
        let searching = self.searches;
        self.busy = true;
        self.outcome = None;
        let site = self.site.clone();
        let official = self.official;
        let working = cx.background_executor().spawn(async move {
            found_offers::keep(&catalog, source.source(), &offers, listed)
                .await
                .map_err(|error| reading(&site, official, &error))
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
                        this.outcome = Some(Outcome::Done(found_offers::kept_text(&report).into()));
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

    fn render_controls(
        &self,
        source: &Arc<dyn OfferSource>,
        blocked: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let orders: Vec<AnyElement> = source
            .orders()
            .iter()
            .map(|&value| {
                let on = self.order == value;
                Button::new(("crawler-order", value as usize))
                    .label(found_offers::order_name(value))
                    .small()
                    .map(|button| if on { button.primary() } else { button.ghost() })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.order = value;
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(320.)).child(Input::new(&self.keyword).small()))
                    .child(
                        Button::new("search-crawler")
                            .label("Buscar")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(blocked || self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.search(1, window, cx)),
                            ),
                    ),
            )
            .when(orders.len() > 1, |controls| {
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
                        .children(orders),
                )
            })
            .into_any_element()
    }

    fn render_results(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.page == 0 {
            return None;
        }
        let t = look(cx).tokens;
        let pending: Vec<FoundOffer> = self
            .items
            .iter()
            .filter(|item| item.keeping_adds())
            .map(|item| item.found.clone())
            .collect();
        let cards: Vec<AnyElement> = (0..self.items.len())
            .map(|index| {
                let action = found_offers::keep_action(
                    index,
                    &self.items[index],
                    self.busy,
                    |this: &mut Self, offers, cx| this.keep(offers, cx),
                    cx,
                );
                found_offers::item_card(
                    "crawler-item-link",
                    index,
                    &self.items[index],
                    false,
                    vec![action],
                    None,
                    cx,
                )
            })
            .collect();
        let list = v_flex()
            .gap_2()
            .when(self.items.is_empty(), |list| {
                list.child(div().text_sm().text_color(t.text2).child(format!(
                    "{} não achou nada com essa busca.",
                    self.site.the_capitalized()
                )))
            })
            .when(!self.items.is_empty(), |list| {
                list.child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(t.text2)
                                .child(results_note(&self.site, &self.items)),
                        )
                        .when(!pending.is_empty(), |row| {
                            let count = pending.len();
                            row.child(
                                Button::new("keep-all-crawled")
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
            .children(cards)
            .when(self.more, |list| {
                let next = self.page + 1;
                list.child(
                    Button::new("more-crawled")
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

impl Render for CrawlerSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(source) = self.source.clone() else {
            return div().into_any_element();
        };
        let t = look(cx).tokens;
        let blocked = source.blocked_until().filter(|until| *until > Utc::now());
        let about = if self.official {
            official_about(&self.site)
        } else {
            about(&self.site, source.as_ref())
        };
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .items_center()
                    .child(kit::section_heading(format!("Buscar {}", self.site.on())))
                    .child(if self.official {
                        kit::tag("Pela sua conta", t.success, cx)
                    } else {
                        kit::tag("Crawler ligado", t.danger, cx)
                    }),
            )
            .child(div().text_sm().text_color(t.text2).child(about))
            .children(blocked.map(|until| {
                kit::error_notice(crawlers::blocked_text(&self.site.the(), until), cx)
            }))
            .child(self.render_controls(&source, blocked.is_some(), cx))
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .children(self.render_results(cx))
            .into_any_element()
    }
}

/// What the list leaves out: the delivery, for the items with a price,
/// and the price itself, for the items the page shows without one.
fn results_note(site: &CrawledSite, items: &[SearchedOffer]) -> String {
    let mut notes = Vec::new();
    if items.iter().any(|item| item.found.price.is_some()) {
        notes.push(format!(
            "A busca {} não diz quanto custa a entrega até você: a oferta guardada entra com \
             frete zero. Registre o mesmo link à mão com o frete para corrigir.",
            site.of()
        ));
    }
    if items.iter().any(|item| item.found.price.is_none()) {
        notes.push(format!(
            "Item sem preço na página {} não vira oferta guardada: abra o link para ver o preço \
             no site.",
            site.of()
        ));
    }
    notes.join(" ")
}

/// What the crawler reads and how, as the panel says it.
fn about(site: &CrawledSite, source: &dyn OfferSource) -> String {
    let what = format!(
        "Lê as páginas públicas de busca {}: uma página a cada 3 segundos ou mais, como o \
         robots.txt do site pedir, e a mesma busca dentro de 1 hora vem da memória.",
        site.of()
    );
    if source.finds_shops() {
        what
    } else {
        format!("{what} A busca de lojas não existe no crawler.")
    }
}

/// What the official search reads and how, as the panel says it.
fn official_about(site: &CrawledSite) -> String {
    format!(
        "Busca no catálogo {} pela API oficial, com a sua conta conectada: sem crawler e sem \
         página de login. Cada produto vem com o anúncio mais barato e o vendedor dele; um \
         produto que ninguém vende agora fica de fora.",
        site.of()
    )
}

/// Why a search or a keep failed, as the owner reads it: in the official
/// search's words, or the crawler's.
fn reading(site: &CrawledSite, official: bool, error: &CatalogError) -> String {
    match (error, site.official) {
        (CatalogError::Platform(error), Some(search)) if official => (search.failure)(error),
        (CatalogError::Platform(error), _) => crawlers::failure(&site.the(), error),
        (error, _) => catalog::failure(error),
    }
}

#[cfg(test)]
mod tests {
    use mascate_catalog::FoundShop;
    use mascate_integrations::{Crawler, MagaluCrawler};
    use mascate_kernel::{Currency, Money, SystemClock};
    use mascate_platform::{Build, Environment};

    use super::*;

    fn magalu() -> CrawledSite {
        CrawledSite::magalu(Build::Release, &(Box::new(|_: &str| None) as Environment))
    }

    fn searched(price: Option<u32>) -> SearchedOffer {
        let price = price.map(|reais| Money::new(reais.into(), Currency::Brl));
        SearchedOffer {
            found: FoundOffer {
                id: "1".into(),
                title: "Fone".into(),
                link: "https://www.magazineluiza.com.br/fone/p/1/".into(),
                image: None,
                shop: FoundShop {
                    id: "P1".into(),
                    name: "Loja".into(),
                    link: "https://www.magazineluiza.com.br".into(),
                    commission: None,
                    rating: None,
                },
                price,
                highest_price: price,
                sales: 0,
                commission: None,
                rating: None,
            },
            kept: None,
        }
    }

    #[test]
    fn the_panel_says_how_slowly_it_reads_and_what_it_does_not_search() {
        let site = magalu();
        let engine = Arc::new(Crawler::new("Mascate/test", Arc::new(SystemClock)));
        let about = about(&site, &MagaluCrawler::new(&site.listing, engine));

        assert!(about.starts_with("Lê as páginas públicas de busca da Magalu"));
        assert!(about.contains("3 segundos"));
        assert!(!about.contains("categoria"));
    }

    #[test]
    fn the_list_explains_the_missing_price_and_the_missing_delivery_as_each_applies() {
        let site = magalu();

        let priceless = results_note(&site, &[searched(None)]);
        let priced = results_note(&site, &[searched(Some(97))]);
        let both = results_note(&site, &[searched(Some(97)), searched(None)]);

        assert!(priceless.starts_with("Item sem preço na página da Magalu"));
        assert!(!priceless.contains("frete"));
        assert!(priced.contains("frete zero"));
        assert!(!priced.contains("sem preço"));
        assert!(both.contains("frete zero") && both.contains("Item sem preço"));
    }
}
