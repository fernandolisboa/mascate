//! Buscar no site por crawler (#152), on the Ofertas screen: searches a
//! site's public pages for items and keeps the chosen ones as Supplier
//! Offers. A site whose robots.txt keeps robots out of its search (the
//! Hotmart marketplace, #154) is browsed by category instead. Shown only
//! while the site's Restricted Feature is on; while the site keeps the
//! crawler blocked, says until when and searches nothing.

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
use mascate_platform::FlagError;

use crate::appearance::look;
use crate::catalog;
use crate::crawlers::{self, CrawledSite, crawlers};
use crate::forms::{Outcome, input, notice};
use crate::found_offers;
use crate::kit;
use crate::restricted::AppFlags;
use crate::shopee_search::OffersKept;

pub struct CrawlerSearch {
    site: CrawledSite,
    keyword: Entity<InputState>,
    /// The category browsed, as its slug or `category/subcategory`; `None`
    /// for the site's best sellers. Only on a site browsed by category.
    category: Option<String>,
    order: OfferOrder,
    /// The crawler, while its flag is on.
    source: Option<Arc<dyn OfferSource>>,
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
            category: None,
            order: OfferOrder::Relevance,
            source: None,
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

    /// Whether the site's flag is on, as last read.
    pub fn is_on(&self) -> bool {
        self.source.is_some()
    }

