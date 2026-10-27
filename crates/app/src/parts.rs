//! The parts every screen is made of, apart from where they go (#40). A
//! screen builds its parts, which hold its behavior (clicks, state, text),
//! and hands them to the current layout's arrangement in [`crate::layout`],
//! which only places them. Every layout reuses the same parts.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::{AnyElement, App, SharedString, Window};
use mascate_kernel::SalesChannelKind;

use crate::work_context::WorkContext;

/// What picking one of `T` runs.
pub type OnPick<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;

/// A place the navigation opens. Places join as the slices that own their
/// screens land, each in the [`PlaceGroup`] of its area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Today,
    Opportunities,
    Offers,
    Products,
    Listings,
    Quality,
    Questions,
    Reputation,
    Promotions,
    Orders,
    Sales,
    Dashboard,
    Purchases,
    Stock,
    Affiliate,
    Promotion,
    Settings,
}

/// Places that belong together, under one heading in the navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaceGroup {
    /// `None` for the group that opens the navigation, which needs none.
    pub name: Option<&'static str>,
    pub places: &'static [Place],
}

impl PlaceGroup {
    /// Its places the Work Context shows, in order; a group left empty
    /// goes from the navigation with its heading.
    pub fn shown(&self, context: WorkContext) -> Vec<Place> {
        self.places
            .iter()
            .copied()
            .filter(|place| place.shows_in(context))
            .collect()
    }
}

impl Place {
    /// Every place but the pinned ones, by area, in the order the work goes:
    /// find, stock, sell, follow the money, promote.
    pub const GROUPS: [PlaceGroup; 6] = [
        PlaceGroup {
            name: None,
            places: &[Place::Today],
        },
        PlaceGroup {
            name: Some("Descoberta"),
            places: &[Place::Opportunities, Place::Offers],
        },
        PlaceGroup {
            name: Some("Estoque"),
            places: &[Place::Products, Place::Purchases, Place::Stock],
        },
        PlaceGroup {
            name: Some("Canais de venda"),
            places: &[
                Place::Listings,
                Place::Quality,
                Place::Promotions,
                Place::Questions,
                Place::Reputation,
            ],
        },
        PlaceGroup {
            name: Some("Pedidos e finanças"),
            places: &[Place::Orders, Place::Sales, Place::Dashboard],
        },
        PlaceGroup {
            name: Some("Afiliação"),
            places: &[Place::Affiliate, Place::Promotion],
        },
    ];
    /// The places kept apart at the end: the sidebar's foot, the tab bar's
    /// right side.
    pub const PINNED: [Place; 1] = [Place::Settings];

