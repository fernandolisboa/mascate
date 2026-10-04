// Release builds on Windows run without a console window behind the app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod appearance;
mod backups;
mod catalog;
mod connections;
mod drafts;
mod forms;
mod home;
mod kit;
mod layout;
mod listings;
mod low_stock;
mod mercado_livre;
mod offers;
mod opportunities;
mod ordered_saves;
mod palette;
mod parts;
mod preferences;
mod pricing;
mod products;
mod purchases;
mod reminders;
mod restricted;
mod settings;
mod shell;
mod startup;
mod stock;
mod stock_mirror;
mod tray;
mod updates;

use std::sync::Arc;

use futures::StreamExt;
use gpui_kit::*;
use mascate_integrations::{Connections, MercadoLivre};
use mascate_kernel::SystemClock;
use mascate_platform::{
    Build, Finish, SystemSecretStore, process_environment, run_installer_after_exit,
    secret_store_for, system_user,
};

use crate::backups::{AppBackups, UnopenedDatabase};
use crate::catalog::AppCatalog;
use crate::connections::AppConnections;
use crate::listings::AppListings;
use crate::mercado_livre::AppMercadoLivre;
use crate::preferences::Preferences;
use crate::pricing::{AppPricing, AppTaxes};
use crate::purchases::AppPurchaseOrders;
use crate::reminders::AppReminders;
use crate::restricted::AppFlags;
use crate::shell::Shell;
use crate::stock::AppInventory;
use crate::stock_mirror::AppStockMirror;
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
    let mercado_livre = Arc::new(MercadoLivre::new(
        &mercado_livre::api_url(Build::CURRENT, &process_environment()),
        &format!("Mascate/{}", env!("CARGO_PKG_VERSION")),
        secrets,
        Arc::new(SystemClock),
    ));
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

    // gpui-kit's default icon set; without it icons draw nothing.
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            // Windows shows system notifications only for an app with an
            // identity; the installer's identifier is that identity.
            cx.set_app_identity("app.mascate.desktop", "Mascate");
            gpui_kit::init(cx);
            cx.set_global(Preferences::new(
                started.as_ref().map(|started| started.database.clone()),
                saved,
            ));
            cx.set_global(AppConnections(connections));
            cx.set_global(AppMercadoLivre(mercado_livre));
            if let Some(started) = &started {
                cx.set_global(AppFlags {
                    flags: started.flags.clone(),
                    user: user.clone(),
                });
                cx.set_global(AppReminders(started.reminders.clone()));
                cx.set_global(AppCatalog(started.catalog.clone()));
                cx.set_global(AppTaxes(started.taxes.clone()));
                cx.set_global(AppPricing(started.pricing.clone()));
                cx.set_global(AppInventory(started.inventory.clone()));
                cx.set_global(AppPurchaseOrders(started.purchase_orders.clone()));
                cx.set_global(AppListings(started.listings.clone()));
                cx.set_global(AppStockMirror(started.stock_mirror.clone()));
                cx.set_global(AppBackups {
                    backups: started.backups.clone(),
                    settings: started.backup_settings.clone(),
                    restore: started.restore.clone(),
                    daily_failure: Default::default(),
                });
                backups::start_daily(cx);
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

            // Clicking a notification (low stock) brings the window back.
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
        titlebar: Some(TitlebarOptions {
            title: Some("Mascate".into()),
            ..Default::default()
        }),
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
