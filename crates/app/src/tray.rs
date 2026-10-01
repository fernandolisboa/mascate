//! The system tray icon that keeps the app reachable after its window closes.

use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use gpui_kit::Global;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Open,
    Quit,
}

/// Owns the icon; the tray disappears when this is dropped.
pub struct Tray {
    _icon: TrayIcon,
}

impl Global for Tray {}

/// Shows the tray icon with "Abrir" and "Sair". Must run on the main thread,
/// whose message loop delivers the clicks on Windows.
pub fn start() -> Result<(Tray, UnboundedReceiver<TrayCommand>), tray_icon::Error> {
    let open = MenuItem::new("Abrir", true, None);
    let quit = MenuItem::new("Sair", true, None);
    let menu = Menu::new();
    menu.append(&open).expect("appending to a new menu");
    menu.append(&quit).expect("appending to a new menu");

    let (sender, receiver) = unbounded();

    let menu_sender = sender.clone();
    let (open_id, quit_id) = (open.id().clone(), quit.id().clone());
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let command = if event.id == open_id {
            TrayCommand::Open
        } else if event.id == quit_id {
            TrayCommand::Quit
        } else {
            return;
        };
        let _ = menu_sender.unbounded_send(command);
    }));

    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let _ = sender.unbounded_send(TrayCommand::Open);
        }
    }));

    let icon = TrayIconBuilder::new()
        .with_tooltip("Mascate")
        .with_title("Mascate")
        .with_icon(icon())
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;

    Ok((Tray { _icon: icon }, receiver))
}

/// A filled circle in the app's color, drawn in code so no image asset is needed.
fn icon() -> Icon {
    const SIZE: u32 = 32;
    const COLOR: [u8; 3] = [0x0f, 0x76, 0x6e];
    let center = (SIZE as f32 - 1.0) / 2.0;
    let radius = SIZE as f32 / 2.0 - 1.0;
    let rgba = (0..SIZE * SIZE)
        .flat_map(|i| {
            let (x, y) = ((i % SIZE) as f32, (i / SIZE) as f32);
            let distance = ((x - center).powi(2) + (y - center).powi(2)).sqrt();
            // One pixel of soft edge keeps the circle from looking jagged.
            let alpha = ((radius - distance + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            [COLOR[0], COLOR[1], COLOR[2], alpha]
        })
        .collect();
    Icon::from_rgba(rgba, SIZE, SIZE).expect("icon buffer matches its size")
}