    /// Reads the site's flag again: on, it builds the crawler; off, it drops
    /// it with what it found.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (Some(app), Some(engine)) = (
            cx.try_global::<AppFlags>(),
            crawlers(cx).map(|crawlers| crawlers.engine.clone()),
        ) else {
            return;
        };
        let flags = app.flags.clone();
        let site = self.site.clone();
        self.searches += 1;
        let reading = self.searches;
        let working = cx
            .background_executor()
            .spawn(async move { crawlers::source_if_on(&flags, &site, engine).await });
        cx.spawn(async move |this, cx| {
            let source: Result<_, FlagError> = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.searches != reading {
                    return;
                }
                this.source = source.ok().flatten();
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
        let keyword = if self.site.browses() {
            String::new()
        } else {
            self.keyword.read(cx).value().trim().to_owned()
        };
        if keyword.is_empty() && !self.site.browses() {
            self.outcome = Some(Outcome::Failed("Digite o que procurar.".into()));
            cx.notify();
            return;
        }
        let search = OfferSearch {
            keyword,
            category: self.category.clone(),
            order: self.order,
            page,
            ..OfferSearch::default()
        };
        self.searches += 1;
        let searching = self.searches;
        self.busy = true;
        self.outcome = None;
        let site = self.site.clone();
        let working = cx.background_executor().spawn(async move {
            catalog
                .search_offers(source.as_ref(), &search)
                .await
                .map_err(|error| reading(&site, &error))
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
        let working = cx.background_executor().spawn(async move {
            found_offers::keep(&catalog, source.source(), &offers, listed)
                .await
                .map_err(|error| reading(&site, &error))
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

    /// Browses `category` (`None` for the best sellers) from its first page.
    fn browse(&mut self, category: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.category = category;
        self.search(1, window, cx);
    }

    /// The categories as buttons, the chosen category's subcategories under
    /// them: a click reads that page.
    fn render_categories(&self, blocked: bool, cx: &mut Context<Self>) -> AnyElement {
        let chosen = self.category.as_deref();
        let top = chosen.and_then(|path| path.split('/').next());
        let off = blocked || self.busy;
        let chip = |id: (&'static str, usize),
                    label: &'static str,
                    path: Option<String>,
                    on: bool,
                    cx: &mut Context<Self>| {
            Button::new(id)
                .label(label)
                .small()
                .map(|button| if on { button.primary() } else { button.ghost() })
                .disabled(off)
                .on_click(
                    cx.listener(move |this, _, window, cx| this.browse(path.clone(), window, cx)),
                )
                .into_any_element()
        };
        let mut categories = vec![chip(
            ("crawler-best-sellers", 0),
            "Mais vendidos",
            None,
            self.page > 0 && chosen.is_none(),
            cx,
        )];
        for (index, category) in self.site.categories.iter().enumerate() {
            categories.push(chip(
                ("crawler-category", index),
                category.name,
                Some(category.slug.to_owned()),
                top == Some(category.slug),
                cx,
            ));
        }
        let subcategories: Vec<AnyElement> = self
            .site
            .categories
            .iter()
            .find(|category| top == Some(category.slug))
            .map(|category| {
                let mut row = vec![chip(
                    ("crawler-whole-category", 0),
                    "Toda a categoria",
                    Some(category.slug.to_owned()),
                    chosen == Some(category.slug),
                    cx,
                )];
                for (index, (slug, name)) in category.subcategories.iter().enumerate() {
                    let path = format!("{}/{slug}", category.slug);
                    let on = chosen == Some(path.as_str());
                    row.push(chip(
                        ("crawler-subcategory", index),
                        name,
                        Some(path),
                        on,
                        cx,
                    ));
                }
                row
            })
            .unwrap_or_default();
        let t = look(cx).tokens;
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .items_center()
                    .child(div().text_sm().text_color(t.text2).child("Categoria:"))
                    .children(categories),
            )
            .when(!subcategories.is_empty(), |list| {
                list.child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .items_center()
                        .child(div().text_sm().text_color(t.text2).child("Dentro dela:"))
                        .children(subcategories),
                )
            })
            .into_any_element()
    }

    fn render_controls(
        &self,
        source: &Arc<dyn OfferSource>,
        blocked: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.site.browses() {
            return self.render_categories(blocked, cx);
        }
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
        let about = about(&self.site, source.as_ref());
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
                    .child(kit::tag("Crawler ligado", t.danger, cx)),
            )
            .child(div().text_sm().text_color(t.text2).child(about))
            .children(
                blocked
                    .map(|until| kit::error_notice(crawlers::blocked_text(&self.site, until), cx)),
            )
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
    let what = if site.browses() {
        format!(
            "Lê a vitrine pública {}: os mais vendidos e os primeiros produtos de cada \
             categoria, uma página a cada 3 segundos ou mais, e a mesma página dentro de 1 hora \
             vem da memória. O robots.txt {} proíbe robôs na busca por palavras, então aqui você \
             escolhe uma categoria em vez de digitar.",
            site.of(),
            site.of()
        )
    } else {
        format!(
            "Lê as páginas públicas de busca {}: uma página a cada 3 segundos ou mais, como o \
             robots.txt do site pedir, e a mesma busca dentro de 1 hora vem da memória.",
            site.of()
        )
    };
    if source.finds_shops() {
        what
    } else {
        format!("{what} A busca de lojas não existe no crawler.")
    }
}

/// Why a search or a keep failed, as the owner reads it.
fn reading(site: &CrawledSite, error: &CatalogError) -> String {
    match error {
        CatalogError::Platform(error) => crawlers::failure(site, error),
        error => catalog::failure(error),
    }
}

#[cfg(test)]
mod tests {
    use mascate_catalog::FoundShop;
    use mascate_integrations::{Crawler, HotmartCrawler, MagaluCrawler};
    use mascate_kernel::{Currency, Money, SystemClock};
    use mascate_platform::{Build, Environment};

    use super::*;

    fn sites() -> (CrawledSite, CrawledSite) {
        let environment = Box::new(|_: &str| None) as Environment;
        (
            CrawledSite::hotmart(Build::Release, &environment),
            CrawledSite::magalu(Build::Release, &environment),
        )
    }

    fn searched(price: Option<u32>) -> SearchedOffer {
        let price = price.map(|reais| Money::new(reais.into(), Currency::Brl));
        SearchedOffer {
            found: FoundOffer {
                id: "1".into(),
                title: "Curso".into(),
                link: "https://hotmart.com/pt-br/marketplace/produtos/curso/A1".into(),
                image: None,
                shop: FoundShop {
                    id: "P1".into(),
                    name: "Produtora".into(),
                    link: "https://hotmart.com/pt-br/marketplace".into(),
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
    fn the_hotmart_panel_says_why_it_browses_categories_instead_of_words() {
        let (hotmart, magalu) = sites();
        let engine = Arc::new(Crawler::new("Mascate/test", Arc::new(SystemClock)));
        let browsing = about(
            &hotmart,
            &HotmartCrawler::new(&hotmart.listing, engine.clone()),
        );
        let searching = about(&magalu, &MagaluCrawler::new(&magalu.listing, engine));

        assert!(browsing.contains("robots.txt da Hotmart proíbe robôs na busca por palavras"));
        assert!(browsing.contains("escolhe uma categoria"));
        assert!(browsing.ends_with("A busca de lojas não existe no crawler."));
        assert!(!searching.contains("categoria"));
    }

    #[test]
    fn the_list_explains_the_missing_price_and_the_missing_delivery_as_each_applies() {
        let (hotmart, _) = sites();

        let priceless = results_note(&hotmart, &[searched(None)]);
        let priced = results_note(&hotmart, &[searched(Some(97))]);
        let both = results_note(&hotmart, &[searched(Some(97)), searched(None)]);

        assert!(priceless.starts_with("Item sem preço na página da Hotmart"));
        assert!(!priceless.contains("frete"));
        assert!(priced.contains("frete zero"));
        assert!(!priced.contains("sem preço"));
        assert!(both.contains("frete zero") && both.contains("Item sem preço"));
    }
}
