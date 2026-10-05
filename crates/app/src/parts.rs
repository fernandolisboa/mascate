//! The parts every screen is made of, apart from where they go (#40). A
//! screen builds its parts, which hold its behavior (clicks, state, text),
//! and hands them to the current layout's arrangement in [`crate::layout`],
//! which only places them. Every layout reuses the same parts.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::{AnyElement, App, SharedString, Window};

/// What picking one of `T` runs.
pub type OnPick<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;

/// A place the navigation opens. Places join as the slices that own their
/// screens land, grouped by area (Descoberta, Vendas, Estoque, Finanças).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Today,
    Opportunities,
    Offers,
    Products,
    Listings,
    Quality,
    Questions,
    Orders,
    Sales,
    Purchases,
    Stock,
    Settings,
}

impl Place {
    /// The places listed first, in order.
    pub const MAIN: [Place; 11] = [
        Place::Today,
        Place::Opportunities,
        Place::Offers,
        Place::Products,
        Place::Listings,
        Place::Quality,
        Place::Questions,
        Place::Orders,
        Place::Sales,
        Place::Purchases,
        Place::Stock,
    ];
    /// The places kept apart at the end: the sidebar's foot, the tab bar's
    /// right side.
    pub const PINNED: [Place; 1] = [Place::Settings];

    pub fn name(self) -> &'static str {
        match self {
            Place::Today => "Hoje",
            Place::Opportunities => "Oportunidades",
            Place::Offers => "Ofertas",
            Place::Products => "Produtos",
            Place::Listings => "Anúncios",
            Place::Quality => "Qualidade",
            Place::Questions => "Perguntas",
            Place::Orders => "Pedidos",
            Place::Sales => "Vendas",
            Place::Purchases => "Compras",
            Place::Stock => "Estoque",
            Place::Settings => "Configurações",
        }
    }

    pub fn icon(self) -> IconName {
        match self {
            Place::Today => IconName::LayoutDashboard,
            Place::Opportunities => IconName::Star,
            Place::Offers => IconName::Globe,
            Place::Products => IconName::Inbox,
            Place::Listings => IconName::Building2,
            Place::Quality => IconName::CircleCheck,
            Place::Questions => IconName::CircleUser,
            Place::Orders => IconName::Bell,
            Place::Sales => IconName::ChartPie,
            Place::Purchases => IconName::FileText,
            Place::Stock => IconName::GalleryVerticalEnd,
            Place::Settings => IconName::Settings,
        }
    }
}

/// Whether the app is ready to work, for the arrangements that show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppState {
    Ready,
    /// The database did not open; the home screen says why.
    DatabaseUnavailable,
}

/// The places, the one on screen, and what picking one does.
pub struct Navigation {
    pub current: Place,
    pub state: AppState,
    pub on_pick: OnPick<Place>,
}

/// A screen, in parts.
pub struct ScreenParts {
    pub title: SharedString,
    /// What sits on the other side of the title.
    pub actions: Vec<AnyElement>,
    /// Messages about the whole screen: saved, failed.
    pub notices: Vec<AnyElement>,
    /// What the screen shows.
    pub content: Vec<AnyElement>,
}

impl ScreenParts {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            actions: Vec::new(),
            notices: Vec::new(),
            content: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::AssetSource;
    use gpui_kit::assets::Assets;

    use super::*;

    /// The app ships only gpui-kit's default icons: any other draws nothing.
    #[test]
    fn every_place_has_an_icon_the_app_ships() {
        for place in Place::MAIN.into_iter().chain(Place::PINNED) {
            assert!(
                matches!(Assets.load(&place.icon().path()), Ok(Some(_))),
                "{} has no shipped icon",
                place.name()
            );
        }
    }
}
