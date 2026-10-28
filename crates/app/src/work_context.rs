//! The Work Context (ADR 0037): the Platform the owner is working on, picked
//! in the title bar, which the navigation and every screen follow. It
//! belongs to the interface: a screen hands the context's Sales Channel or
//! Platform to the module queries that already take one, and no module
//! knows the context itself.

use gpui_kit::assets::IconName;
use mascate_affiliate::{AffiliateLink, AffiliatePlatform, Commission, spends_on};
use mascate_commerce::SalesPeriod;
use mascate_kernel::{ChannelSpend, SalesChannelKind};

/// Everything, or one Platform. Mercado Livre, Shopee, Amazon and Magalu
/// are a Sales Channel and an affiliate Platform at once; the others are
/// affiliate Platforms only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WorkContext {
    #[default]
    All,
    MercadoLivre,
    Shopee,
    Amazon,
    Magalu,
    Hotmart,
    Kiwify,
    Monetizze,
    Eduzz,
}

impl WorkContext {
    /// In the order the selector lists them.
    pub const ALL: [WorkContext; 9] = [
        WorkContext::All,
        WorkContext::MercadoLivre,
        WorkContext::Shopee,
        WorkContext::Amazon,
        WorkContext::Magalu,
        WorkContext::Hotmart,
        WorkContext::Kiwify,
        WorkContext::Monetizze,
        WorkContext::Eduzz,
    ];

    /// Stable code, used in storage.
    pub fn code(self) -> &'static str {
        match self {
            WorkContext::All => "all",
            WorkContext::MercadoLivre => "mercado_livre",
            WorkContext::Shopee => "shopee",
            WorkContext::Amazon => "amazon",
            WorkContext::Magalu => "magalu",
            WorkContext::Hotmart => "hotmart",
            WorkContext::Kiwify => "kiwify",
            WorkContext::Monetizze => "monetizze",
            WorkContext::Eduzz => "eduzz",
        }
    }

    /// Reads the stored code; nothing saved or a code this version does not
    /// know gives All.
    pub fn from_code_or_default(code: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|context| Some(context.code()) == code)
            .unwrap_or_default()
    }

    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            WorkContext::All => "Tudo",
            WorkContext::MercadoLivre => "Mercado Livre",
            WorkContext::Shopee => "Shopee",
            WorkContext::Amazon => "Amazon",
            WorkContext::Magalu => "Magalu",
            WorkContext::Hotmart => "Hotmart",
            WorkContext::Kiwify => "Kiwify",
            WorkContext::Monetizze => "Monetizze",
            WorkContext::Eduzz => "Eduzz",
        }
    }

    /// The icon of what the context does: everything, sell and promote, or
    /// promote only, as the navigation draws Anúncios and Afiliação.
    pub fn icon(self) -> IconName {
        match (self, self.sales_channel()) {
            (WorkContext::All, _) => IconName::LayoutDashboard,
            (_, Some(_)) => IconName::Building2,
            (_, None) => IconName::Network,
        }
    }

    /// The Sales Channel the context filters by; `None` in All and on
    /// Platforms the owner only promotes.
    pub fn sales_channel(self) -> Option<SalesChannelKind> {
        match self {
            WorkContext::MercadoLivre => Some(SalesChannelKind::MercadoLivre),
            WorkContext::Shopee => Some(SalesChannelKind::Shopee),
            WorkContext::Amazon => Some(SalesChannelKind::Amazon),
            WorkContext::Magalu => Some(SalesChannelKind::Magalu),
            WorkContext::All
            | WorkContext::Hotmart
            | WorkContext::Kiwify
            | WorkContext::Monetizze
            | WorkContext::Eduzz => None,
        }
    }

    /// The affiliate Platform the context filters by; `None` in All, where
    /// a Program of a Platform without an API also shows.
    pub fn affiliate_platform(self) -> Option<AffiliatePlatform> {
        match self {
            WorkContext::All => None,
            WorkContext::MercadoLivre => Some(AffiliatePlatform::MercadoLivre),
            WorkContext::Shopee => Some(AffiliatePlatform::Shopee),
            WorkContext::Amazon => Some(AffiliatePlatform::Amazon),
            WorkContext::Magalu => Some(AffiliatePlatform::Magalu),
            WorkContext::Hotmart => Some(AffiliatePlatform::Hotmart),
            WorkContext::Kiwify => Some(AffiliatePlatform::Kiwify),
            WorkContext::Monetizze => Some(AffiliatePlatform::Monetizze),
            WorkContext::Eduzz => Some(AffiliatePlatform::Eduzz),
        }
    }

    /// Whether what belongs to the Sales Channel `kind` shows: everything
    /// in All, only its own in a Platform's context.
    pub fn shows_channel(self, kind: SalesChannelKind) -> bool {
        self == WorkContext::All || self.sales_channel() == Some(kind)
    }

    /// Whether what belongs to the affiliate Platform `platform` shows:
    /// everything in All, only its own in a Platform's context.
    pub fn shows_platform(self, platform: AffiliatePlatform) -> bool {
        self == WorkContext::All || self.affiliate_platform() == Some(platform)
    }

    /// Whether the context sells: All, or a Sales Channel's.
    pub fn sells(self) -> bool {
        self == WorkContext::All || self.sales_channel().is_some()
    }

    /// The part of `period` the context shows: all of it in All, its
    /// channel's sales and unplaced ads on a Sales Channel, nothing on a
    /// Platform the owner only promotes.
    pub fn sales_in(self, period: &SalesPeriod) -> SalesPeriod {
        match (self, self.sales_channel()) {
            (WorkContext::All, _) => period.clone(),
            (_, Some(kind)) => period.on(kind),
            (_, None) => SalesPeriod {
                sales: Vec::new(),
                unplaced_ads: Vec::new(),
            },
        }
    }

    /// The part of each Promotion Channel's spend the context shows (#196):
    /// all of it in All, the Platform's part elsewhere, as Affiliate weighs
    /// it against `commissions`, the period's of every Platform, and the
    /// live `links`.
    pub fn spends_in(
        self,
        spends: &[ChannelSpend],
        commissions: &[Commission],
        links: &[AffiliateLink],
    ) -> Vec<ChannelSpend> {
        match self.affiliate_platform() {
            None => spends.to_vec(),
            Some(platform) => spends_on(platform, spends, commissions, links),
        }
    }
}

