//! Product Ads (#27) as the screens share them: read once a day after an
//! Order Sync while the Reputation unlocks them, their cost handed to
//! Commerce for each sale's Realized Margin, and each Product's ROAS
//! against its break-even, worked out from Commerce's prices and summed up
//! by Marketing (ADR 0025). Ad Campaigns change only on the owner's
//! confirmed click, in Vendas (#180, ADR 0036).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::{App, Global, Task};
use mascate_catalog::Catalog;
use mascate_commerce::{AdCost, Listings, Pricing};
use mascate_finance::Taxes;
use mascate_integrations::{Connection, ConnectionState, MercadoLivre};
use mascate_kernel::{RecordId, Timestamp};
use mascate_marketing::{
    AdCampaigns, AdsError, AdsReport, AdsSync, AdvertisedListing, AdvertisedProducts, ProductAds,
    Reputation, SellerTool,
};

use crate::catalog;
use crate::connections::AppConnections;
use crate::listings::{self, listings};
use crate::mercado_livre;
use crate::pricing;
use crate::reputation::{self, reputation};

/// The app's Product Ads; absent when the database did not open.
pub struct AppProductAds(pub Arc<ProductAds>);

impl Global for AppProductAds {}

pub(crate) fn product_ads(cx: &App) -> Option<Arc<ProductAds>> {
    cx.try_global::<AppProductAds>().map(|app| app.0.clone())
}

/// The app's Ad Campaigns; absent when the database did not open.
pub struct AppAdCampaigns(pub Arc<AdCampaigns>);

impl Global for AppAdCampaigns {}

pub(crate) fn ad_campaigns(cx: &App) -> Option<Arc<AdCampaigns>> {
    cx.try_global::<AppAdCampaigns>().map(|app| app.0.clone())
}

/// The Syncs of Product Ads since the app opened; the screens that show
/// them read them again after each.
#[derive(Default)]
pub struct AdsSyncs {
    pub finished: u64,
}

impl Global for AdsSyncs {}

pub(crate) fn failure(error: &AdsError) -> String {
    match error {
        AdsError::Platform(error) => mercado_livre::failure(error),
        AdsError::Currencies(_) => "Os valores do Product Ads estão em moedas diferentes.".into(),
        AdsError::Locked => format!(
            "Campanhas pedem {}; sua reputação ainda não chegou lá. Veja em Reputação.",
            SellerTool::ProductAds.requirement()
        ),
        AdsError::NoAccount => "O Mercado Livre ainda não tem uma conta de Product Ads para você. \
             Ative o Product Ads em Minha conta › Publicidade no Mercado Livre e sincronize de novo."
            .into(),
        AdsError::NoName => "Dê um nome à campanha.".into(),
        AdsError::NameTaken => "Já existe uma campanha com esse nome; escolha outro.".into(),
        AdsError::NotRunnable => "Uma campanha fica ativa ou pausada.".into(),
        AdsError::InvalidBudget => "Digite o orçamento diário em reais, maior que zero, como 20,00."
            .into(),
        AdsError::AboveMaximumBudget(maximum) => format!(
            "O orçamento diário passa do teto de {}. Para gastar mais por dia, suba o teto em \
             Configurações › Vendas › Product Ads antes.",
            maximum.to_pt_br()
        ),
        AdsError::InvalidMaximum => "O teto do orçamento diário precisa ser maior que zero.".into(),
        AdsError::TargetOutOfRange => {
            "O Mercado Livre aceita ROAS alvo de 1 a 35, como 4,5.".into()
        }
        AdsError::UnknownCampaign => "Essa campanha não existe mais; atualize a tela.".into(),
        AdsError::NotManaged => "O Mercado Livre não deixa a sua conta criar nem mudar campanhas \
             pela API (ou a campanha não existe mais lá). Faça a mudança no painel do Mercado \
             Livre, em Publicidade › Product Ads; a leitura do custo e do ROAS continua."
            .into(),
        AdsError::Sending => {
            "Essa mudança já está a caminho do Mercado Livre; aguarde a resposta.".into()
        }
        error => format!("Não consegui ler ou gravar o Product Ads no banco: {error}"),
    }
}

/// Whether the Reputation, as last read, unlocks Product Ads; `None`
/// before it was ever read.
pub(crate) async fn unlocked(reputation: &Reputation) -> Result<Option<bool>, String> {
    Ok(reputation
        .standing()
        .await
        .map_err(|e| reputation::failure(&e))?
        .map(|standing| {
            standing
                .tools
                .iter()
                .any(|(tool, unlocked)| *tool == SellerTool::ProductAds && *unlocked)
        }))
}

