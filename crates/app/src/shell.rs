use std::rc::Rc;

use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::{Entity, SharedString, Subscription, Window, div};
use mascate_kernel::RecordId;

use crate::affiliate::AffiliateScreen;
use crate::appearance;
use crate::backups::AppBackups;
use crate::dashboard::DashboardScreen;
use crate::drafts::StartDraft;
use crate::event_alerts::OpenCalendar;
use crate::home::Home;
use crate::layout;
use crate::listings::ListingsScreen;
use crate::low_stock::OpenStock;
use crate::offer_posts::StartPost;
use crate::offers::{OffersScreen, OpenProduct};
use crate::opportunities::OpportunitiesScreen;
use crate::order_alerts::OpenOrders;
use crate::orders::OrdersScreen;
use crate::parts::{AppState, Navigation, Place};
use crate::preferences;
use crate::products::ProductsScreen;
use crate::promotion_channels::PromotionScreen;
use crate::promotions::PromotionsScreen;
use crate::purchases::PurchasesScreen;
use crate::quality::QualityScreen;
use crate::question_alerts::OpenQuestions;
use crate::questions::QuestionsScreen;
use crate::reputation::ReputationScreen;
use crate::reputation_alerts::OpenReputation;
use crate::sales::SalesScreen;
use crate::settings::SettingsScreen;
use crate::stock::StockScreen;
use crate::title_bar;
use crate::work_context::WorkContext;

