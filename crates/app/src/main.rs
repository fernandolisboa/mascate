// Release builds on Windows run without a console window behind the app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod appearance;
mod home;
mod kit;
mod layout;
mod palette;
mod parts;
mod preferences;
mod settings;
mod shell;
mod startup;
mod tray;

use futures::StreamExt;
use gpui_kit::*;

use crate::preferences::Preferences;
use crate::shell::Shell;
use crate::tray::TrayCommand;

fn main() {
    let startup = startup::prepare();
    let (database, saved, problem) = match startup {
        Ok(started) => (Some(started.database), started.appearance, None),
        Err(problem) => (None, Default::default(), Some(SharedString::from(problem))),
    };

    // gpui-kit's default icon set; without it icons draw nothing.
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.set_global(Preferences::new(database, saved));
            appearance::init(saved.theme, cx);
            layout::show(saved.layout, cx);

            match tray::start() {
                Ok((tray, mut commands)) => {
                    // Closing the window only hides the app; "Sair" in the tray quits.
                    cx.set_quit_mode(QuitMode::Explicit);
                    cx.set_global(tray);
                    let problem = problem.clone();
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

            show_home(problem, cx);
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
