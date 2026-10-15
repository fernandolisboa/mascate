//! The stock of the listings on every connected Sales Channel follows the
//! app's (#18, ADR 0033): after every stock movement the app sends what
//! changed, off the UI thread. What cannot go now waits in the queue on
//! Anúncios, and the next Sync sends it.

use std::sync::Arc;

use gpui_kit::{App, Global};
use mascate_commerce::StockMirror;

use crate::sales_channels;

/// The app's Stock Mirror; absent when the database did not open.
pub struct AppStockMirror(pub Arc<StockMirror>);

impl Global for AppStockMirror {}

pub fn mirror(cx: &App) -> Option<Arc<StockMirror>> {
    cx.try_global::<AppStockMirror>().map(|app| app.0.clone())
}

/// Sends the stock still to send to each connected channel. Each Listing
/// keeps how its send went, so nothing here waits for the answer.
pub fn send_after_movement(cx: &mut App) {
    let Some(mirror) = mirror(cx) else {
        return;
    };
    let reach = sales_channels::reach(cx);
    cx.background_executor()
        .spawn(async move {
            for channel in reach.connected().await {
                if let Err(error) = mirror.send(channel.listings.as_ref()).await {
                    eprintln!("could not send the listings' stock: {error}");
                }
            }
        })
        .detach();
}