/// The main window: the navigation and the current screen; where each goes
/// is the layout's ([`crate::layout`]).
pub struct Shell {
    place: Place,
    state: AppState,
    home: Entity<Home>,
    opportunities: Entity<OpportunitiesScreen>,
    offers: Entity<OffersScreen>,
    products: Entity<ProductsScreen>,
    listings: Entity<ListingsScreen>,
    quality: Entity<QualityScreen>,
    questions: Entity<QuestionsScreen>,
    reputation: Entity<ReputationScreen>,
    promotions: Entity<PromotionsScreen>,
    orders: Entity<OrdersScreen>,
    sales: Entity<SalesScreen>,
    dashboard: Entity<DashboardScreen>,
    purchases: Entity<PurchasesScreen>,
    stock: Entity<StockScreen>,
    affiliate: Entity<AffiliateScreen>,
    promotion: Entity<PromotionScreen>,
    settings: Entity<SettingsScreen>,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    /// `problem` says why the database is not ready, if it isn't.
    pub fn new(problem: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = match problem {
            None => AppState::Ready,
            Some(_) => AppState::DatabaseUnavailable,
        };
        // Back from a merge, the app opens on its report in Settings.
        let place = if cx
            .try_global::<AppBackups>()
            .is_some_and(|backups| backups.merged.is_some())
        {
            Place::Settings
        } else {
            Place::Today
        };
        let home = cx.new(|cx| Home::new(problem, cx));
        let opportunities = cx.new(|cx| OpportunitiesScreen::new(window, cx));
        let offers = cx.new(|cx| OffersScreen::new(window, cx));
        let products = cx.new(|cx| ProductsScreen::new(window, cx));
        let listings = cx.new(|cx| ListingsScreen::new(window, cx));
        let quality = cx.new(|cx| QualityScreen::new(window, cx));
        let questions = cx.new(|cx| QuestionsScreen::new(window, cx));
        let reputation = cx.new(|cx| ReputationScreen::new(window, cx));
        let promotions = cx.new(|cx| PromotionsScreen::new(window, cx));
        let orders = cx.new(|cx| OrdersScreen::new(window, cx));
        let sales = cx.new(|cx| SalesScreen::new(window, cx));
        let dashboard = cx.new(|cx| DashboardScreen::new(window, cx));
        let purchases = cx.new(|cx| PurchasesScreen::new(window, cx));
        let stock = cx.new(|cx| StockScreen::new(window, cx));
        let affiliate = cx.new(|cx| AffiliateScreen::new(window, cx));
        let promotion = cx.new(|cx| PromotionScreen::new(window, cx));
        let settings = cx.new(|cx| SettingsScreen::new(window, cx));
        // The startup theme guessed the system's appearance before any window
        // existed; this window knows it.
        appearance::follow(
            preferences::appearance(cx).theme,
            appearance::system_mode(window),
            cx,
        );
        let subscriptions = vec![
            cx.subscribe_in(
                &opportunities,
                window,
                |shell, _, open: &OpenProduct, window, cx| {
                    shell.open_product(open.0, window, cx);
                },
            ),
            cx.subscribe_in(
                &offers,
                window,
                |shell, _, open: &OpenProduct, window, cx| {
                    shell.open_product(open.0, window, cx);
                },
            ),
            cx.subscribe_in(
                &listings,
                window,
                |shell, _, open: &OpenProduct, window, cx| {
                    shell.open_product(open.0, window, cx);
                },
            ),
            cx.subscribe_in(
                &products,
                window,
                |shell, _, start: &StartDraft, window, cx| {
                    shell.go(Place::Listings, window, cx);
                    shell.listings.update(cx, |listings, cx| {
                        listings.refresh(window, cx);
                        listings.start_draft(start.0, window, cx);
                    });
                    cx.notify();
                },
            ),
            cx.subscribe_in(
                &home.read(cx).low_stock.clone(),
                window,
                |shell, _, open: &OpenStock, window, cx| {
                    shell
                        .stock
                        .update(cx, |stock, cx| stock.open(Some(open.0), window, cx));
                    shell.go(Place::Stock, window, cx);
                    cx.notify();
                },
            ),
            cx.subscribe_in(
                &home.read(cx).question_alerts.clone(),
                window,
                |shell, _, _: &OpenQuestions, window, cx| {
                    shell.questions.update(cx, QuestionsScreen::refresh);
                    shell.go(Place::Questions, window, cx);
                    cx.notify();
                },
            ),
            cx.subscribe_in(
                &home.read(cx).reputation_alerts.clone(),
                window,
                |shell, _, _: &OpenReputation, window, cx| {
                    shell.reputation.update(cx, ReputationScreen::refresh);
                    shell.go(Place::Reputation, window, cx);
                    cx.notify();
                },
            ),
            cx.subscribe_in(
                &home.read(cx).order_alerts.clone(),
                window,
                |shell, _, _: &OpenOrders, window, cx| {
                    shell.orders.update(cx, OrdersScreen::refresh);
                    shell.go(Place::Orders, window, cx);
                    cx.notify();
                },
            ),
            cx.subscribe_in(
                &home.read(cx).event_alerts.clone(),
                window,
                |shell, _, _: &OpenCalendar, window, cx| {
                    shell
                        .promotion
                        .update(cx, |promotion, cx| promotion.refresh(window, cx));
                    shell.go(Place::Promotion, window, cx);
                    cx.notify();
                },
            ),
            cx.subscribe_in(
                &affiliate.read(cx).affiliate_links.clone(),
                window,
                |shell, _, start: &StartPost, window, cx| shell.start_post(start, window, cx),
            ),
            cx.subscribe_in(
                &offers.read(cx).shopee.clone(),
                window,
                |shell, _, start: &StartPost, window, cx| shell.start_post(start, window, cx),
            ),
            // "Seguir o sistema" switches with the system's light/dark setting.
            cx.observe_window_appearance(window, |_, window, cx| {
                let preference = preferences::appearance(cx).theme;
                appearance::follow(preference, appearance::system_mode(window), cx);
            }),
        ];
        Self {
            place,
            state,
            home,
            opportunities,
            offers,
            products,
            listings,
            quality,
            questions,
            reputation,
            promotions,
            orders,
            sales,
            dashboard,
            purchases,
            stock,
            affiliate,
            promotion,
            settings,
            _subscriptions: subscriptions,
        }
    }

    /// Opens `place`, asked from another screen; a place the Work Context
    /// does not list brings back All, which lists every place.
    fn go(&mut self, place: Place, window: &mut Window, cx: &mut Context<Self>) {
        if !place.shows_in(preferences::work_context(cx)) {
            self.switch_context(WorkContext::All, window, cx);
        }
        self.place = place;
    }

    fn open_product(&mut self, product: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.products
            .update(cx, |products, cx| products.open(product, window, cx));
        self.go(Place::Products, window, cx);
        cx.notify();
    }

    /// Opens a new post from a link, in All when the Work Context does not
    /// show the link's Platform.
    fn start_post(&mut self, start: &StartPost, window: &mut Window, cx: &mut Context<Self>) {
        if !preferences::work_context(cx).shows_platform(start.platform) {
            self.switch_context(WorkContext::All, window, cx);
        }
        self.promotion
            .update(cx, |promotion, cx| promotion.start_post(start, window, cx));
        self.go(Place::Promotion, window, cx);
        cx.notify();
    }

