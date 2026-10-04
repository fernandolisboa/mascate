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
    Purchases,
    Stock,
    Settings,
}

impl Place {
    /// The places listed first, in order.
    pub const MAIN: [Place; 6] = [
        Place::Today,
        Place::Opportunities,
        Place::Offers,
        Place::Products,
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