/// Runs a Sync of Product Ads off the UI thread, once a day or when
/// `forced`, and only while the Reputation unlocks them. `Ok(None)` when
/// the Connection is not up, Product Ads are locked or nothing was due.
pub fn sync_now(cx: &mut App, forced: bool) -> Option<Task<Result<Option<AdsSync>, String>>> {
    let (Some(ads), Some(channel), Some(reputation)) =
        (product_ads(cx), mercado_livre::adapter(cx), reputation(cx))
    else {
        return None;
    };
    let connections = cx.global::<AppConnections>().0.clone();
    Some(cx.background_executor().spawn(async move {
        if connections.state(Connection::MercadoLivre) != ConnectionState::Connected
            || unlocked(&reputation).await? != Some(true)
        {
            return Ok(None);
        }
        if !forced && !ads.is_due().await.map_err(|e| failure(&e))? {
            return Ok(None);
        }
        ads.sync(channel.as_ref())
            .await
            .map(Some)
            .map_err(|e| failure(&e))
    }))
}

/// Tells the screens a Sync ran.
pub fn finished(outcome: &Result<Option<AdsSync>, String>, cx: &mut App) {
    if let Err(error) = outcome {
        eprintln!("could not sync the Product Ads: {error}");
    }
    cx.default_global::<AdsSyncs>().finished += 1;
}

/// What Product Ads cost, by listing and day, as Commerce takes it into
/// the Realized Margin; nothing without the app's Product Ads.
pub(crate) async fn costs(ads: Option<&ProductAds>) -> Result<Vec<AdCost>, String> {
    let Some(ads) = ads else {
        return Ok(Vec::new());
    };
    Ok(ads
        .costs()
        .await
        .map_err(|e| failure(&e))?
        .into_iter()
        .map(|day| AdCost {
            listing: day.listing,
            day: day.day,
            cost: day.cost,
        })
        .collect())
}

/// What the report needs from the rest of the app.
pub(crate) struct ReportSources {
    pub ads: Arc<ProductAds>,
    pub listings: Arc<Listings>,
    pub catalog: Arc<Catalog>,
    pub pricing: Arc<Pricing>,
    pub taxes: Arc<Taxes>,
    /// `None` while the Connection is not up: the break-even needs the
    /// channel's sale fee.
    pub channel: Option<Arc<MercadoLivre>>,
}

/// What the report needs, from the app's globals; `None` when the
/// database did not open.
pub(crate) fn report_sources(cx: &App) -> Option<ReportSources> {
    let connected = cx
        .global::<AppConnections>()
        .0
        .state(Connection::MercadoLivre)
        == ConnectionState::Connected;
    Some(ReportSources {
        ads: product_ads(cx)?,
        listings: listings(cx)?,
        catalog: catalog::catalog(cx)?,
        pricing: pricing::pricing(cx)?,
        taxes: pricing::taxes(cx)?,
        channel: mercado_livre::adapter(cx).filter(|_| connected),
    })
}

/// The ads of the days that start `during`, by campaign and by Product,
/// each advertised listing with its Product and the break-even at today's
/// price. A break-even that cannot be worked out (no Average Cost, no
/// Connection) is left unknown.
pub(crate) async fn report(
    sources: &ReportSources,
    during: Range<Timestamp>,
) -> Result<AdsReport, String> {
    let ads = &sources.ads;
    let advertised = ads
        .report(during.clone(), &[])
        .await
        .map_err(|e| failure(&e))?;
    if advertised.total.is_none() {
        return Ok(advertised);
    }
    let items: BTreeSet<&String> = advertised
        .products
        .iter()
        .flat_map(|row| &row.listings)
        .collect();
    let known = known_listings(sources, items).await?;
    ads.report(during, &known).await.map_err(|e| failure(&e))
}

/// The Products each campaign advertised in the days read, with the
/// break-even at today's price, from which a campaign's Target ROAS is
/// suggested.
pub(crate) async fn advertised_products(
    sources: &ReportSources,
) -> Result<AdvertisedProducts, String> {
    let ads = &sources.ads;
    let costs = ads.costs().await.map_err(|e| failure(&e))?;
    let items: BTreeSet<&String> = costs.iter().map(|day| &day.listing).collect();
    let known = known_listings(sources, items).await?;
    ads.advertised_products(&known)
        .await
        .map_err(|e| failure(&e))
}

/// Each of the channel's `items` the app knows, with its Product and its
/// break-even at today's price.
async fn known_listings(
    sources: &ReportSources,
    items: BTreeSet<&String>,
) -> Result<Vec<AdvertisedListing>, String> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let sold = listings::listing_products(&sources.listings, &sources.catalog).await?;
    let mut records: BTreeMap<String, RecordId> = BTreeMap::new();
    for listing in listings::mercado_livre_listings(&sources.listings).await? {
        records.entry(listing.listed.id).or_insert(listing.id);
    }
    let assumptions = pricing::assumptions(&sources.catalog, &sources.taxes).await?;
    let mut known = Vec::new();
    for item in items {
        let Some(product) = sold.get(item) else {
            continue;
        };
        let break_even = match (&sources.channel, records.get(item)) {
            (Some(channel), Some(&record)) => sources
                .pricing
                .break_even(record, channel.as_ref(), assumptions)
                .await
                .ok(),
            _ => None,
        };
        known.push(AdvertisedListing {
            listing: item.clone(),
            product: product
                .product
                .map_or_else(|| item.clone(), |product| product.to_string()),
            name: product.name.clone(),
            break_even,
        });
    }
    Ok(known)
}