    /// Whether the navigation lists this place in `context` (ADR 0037,
    /// decision 2). The sales screens show on the Sales Channels, and the
    /// rest everywhere: the stock is one, split across the Listings.
    pub fn shows_in(self, context: WorkContext) -> bool {
        if context == WorkContext::All {
            return true;
        }
        match self {
            Place::Listings | Place::Quality | Place::Promotions | Place::Orders | Place::Sales => {
                context.sales_channel().is_some()
            }
            // Only Mercado Livre's buyers ask questions and rate the seller
            // through a port the app reads.
            Place::Questions | Place::Reputation => {
                context.sales_channel() == Some(SalesChannelKind::MercadoLivre)
            }
            Place::Today
            | Place::Opportunities
            | Place::Offers
            | Place::Products
            | Place::Dashboard
            | Place::Purchases
            | Place::Stock
            | Place::Affiliate
            | Place::Promotion
            | Place::Settings => true,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Place::Today => "Hoje",
            Place::Opportunities => "Oportunidades",
            Place::Offers => "Ofertas",
            Place::Products => "Produtos",
            Place::Listings => "Anúncios",
            Place::Quality => "Qualidade",
            Place::Questions => "Perguntas",
            Place::Reputation => "Reputação",
            Place::Promotions => "Promoções",
            Place::Orders => "Pedidos",
            Place::Sales => "Vendas",
            Place::Dashboard => "Painel",
            Place::Purchases => "Compras",
            Place::Stock => "Estoque",
            Place::Affiliate => "Afiliação",
            Place::Promotion => "Divulgação",
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
            Place::Reputation => IconName::ThumbsUp,
            Place::Promotions => IconName::Calendar,
            Place::Orders => IconName::Bell,
            Place::Sales => IconName::ChartPie,
            Place::Dashboard => IconName::Frame,
            Place::Purchases => IconName::FileText,
            Place::Stock => IconName::GalleryVerticalEnd,
            Place::Affiliate => IconName::Network,
            Place::Promotion => IconName::Heart,
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
    /// Which places the navigation lists.
    pub context: WorkContext,
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

    const EVERY_PLACE: [Place; 17] = [
        Place::Today,
        Place::Opportunities,
        Place::Offers,
        Place::Products,
        Place::Listings,
        Place::Quality,
        Place::Questions,
        Place::Reputation,
        Place::Promotions,
        Place::Orders,
        Place::Sales,
        Place::Dashboard,
        Place::Purchases,
        Place::Stock,
        Place::Affiliate,
        Place::Promotion,
        Place::Settings,
    ];

    fn listed(place: Place) -> usize {
        Place::GROUPS
            .iter()
            .flat_map(|group| group.places)
            .chain(&Place::PINNED)
            .filter(|listed| **listed == place)
            .count()
    }

    #[test]
    fn the_navigation_lists_every_place_once() {
        for place in EVERY_PLACE {
            assert_eq!(listed(place), 1, "{}", place.name());
        }
    }

    #[test]
    fn only_the_first_group_goes_without_a_heading() {
        let [first, rest @ ..] = Place::GROUPS;
        assert_eq!(first.name, None);
        assert_eq!(first.places, [Place::Today]);
        for group in rest {
            assert!(group.name.is_some(), "{:?}", group.places);
            assert!(!group.places.is_empty(), "{:?}", group.name);
        }
    }

    #[test]
    fn places_sit_with_the_work_they_belong_to() {
        let group_of = |place: Place| {
            Place::GROUPS
                .iter()
                .find(|group| group.places.contains(&place))
                .and_then(|group| group.name)
        };
        assert_eq!(group_of(Place::Offers), Some("Descoberta"));
        assert_eq!(group_of(Place::Purchases), Some("Estoque"));
        assert_eq!(group_of(Place::Questions), Some("Canais de venda"));
        assert_eq!(group_of(Place::Dashboard), Some("Pedidos e finanças"));
        assert_eq!(group_of(Place::Promotion), Some("Afiliação"));
        assert_eq!(group_of(Place::Settings), None);
    }

    /// What ADR 0037 decision 2 lists in each context, in the navigation's
    /// order.
    fn expected(context: WorkContext) -> Vec<Place> {
        use Place::*;
        let everywhere = [
            Today,
            Opportunities,
            Offers,
            Products,
            Purchases,
            Stock,
            Dashboard,
            Affiliate,
            Promotion,
            Settings,
        ];
        let selling = [Listings, Quality, Promotions, Orders, Sales];
        let mercado_livre_only = [Questions, Reputation];
        EVERY_PLACE
            .into_iter()
            .filter(|place| match context {
                WorkContext::All => true,
                WorkContext::MercadoLivre => true,
                WorkContext::Shopee | WorkContext::Amazon | WorkContext::Magalu => {
                    everywhere.contains(place) || selling.contains(place)
                }
                WorkContext::Hotmart
                | WorkContext::Kiwify
                | WorkContext::Monetizze
                | WorkContext::Eduzz => everywhere.contains(place),
            })
            .inspect(|place| {
                assert!(
                    everywhere.contains(place)
                        || selling.contains(place)
                        || mercado_livre_only.contains(place),
                    "{} has no rule",
                    place.name()
                );
            })
            .collect()
    }

    fn navigation_in(context: WorkContext) -> Vec<Place> {
        let mut listed: Vec<Place> = Place::GROUPS
            .iter()
            .flat_map(|group| group.shown(context))
            .chain(
                Place::PINNED
                    .into_iter()
                    .filter(|place| place.shows_in(context)),
            )
            .collect();
        let order = |place: &Place| EVERY_PLACE.iter().position(|every| every == place);
        listed.sort_by_key(order);
        listed
    }

    #[test]
    fn each_context_lists_exactly_the_places_that_apply_to_it() {
        for context in WorkContext::ALL {
            assert_eq!(
                navigation_in(context),
                expected(context),
                "{}",
                context.name()
            );
        }
    }

    #[test]
    fn today_and_settings_show_in_every_context() {
        for context in WorkContext::ALL {
            assert!(Place::Today.shows_in(context), "{}", context.name());
            for place in Place::PINNED {
                assert!(place.shows_in(context), "{}", context.name());
            }
        }
    }

    #[test]
    fn a_context_that_only_promotes_hides_the_whole_sales_areas() {
        let named: Vec<&str> = Place::GROUPS
            .iter()
            .filter(|group| !group.shown(WorkContext::Hotmart).is_empty())
            .filter_map(|group| group.name)
            .collect();
        assert_eq!(
            named,
            ["Descoberta", "Estoque", "Pedidos e finanças", "Afiliação"]
        );
        assert_eq!(
            Place::GROUPS[4].shown(WorkContext::Hotmart),
            [Place::Dashboard]
        );
    }

    /// The app ships only gpui-kit's default icons: any other draws nothing.
    #[test]
    fn every_place_has_an_icon_the_app_ships() {
        for place in EVERY_PLACE {
            assert!(
                matches!(Assets.load(&place.icon().path()), Ok(Some(_))),
                "{} has no shipped icon",
                place.name()
            );
        }
    }
}
