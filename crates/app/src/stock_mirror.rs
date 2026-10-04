//! The stock of the Mercado Livre listings follows the app's (#18): after
//! every stock movement the app sends what changed, off the UI thread. What
//! cannot go now waits in the queue on Anúncios, and the next Sync sends it.

use std::sync::Arc;

use gpui_kit::{App, Global};
use mascate_commerce::StockMirror;
use mascate_integrations::{Connection, ConnectionState};

use crate::connections::AppConnections;
use crate::mercado_livre;

/// The app's Stock Mirror; absent when the database did not open.
pub struct AppStockMirror(pub Arc<StockMirror>);

impl Global for AppStockMirror {}

pub fn mirror(cx: &App) -> Option<Arc<StockMirror>> {
    cx.try_global::<AppStockMirror>().map(|app| app.0.clone())
}

/// Sends the stock still to send, with the Connection up. Each Listing keeps
/// how its send went, so nothing here waits for the answer.
pub fn send_after_movement(cx: &mut App) {
    let (Some(mirror), Some(channel)) = (mirror(cx), mercado_livre::adapter(cx)) else {
        return;
    };
    let connections = cx.global::<AppConnections>().0.clone();
    cx.background_executor()
        .spawn(async move {
            if connections.state(Connection::MercadoLivre) != ConnectionState::Connected {
                return;
            }
            if let Err(error) = mirror.send(channel.as_ref()).await {
                eprintln!("could not send the listings' stock: {error}");
            }
        })
        .detach();
}