/// Closes `open`, a form or a confirmation waiting on a screen, unless it
/// goes to a Sales Channel `context` shows: switching context never sends
/// what was open (ADR 0037, decision 4). One whose channel is not known
/// closes too.
pub fn close_outside<T>(
    open: &mut Option<T>,
    context: WorkContext,
    channel: impl FnOnce(&T) -> Option<SalesChannelKind>,
) {
    close_unless(open, |open| {
        channel(open).is_some_and(|kind| context.shows_channel(kind))
    });
}

/// Closes `open` unless it belongs to an affiliate Platform `context`
/// shows, as [`close_outside`] does for a Sales Channel's.
pub fn close_outside_platform<T>(
    open: &mut Option<T>,
    context: WorkContext,
    platform: impl FnOnce(&T) -> Option<AffiliatePlatform>,
) {
    close_unless(open, |open| {
        platform(open).is_some_and(|platform| context.shows_platform(platform))
    });
}

fn close_unless<T>(open: &mut Option<T>, keeps: impl FnOnce(&T) -> bool) {
    if !open.as_ref().is_some_and(keeps) {
        *open = None;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use gpui_kit::AssetSource;
    use gpui_kit::assets::Assets;
    use mascate_commerce::{ChannelShipments, OnSalesChannel, SentShipment};
    use mascate_kernel::{Currency, Money, PlatformError, RecordId, Timestamp};
    use rust_decimal::Decimal;

    use super::*;

    #[test]
    fn all_counts_every_spend_and_a_platform_only_its_part() {
        let spends = [ChannelSpend {
            channel: RecordId::from_u128(1),
            amount: Money::new(Decimal::from(50), Currency::Brl),
        }];

        assert_eq!(WorkContext::All.spends_in(&spends, &[], &[]), spends);
        // Nothing sold through the channel and it has no link to weigh by.
        assert!(WorkContext::Hotmart.spends_in(&spends, &[], &[]).is_empty());
    }

    /// A channel that counts each shipment it is told about.
    struct CountingShipments {
        kind: SalesChannelKind,
        sent: AtomicUsize,
    }

    impl OnSalesChannel for CountingShipments {
        fn sales_channel(&self) -> SalesChannelKind {
            self.kind
        }
    }

    impl ChannelShipments for CountingShipments {
        fn confirm_shipment(&self, _: &str, _: &SentShipment) -> Result<(), PlatformError> {
            self.sent.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// A shipment form as a screen holds it until the owner confirms.
    struct ShipmentForm {
        port: Arc<CountingShipments>,
        order: &'static str,
    }

    impl ShipmentForm {
        /// What the confirm button does: send what is still open.
        fn confirm(open: &mut Option<ShipmentForm>) {
            if let Some(form) = open.take() {
                let sent = SentShipment {
                    carrier: "Correios".into(),
                    tracking_code: "BR123".into(),
                    shipped_at: Timestamp::UNIX_EPOCH,
                };
                form.port.confirm_shipment(form.order, &sent).unwrap();
            }
        }
    }

    fn open_on(kind: SalesChannelKind) -> (Arc<CountingShipments>, Option<ShipmentForm>) {
        let port = Arc::new(CountingShipments {
            kind,
            sent: AtomicUsize::new(0),
        });
        let form = ShipmentForm {
            port: port.clone(),
            order: "2000001",
        };
        (port, Some(form))
    }

    fn channel_of(form: &ShipmentForm) -> Option<SalesChannelKind> {
        Some(form.port.sales_channel())
    }

    #[test]
    fn switching_context_with_an_open_form_sends_nothing_and_closes_it() {
        let (port, mut open) = open_on(SalesChannelKind::MercadoLivre);

        close_outside(&mut open, WorkContext::Shopee, channel_of);
        assert!(open.is_none());
        // The confirm button of the form that was open finds nothing.
        ShipmentForm::confirm(&mut open);

        assert_eq!(port.sent.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_form_on_a_channel_the_context_still_shows_stays_open_unsent() {
        for context in [WorkContext::All, WorkContext::MercadoLivre] {
            let (port, mut open) = open_on(SalesChannelKind::MercadoLivre);

            close_outside(&mut open, context, channel_of);

            assert!(open.is_some(), "{}", context.name());
            assert_eq!(port.sent.load(Ordering::SeqCst), 0);
            // Only the owner's click sends it, once.
            ShipmentForm::confirm(&mut open);
            ShipmentForm::confirm(&mut open);
            assert_eq!(port.sent.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn a_form_whose_channel_is_unknown_closes() {
        let (port, mut open) = open_on(SalesChannelKind::Amazon);

        close_outside(&mut open, WorkContext::All, |_| None);

        assert!(open.is_none());
        assert_eq!(port.sent.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn codes_round_trip_and_anything_else_reads_as_all() {
        for context in WorkContext::ALL {
            assert_eq!(
                WorkContext::from_code_or_default(Some(context.code())),
                context
            );
        }
        for code in [None, Some(""), Some("Hotmart"), Some("tiktok")] {
            assert_eq!(
                WorkContext::from_code_or_default(code),
                WorkContext::All,
                "{code:?}"
            );
        }
    }

    #[test]
    fn every_sales_channel_and_affiliate_platform_with_an_api_has_one_context() {
        for kind in SalesChannelKind::ALL {
            let contexts: Vec<WorkContext> = WorkContext::ALL
                .into_iter()
                .filter(|context| context.sales_channel() == Some(kind))
                .collect();
            assert_eq!(contexts.len(), 1, "{kind:?}");
        }
        for platform in AffiliatePlatform::ALL {
            let contexts = WorkContext::ALL
                .into_iter()
                .filter(|context| context.affiliate_platform() == Some(platform))
                .count();
            // A Program of a Platform without an API shows in All only.
            let expected = usize::from(platform != AffiliatePlatform::Other);
            assert_eq!(contexts, expected, "{platform:?}");
        }
    }

    #[test]
    fn a_context_is_named_after_its_platform() {
        for context in WorkContext::ALL {
            if let Some(kind) = context.sales_channel() {
                assert_eq!(context.name(), kind.name());
            }
            if let Some(platform) = context.affiliate_platform() {
                assert_eq!(context.name(), platform.name());
            }
        }
        assert_eq!(WorkContext::All.name(), "Tudo");
    }

    #[test]
    fn all_shows_every_channel_and_a_platform_only_its_own() {
        for kind in SalesChannelKind::ALL {
            assert!(WorkContext::All.shows_channel(kind));
            assert!(!WorkContext::Hotmart.shows_channel(kind));
        }
        assert!(WorkContext::Shopee.shows_channel(SalesChannelKind::Shopee));
        assert!(!WorkContext::Shopee.shows_channel(SalesChannelKind::MercadoLivre));
    }

    #[test]
    fn all_shows_every_affiliate_platform_and_a_platform_only_its_own() {
        for platform in AffiliatePlatform::ALL {
            assert!(WorkContext::All.shows_platform(platform));
        }
        for context in WorkContext::ALL.into_iter().skip(1) {
            let shown: Vec<AffiliatePlatform> = AffiliatePlatform::ALL
                .into_iter()
                .filter(|&platform| context.shows_platform(platform))
                .collect();
            assert_eq!(shown, [context.affiliate_platform().unwrap()]);
        }
    }

    #[test]
    fn a_form_of_a_platform_the_new_context_does_not_show_closes() {
        for (context, kept) in [
            (WorkContext::All, true),
            (WorkContext::Hotmart, true),
            (WorkContext::Kiwify, false),
            (WorkContext::MercadoLivre, false),
        ] {
            let mut open = Some(AffiliatePlatform::Hotmart);
            close_outside_platform(&mut open, context, |platform| Some(*platform));
            assert_eq!(open.is_some(), kept, "{}", context.name());
        }
        let mut unknown = Some(());
        close_outside_platform(&mut unknown, WorkContext::All, |_| None);
        assert!(unknown.is_none());
    }

    #[test]
    fn only_all_and_the_sales_channels_sell() {
        let selling: Vec<WorkContext> = WorkContext::ALL
            .into_iter()
            .filter(|context| context.sells())
            .collect();
        assert_eq!(
            selling,
            [
                WorkContext::All,
                WorkContext::MercadoLivre,
                WorkContext::Shopee,
                WorkContext::Amazon,
                WorkContext::Magalu,
            ]
        );
    }

    /// The app ships only gpui-kit's default icons: any other draws nothing.
    #[test]
    fn every_context_has_an_icon_the_app_ships() {
        for context in WorkContext::ALL {
            assert!(
                matches!(Assets.load(&context.icon().path()), Ok(Some(_))),
                "{} has no shipped icon",
                context.name()
            );
        }
    }
}
