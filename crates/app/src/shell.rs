use gpui_kit::prelude::*;
use gpui_kit::{Entity, SharedString, Subscription, Window};

use crate::appearance;
use crate::home::Home;
use crate::layout;
use crate::offers::{OffersScreen, OpenProduct};
use crate::parts::{AppState, Navigation, Place};
use crate::preferences;
use crate::products::ProductsScreen;
use crate::settings::SettingsScreen;

/// The main window: the navigation and the current screen; where each goes
/// is the layout's ([`crate::layout`]).
pub struct Shell {
    place: Place,
    state: AppState,
    home: Entity<Home>,
    offers: Entity<OffersScreen>,
    products: Entity<ProductsScreen>,
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
        let offers = cx.new(|cx| OffersScreen::new(window, cx));
        let products = cx.new(|cx| ProductsScreen::new(window, cx));
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
                &offers,
                window,
                |shell, _, open: &OpenProduct, window, cx| {
                    shell
                        .products
                        .update(cx, |products, cx| products.open(open.0, window, cx));
                    shell.place = Place::Products;
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
            offers,
            products,
            settings,
            _subscriptions: subscriptions,
        }
    }

    fn navigation(&self, cx: &mut Context<Self>) -> Navigation {
        let shell = cx.entity().downgrade();
        Navigation {
            current: self.place,
            state: self.state.clone(),
            on_pick: std::rc::Rc::new(move |place, window, cx| {
                let _ = shell.update(cx, |shell, cx| {
                    shell.place = place;
                    // Each visit reads the catalog again: the other screen
                    // may have changed it.
                    match place {
                        Place::Offers => shell
                            .offers
                            .update(cx, |offers, cx| offers.refresh(window, cx)),
                        Place::Products => shell
                            .products
                            .update(cx, |products, cx| products.refresh(window, cx)),
                        Place::Today | Place::Settings => {}
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
            Place::Offers => self.offers.clone().into_any_element(),
            Place::Products => self.products.clone().into_any_element(),
            Place::Settings => self.settings.clone().into_any_element(),
        };
        layout::shell(navigation, screen, cx)
    }
}
