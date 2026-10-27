// Release builds on Windows run without a console window behind the app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ad_campaigns;
mod ad_groups;
mod ads;
mod affiliate;
mod affiliate_links;
mod affiliate_panels;
mod amazon;
mod amazon_import;
mod appearance;
mod backups;
mod brand;
mod catalog;
mod commission_dashboard;
mod connections;
mod crawler_search;
mod crawlers;
mod credential_guides;
mod dashboard;
mod drafts;
mod eduzz;
mod event_alerts;
mod finance;
mod forms;
mod found_offers;
mod home;
mod hotmart;
mod instagram;
mod kit;
mod kiwify;
mod layout;
mod listing_copy;
mod listings;
mod low_stock;
mod magalu;
mod mercado_livre;
mod merge_import;
mod monetizze;
mod offer_posts;
mod offers;
mod opportunities;
mod order_alerts;
mod ordered_saves;
mod orders;
mod palette;
mod parts;
mod payouts;
mod pinterest;
mod post_destinations;
mod post_queue;
mod preferences;
mod pricing;
mod product_links;
mod products;
mod program_joins;
mod promotion_channels;
mod promotion_rules;
mod promotions;
mod purchases;
mod quality;
mod question_alerts;
mod questions;
mod reminders;
mod repricing;
mod reputation;
mod reputation_alerts;
mod resell_or_promote;
mod restricted;
mod sales;
mod sales_channels;
mod sales_events;
mod settings;
mod shell;
mod shopee;
mod shopee_search;
mod shopee_seller;
mod short_io;
mod span;
mod startup;
mod stock;
mod stock_mirror;
mod telegram;
#[cfg(test)]
mod testing;
mod title_bar;
mod tray;
mod updates;
mod work_context;

use std::sync::Arc;

use futures::StreamExt;
use gpui_kit::*;
use mascate_affiliate::{CommissionSource, LinkTagger};
use mascate_integrations::{
    AMAZON_LINKS, Amazon, Anthropic, Connection, Connections, Crawler, EDUZZ_LINKS, Eduzz,
    HOTMART_LINKS, Hotmart, Instagram, KIWIFY_LINKS, Kiwify, MONETIZZE_LINKS, Magalu, MercadoLivre,
    Monetizze, Pinterest, ShopeeAffiliates, ShopeeSeller, ShortIo, Telegram, installed_browser,
};
use mascate_kernel::SystemClock;
use mascate_platform::{
    Build, Finish, SystemSecretStore, default_database_path, process_environment,
    run_installer_after_exit, secret_store_for, system_user,
};

use crate::ads::{AppAdCampaigns, AppProductAds};
use crate::affiliate::{AppAffiliate, CommissionSources};
use crate::affiliate_links::LinkTaggers;
use crate::affiliate_panels::{AppAffiliatePanels, PanelSite, Panels};
use crate::backups::{AppBackups, UnopenedDatabase};
use crate::catalog::AppCatalog;
use crate::connections::AppConnections;
use crate::crawlers::{AppCrawlers, CrawledSite};
use crate::finance::AppSalesVolume;
use crate::instagram::AppInstagram;
use crate::listing_copy::AppListingCopy;
use crate::listings::AppListings;
use crate::magalu::AppMagalu;
use crate::mercado_livre::AppMercadoLivre;
use crate::offer_posts::AppOfferPosts;
use crate::orders::AppOrders;
use crate::pinterest::AppPinterest;
use crate::preferences::Preferences;
use crate::pricing::{AppPricing, AppTaxes};
use crate::promotion_channels::AppPromotionChannels;
use crate::promotions::AppPromotions;
use crate::purchases::AppPurchaseOrders;
use crate::quality::AppQuality;
use crate::questions::AppQuestions;
use crate::reminders::AppReminders;
use crate::repricing::AppRepricing;
use crate::reputation::AppReputation;
use crate::restricted::AppFlags;
use crate::sales_channels::{AppChannelRanking, ChannelPorts, ChannelRepricing, SalesChannels};
use crate::sales_events::AppSalesEvents;
use crate::shell::Shell;
use crate::shopee::AppShopee;
use crate::shopee_seller::AppShopeeSeller;
use crate::short_io::AppShortIo;
use crate::stock::AppInventory;
use crate::stock_mirror::AppStockMirror;
use crate::telegram::AppTelegram;
use crate::tray::TrayCommand;

