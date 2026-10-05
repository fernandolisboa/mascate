use gpui_kit::prelude::*;
use gpui_kit::{Entity, SharedString, Subscription, Window};
use mascate_kernel::RecordId;

use crate::appearance;
use crate::drafts::StartDraft;
use crate::home::Home;
use crate::layout;
use crate::listings::ListingsScreen;
use crate::low_stock::OpenStock;
use crate::offers::{OffersScreen, OpenProduct};
use crate::opportunities::OpportunitiesScreen;
use crate::order_alerts::OpenOrders;
use crate::orders::OrdersScreen;
use crate::parts::{AppState, Navigation, Place};
use crate::preferences;
use crate::products::ProductsScreen;
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
    purchases: Entity<PurchasesScreen>,
    stock: Entity<StockScreen>,
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
        let purchases = cx.new(|cx| PurchasesScreen::new(window, cx));
        let stock = cx.new(|cx| StockScreen::new(window, cx));
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
                    shell.listings.update(cx, |listings, cx| {
                        listings.refresh(window, cx);
                        listings.start_draft(start.0, window, cx);
                    });
                    shell.place = Place::Listings;
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
                    shell.place = Place::Stock;
                    cx.notify();
                },
            ),
            cx.subscribe_in(
                &home.read(cx).question_alerts.clone(),
                window,
                |shell, _, _: &OpenQuestions, _, cx| {
                    shell.questions.update(cx, QuestionsScreen::refresh);
                    shell.place = Place::Questions;
                    cx.notify();
                },
            ),
            cx.subscribe_in(
                &home.read(cx).reputation_alerts.clone(),
                window,
                |shell, _, _: &OpenReputation, _, cx| {
                    shell.reputation.update(cx, ReputationScreen::refresh);
                    shell.place = Place::Reputation;
                    cx.notify();
                },
            ),
            cx.subscribe_in(
                &home.read(cx).order_alerts.clone(),
                window,
                |shell, _, _: &OpenOrders, _, cx| {
                    shell.orders.update(cx, OrdersScreen::refresh);
                    shell.place = Place::Orders;
                    cx.notify();
                },
            ),
            // "Seguir o sistema" switches with the system's light/dark setting.
            cx.observe_window_appearance(window, |_, window, cx| {
                let preference = preferences::appearance(cx).theme;
                appearance::follow(preference, appearance::system_mode(window), cx);
            }),
        ];
        Self {
            place: Place::Today,
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
            purchases,
            stock,
            settings,
            _subscriptions: subscriptions,
        }
    }

    fn open_product(&mut self, product: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.products
            .update(cx, |products, cx| products.open(product, window, cx));
        self.place = Place::Products;
        cx.notify();
    }

    fn navigation(&self, cx: &mut Context<Self>) -> Navigation {
        let shell = cx.entity().downgrade();
        Navigation {
            current: self.place,
            state: self.state.clone(),
            on_pick: std::rc::Rc::new(move |place, window, cx| {
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
                        Place::Purchases => shell
                            .purchases
                            .update(cx, |purchases, cx| purchases.refresh(window, cx)),
                        Place::Stock => shell
                            .stock
                            .update(cx, |stock, cx| stock.refresh(window, cx)),
                        Place::Today => shell.home.update(cx, Home::refresh),
                        Place::Settings => {}
                    }
                    cx.notify();
                });
            }),
        }
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let navigation = self.navigation(cx);
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
            Place::Purchases => self.purchases.clone().into_any_element(),
            Place::Stock => self.stock.clone().into_any_element(),
            Place::Settings => self.settings.clone().into_any_element(),
        };
        layout::shell(navigation, screen, cx)
    }
}