    fn navigation(&self, cx: &mut Context<Self>) -> Navigation {
        let shell = cx.entity().downgrade();
        Navigation {
            current: self.place,
            context: preferences::work_context(cx),
            state: self.state.clone(),
            on_pick: Rc::new(move |place, window, cx| {
                let _ = shell.update(cx, |shell, cx| {
                    shell.place = place;
                    // Each visit reads its data again: another screen may
                    // have changed it.
                    match place {
                        Place::Opportunities => shell
                            .opportunities
                            .update(cx, |opportunities, cx| opportunities.refresh(window, cx)),
                        Place::Offers => shell
                            .offers
                            .update(cx, |offers, cx| offers.refresh(window, cx)),
                        Place::Products => shell
                            .products
                            .update(cx, |products, cx| products.refresh(window, cx)),
                        Place::Listings => shell
                            .listings
                            .update(cx, |listings, cx| listings.refresh(window, cx)),
                        Place::Quality => shell.quality.update(cx, QualityScreen::refresh),
                        Place::Questions => shell.questions.update(cx, QuestionsScreen::refresh),
                        Place::Reputation => shell.reputation.update(cx, ReputationScreen::refresh),
                        Place::Promotions => shell
                            .promotions
                            .update(cx, |promotions, cx| promotions.refresh(window, cx)),
                        Place::Orders => shell.orders.update(cx, OrdersScreen::refresh),
                        Place::Sales => shell.sales.update(cx, SalesScreen::refresh),
                        Place::Dashboard => shell.dashboard.update(cx, DashboardScreen::refresh),
                        Place::Purchases => shell
                            .purchases
                            .update(cx, |purchases, cx| purchases.refresh(window, cx)),
                        Place::Stock => shell
                            .stock
                            .update(cx, |stock, cx| stock.refresh(window, cx)),
                        Place::Affiliate => shell
                            .affiliate
                            .update(cx, |affiliate, cx| affiliate.refresh(window, cx)),
                        Place::Promotion => shell
                            .promotion
                            .update(cx, |promotion, cx| promotion.refresh(window, cx)),
                        Place::Today => shell.home.update(cx, Home::refresh),
                        Place::Settings => {}
                    }
                    cx.notify();
                });
            }),
        }
    }
}

impl Shell {
    /// Shows `context` from the next frame on: each screen closes what it
    /// had open on another channel or Platform without sending it, and a
    /// place the context does not list gives way to Hoje.
    fn switch_context(
        &mut self,
        context: WorkContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if preferences::work_context(cx) == context {
            return;
        }
        let saving = preferences::set_work_context(context, cx);
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = saving.await {
                    eprintln!("{error}");
                }
            })
            .detach();
        self.listings
            .update(cx, |listings, cx| listings.follow_context(window, cx));
        self.orders.update(cx, OrdersScreen::follow_context);
        self.promotions
            .update(cx, |promotions, cx| promotions.follow_context(window, cx));
        self.affiliate
            .update(cx, |affiliate, cx| affiliate.follow_context(window, cx));
        self.promotion.update(cx, PromotionScreen::follow_context);
        self.dashboard.update(cx, DashboardScreen::refresh);
        self.quality.update(cx, |_, cx| cx.notify());
        self.sales.update(cx, |_, cx| cx.notify());
        self.home.update(cx, |_, cx| cx.notify());
        if !self.place.shows_in(context) {
            self.place = Place::Today;
            self.home.update(cx, Home::refresh);
        }
        cx.notify();
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let navigation = self.navigation(cx);
        let shell = cx.entity().downgrade();
        let screen = match self.place {
            Place::Today => self.home.clone().into_any_element(),
            Place::Opportunities => self.opportunities.clone().into_any_element(),
            Place::Offers => self.offers.clone().into_any_element(),
            Place::Products => self.products.clone().into_any_element(),
            Place::Listings => self.listings.clone().into_any_element(),
            Place::Quality => self.quality.clone().into_any_element(),
            Place::Questions => self.questions.clone().into_any_element(),
            Place::Reputation => self.reputation.clone().into_any_element(),
            Place::Promotions => self.promotions.clone().into_any_element(),
            Place::Orders => self.orders.clone().into_any_element(),
            Place::Sales => self.sales.clone().into_any_element(),
            Place::Dashboard => self.dashboard.clone().into_any_element(),
            Place::Purchases => self.purchases.clone().into_any_element(),
            Place::Stock => self.stock.clone().into_any_element(),
            Place::Affiliate => self.affiliate.clone().into_any_element(),
            Place::Promotion => self.promotion.clone().into_any_element(),
            Place::Settings => self.settings.clone().into_any_element(),
        };
        v_flex()
            .size_full()
            .child(title_bar::title_bar(
                preferences::work_context(cx),
                Rc::new(move |context, window, cx| {
                    let _ = shell.update(cx, |shell, cx| shell.switch_context(context, window, cx));
                }),
                window,
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(layout::shell(navigation, screen, cx)),
            )
    }
}