fn main() {
    // The release workflow reads this to write the update manifest.
    if std::env::args().nth(1).as_deref() == Some("--risky-migrations") {
        println!("{}", startup::risky_migrations_json());
        return;
    }
    let updater = startup::updater().map(Arc::new);
    // A silent update left for this start: its installer runs once the app
    // has exited, and opens the new version.
    if let Some(Finish::RunInstaller(installer)) =
        updater.as_ref().and_then(|updater| updater.take_staged())
    {
        match run_installer_after_exit(&installer) {
            Ok(()) => return,
            Err(error) => eprintln!("could not run the staged update: {error}"),
        }
    }
    let startup = startup::prepare();
    let secrets = secret_store_for(
        Build::CURRENT,
        Arc::new(SystemSecretStore::default()),
        process_environment(),
    );
    let connections = Arc::new(Connections::new(secrets.clone()));
    let user_agent = format!("Mascate/{}", env!("CARGO_PKG_VERSION"));
    let shopee = Arc::new(ShopeeAffiliates::new(
        &shopee::api_url(Build::CURRENT, &process_environment()),
        &user_agent,
        secrets.clone(),
        Arc::new(SystemClock),
    ));
    let (hotmart_api, hotmart_auth) = hotmart::urls(Build::CURRENT, &process_environment());
    let hotmart = Arc::new(Hotmart::new(
        &hotmart_api,
        &hotmart_auth,
        &user_agent,
        secrets.clone(),
        Arc::new(SystemClock),
    ));
    let monetizze = Arc::new(Monetizze::new(
        &monetizze::api_url(Build::CURRENT, &process_environment()),
        &user_agent,
        secrets.clone(),
        Arc::new(SystemClock),
    ));
    let kiwify = Arc::new(Kiwify::new(
        &kiwify::api_url(Build::CURRENT, &process_environment()),
        &user_agent,
        secrets.clone(),
        Arc::new(SystemClock),
    ));
    let eduzz = Arc::new(Eduzz::new(
        &eduzz::api_url(Build::CURRENT, &process_environment()),
        &user_agent,
        secrets.clone(),
    ));
    let telegram = Arc::new(Telegram::new(
        &telegram::api_url(Build::CURRENT, &process_environment()),
        &user_agent,
        secrets.clone(),
    ));
    let (amazon_api, amazon_auth) = amazon::urls(Build::CURRENT, &process_environment());
    let amazon = Arc::new(Amazon::new(
        &amazon_api,
        &amazon_auth,
        &user_agent,
        secrets.clone(),
        Arc::new(SystemClock),
    ));
    let shopee_seller = Arc::new(ShopeeSeller::new(
        &shopee_seller::api_url(Build::CURRENT, &process_environment()),
        &user_agent,
        secrets.clone(),
        Arc::new(SystemClock),
    ));
    let (magalu_api, magalu_id) = magalu::urls(Build::CURRENT, &process_environment());
    let magalu = Arc::new(Magalu::new(
        &magalu_api,
        &magalu_id,
        &user_agent,
        secrets.clone(),
        Arc::new(SystemClock),
    ));
    let (pinterest_api, pinterest_login) = pinterest::urls(Build::CURRENT, &process_environment());
    let pinterest = Arc::new(Pinterest::new(
        &pinterest_api,
        &pinterest_login,
        &user_agent,
        secrets.clone(),
        Arc::new(SystemClock),
    ));
    let instagram = Arc::new(Instagram::new(
        &instagram::api_url(Build::CURRENT, &process_environment()),
        &user_agent,
        secrets.clone(),
        Arc::new(SystemClock),
    ));
    let short_io = Arc::new(ShortIo::new(
        &short_io::api_url(Build::CURRENT, &process_environment()),
        &short_io::statistics_url(Build::CURRENT, &process_environment()),
        &user_agent,
        secrets.clone(),
    ));
    let anthropic = Arc::new(Anthropic::new(
        &listing_copy::api_url(Build::CURRENT, &process_environment()),
        &user_agent,
        secrets.clone(),
    ));
    let mercado_livre = Arc::new(
        MercadoLivre::new(
            &mercado_livre::api_url(Build::CURRENT, &process_environment()),
            &user_agent,
            secrets,
            Arc::new(SystemClock),
        )
        .with_payments_api(&mercado_livre::payments_api_url(
            Build::CURRENT,
            &process_environment(),
        )),
    );
    // The engine exists from the start so its memory and breakers outlive
    // each screen; no crawler is built until its flag is on.
    let crawlers = AppCrawlers {
        engine: Arc::new(Crawler::new(&user_agent, Arc::new(SystemClock))),
        sites: CrawledSite::all(Build::CURRENT, &process_environment()),
    };
    // The browser opens nothing until a panel whose flag is on reads, on a
    // profile beside the database.
    let affiliate_panels = default_database_path()
        .and_then(|database| database.parent().map(std::path::Path::to_path_buf))
        .map(|data| {
            AppAffiliatePanels(Arc::new(Panels {
                browser: Arc::new(mascate_integrations::BrowserSession::new(
                    installed_browser(&*process_environment()),
                    affiliate_panels::profile_folder(data),
                )),
                sites: PanelSite::all(Build::CURRENT, &process_environment()),
            }))
        });
    let user: SharedString = system_user(&process_environment())
        .unwrap_or_else(|| "usuário do sistema".into())
        .into();
    let (started, saved, problem) = match startup {
        Ok(started) => {
            let saved = started.appearance;
            (Some(started), saved, None)
        }
        Err(problem) => (None, Default::default(), Some(problem)),
    };
    let problem_text = problem
        .as_ref()
        .map(|problem| SharedString::from(problem.message.clone()));

    // gpui-kit's default icon set and the logo; without them icons draw nothing.
    gpui_kit::application()
        .with_assets(brand::Assets)
        .run(move |cx| {
            // Windows shows system notifications only for an app with an
            // identity; the installer's identifier is that identity.
            cx.set_app_identity("app.mascate.desktop", "Mascate");
            gpui_kit::init(cx);
            cx.set_global(Preferences::new(
                started.as_ref().map(|started| started.database.clone()),
                saved,
                started
                    .as_ref()
                    .map(|started| started.work_context)
                    .unwrap_or_default(),
            ));
            cx.set_global(AppConnections(connections));
            cx.set_global(SalesChannels(vec![
                ChannelPorts {
                    connection: Connection::MercadoLivre,
                    listings: mercado_livre.clone(),
                    orders: Some(mercado_livre.clone()),
                    billing: Some(mercado_livre.clone()),
                    payments: Some(mercado_livre.clone()),
                    labels: Some(mercado_livre.clone()),
                    shipments: None,
                    publisher: Some(mercado_livre.clone()),
                    // Mercado Livre reprices the listing itself within the
                    // limits.
                    repricing: Some(ChannelRepricing::Automation(mercado_livre.clone())),
                    quality: Some(mercado_livre.clone()),
                    promotions: Some(mercado_livre.clone()),
                    // Product Ads.
                    ads: Some(mercado_livre.clone()),
                },
                // The owner sends Amazon's Orders by a carrier of their
                // own and confirms each shipment here: no label to download.
                ChannelPorts {
                    connection: Connection::Amazon,
                    listings: amazon.clone(),
                    orders: Some(amazon.clone()),
                    billing: Some(amazon.clone()),
                    payments: None,
                    labels: None,
                    shipments: Some(amazon.clone()),
                    // A draft goes up as an offer on an ASIN found by the
                    // GTIN.
                    publisher: Some(amazon.clone()),
                    // The app follows the featured offer after each Sync.
                    repricing: Some(ChannelRepricing::FeaturedOffer(amazon.clone())),
                    // The listing's issues; Amazon gives no score.
                    quality: Some(amazon),
                    // Amazon offers sellers no Promotion by its API, and its
                    // Ads API is apart from SP-API (ADR 0036).
                    promotions: None,
                    ads: None,
                },
                // Shopee prints the label once the shipment is arranged, and
                // the label arranges it: no shipment to confirm.
                ChannelPorts {
                    connection: Connection::ShopeeSeller,
                    listings: shopee_seller.clone(),
                    orders: Some(shopee_seller.clone()),
                    billing: Some(shopee_seller.clone()),
                    payments: Some(shopee_seller.clone()),
                    labels: Some(shopee_seller.clone()),
                    shipments: None,
                    // A draft goes up with its pictures, then its models.
                    publisher: Some(shopee_seller.clone()),
                    // Shopee and Magalu tell no competitor's price.
                    repricing: None,
                    // The listings with issues and the shop's performance.
                    quality: Some(shopee_seller.clone()),
                    // The owner's discounts.
                    promotions: Some(shopee_seller.clone()),
                    // Shopee Ads, read once Shopee grants the permission.
                    ads: Some(shopee_seller.clone()),
                },
                // The owner sends the NF-e and ships in Magalu's panel:
                // nothing to print or confirm here. Publishing is out of the
                // epic.
                ChannelPorts {
                    connection: Connection::Magalu,
                    listings: magalu.clone(),
                    orders: Some(magalu.clone()),
                    billing: Some(magalu.clone()),
                    payments: None,
                    labels: None,
                    shipments: None,
                    publisher: None,
                    repricing: None,
                    // Magalu's API tells no listing quality and offers
                    // sellers no Promotion, and Magalu Ads has no public API.
                    quality: None,
                    promotions: None,
                    ads: None,
                },
            ]));
            cx.set_global(AppShopeeSeller(shopee_seller));
            cx.set_global(AppMagalu(magalu));
            cx.set_global(AppMercadoLivre(mercado_livre));
            cx.set_global(AppShopee(shopee.clone()));
            cx.set_global(AppTelegram(telegram.clone()));
            cx.set_global(AppPinterest(pinterest));
            cx.set_global(AppInstagram(instagram));
            cx.set_global(AppShortIo(short_io));
            if let Some(started) = &started {
                cx.set_global(AppFlags {
                    flags: started.flags.clone(),
                    user: user.clone(),
                });
                cx.set_global(AppReminders(started.reminders.clone()));
                cx.set_global(AppAffiliate(started.affiliate.clone()));
                cx.set_global(CommissionSources(vec![
                    (
                        Connection::Hotmart,
                        hotmart.clone() as Arc<dyn CommissionSource>,
                    ),
                    (
                        Connection::ShopeeAffiliates,
                        shopee.clone() as Arc<dyn CommissionSource>,
                    ),
                    (
                        Connection::Monetizze,
                        monetizze.clone() as Arc<dyn CommissionSource>,
                    ),
                    (
                        Connection::Kiwify,
                        kiwify.clone() as Arc<dyn CommissionSource>,
                    ),
                    (
                        Connection::Eduzz,
                        eduzz.clone() as Arc<dyn CommissionSource>,
                    ),
                ]));
                cx.set_global(LinkTaggers(
                    [
                        HOTMART_LINKS,
                        KIWIFY_LINKS,
                        MONETIZZE_LINKS,
                        EDUZZ_LINKS,
                        AMAZON_LINKS,
                    ]
                    .into_iter()
                    .map(|tagger| Arc::new(tagger) as Arc<dyn LinkTagger>)
                    .chain([shopee.clone() as Arc<dyn LinkTagger>])
                    .collect(),
                ));
                cx.set_global(AppPromotionChannels(started.promotion_channels.clone()));
                cx.set_global(AppOfferPosts(started.offer_posts.clone()));
                cx.set_global(AppSalesEvents(started.sales_events.clone()));
                cx.set_global(AppCatalog(started.catalog.clone()));
                cx.set_global(crawlers);
                if let Some(affiliate_panels) = affiliate_panels {
                    cx.set_global(affiliate_panels);
                }
                cx.set_global(AppTaxes(started.taxes.clone()));
                cx.set_global(AppSalesVolume(started.sales_volume.clone()));
                cx.set_global(AppPricing(started.pricing.clone()));
                cx.set_global(AppInventory(started.inventory.clone()));
                cx.set_global(AppPurchaseOrders(started.purchase_orders.clone()));
                cx.set_global(AppListings(started.listings.clone()));
                cx.set_global(AppStockMirror(started.stock_mirror.clone()));
                cx.set_global(AppChannelRanking(started.channel_ranking.clone()));
                cx.set_global(AppOrders(started.orders.clone()));
                cx.set_global(AppQuality(started.quality.clone()));
                cx.set_global(AppListingCopy {
                    copy: started.listing_copy.clone(),
                    writer: anthropic.clone(),
                });
                cx.set_global(AppQuestions {
                    questions: started.questions.clone(),
                    templates: started.reply_templates.clone(),
                });
                cx.set_global(AppReputation(started.reputation.clone()));
                cx.set_global(AppPromotions(started.promotions.clone()));
                cx.set_global(AppRepricing(started.repricing.clone()));
                cx.set_global(AppProductAds(started.product_ads.clone()));
                cx.set_global(AppAdCampaigns(started.ad_campaigns.clone()));
                cx.set_global(AppBackups {
                    backups: started.backups.clone(),
                    merge_import: started.merge_import.clone(),
                    settings: started.backup_settings.clone(),
                    restore: started.restore.clone(),
                    merged: started.merged.clone(),
                    daily_failure: Default::default(),
                });
                backups::start_daily(cx);
                orders::start_polling(cx);
                post_queue::start(cx);
                affiliate::start_syncing(cx);
            } else if let Some(path) = problem
                .as_ref()
                .and_then(|problem| problem.database_path.clone())
            {
                cx.set_global(UnopenedDatabase(path));
            }
            if let Some(updater) = &updater {
                let failed_to_migrate = problem
                    .as_ref()
                    .is_some_and(|problem| problem.failed_to_migrate);
                updates::init(
                    updater.clone(),
                    started.as_ref().map(|started| started.database.clone()),
                    started
                        .as_ref()
                        .map(|started| started.update_settings)
                        .unwrap_or_default(),
                    failed_to_migrate,
                    cx,
                );
                if failed_to_migrate {
                    updates::go_back(cx);
                } else {
                    updates::start_checking(cx);
                }
            }
            appearance::init(saved.theme, cx);
            layout::show(saved.layout, cx);

            match tray::start() {
                Ok((tray, mut commands)) => {
                    // Closing the window only hides the app; "Sair" in the tray quits.
                    cx.set_quit_mode(QuitMode::Explicit);
                    cx.set_global(tray);
                    let problem = problem_text.clone();
                    cx.spawn(async move |cx| {
                        while let Some(command) = commands.next().await {
                            cx.update(|cx| match command {
                                TrayCommand::Open => show_home(problem.clone(), cx),
                                TrayCommand::Quit => cx.quit(),
                            });
                        }
                    })
                    .detach();
                }
                // Without a tray there would be no way back to a closed window.
                Err(error) => {
                    eprintln!("tray unavailable, closing the window will quit: {error}");
                    cx.set_quit_mode(QuitMode::LastWindowClosed);
                }
            }

            // Clicking a notification (a new Order, low stock) brings the window back.
            let problem = problem_text.clone();
            cx.on_system_notification_response(move |_, cx| show_home(problem.clone(), cx));

            show_home(problem_text, cx);
        });
}

/// Brings the home window to the front, opening it if it was closed.
fn show_home(problem: Option<SharedString>, cx: &mut App) {
    if let Some(window) = cx.windows().into_iter().next() {
        let _ = window.update(cx, |_, window, _| window.activate_window());
        cx.activate(true);
        return;
    }
    let options = WindowOptions {
        // Mascate draws the title bar in the interface theme (`title_bar`);
        // the system keeps the title for the taskbar.
        titlebar: Some(TitlebarOptions {
            title: Some("Mascate".into()),
            appears_transparent: true,
            ..Default::default()
        }),
        app_owns_titlebar_drag: true,
        window_bounds: Some(WindowBounds::centered(size(px(1100.), px(720.)), cx)),
        app_id: Some("mascate".into()),
        ..Default::default()
    };
    if let Err(error) = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| Shell::new(problem, window, cx))
    }) {
        eprintln!("could not open the window: {error}");
        cx.quit();
    }
}
