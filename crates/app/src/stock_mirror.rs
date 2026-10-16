//! The stock of the listings on every connected Sales Channel follows its
//! Stock Share of the app's (#18, #125, ADR 0033): after every stock
//! movement the app reads each channel's Orders and then sends what
//! changed, off the UI thread. What cannot go now waits in the queue on
//! Anúncios, and the next Sync sends it.

use std::sync::Arc;

use gpui_kit::{App, Global};
use mascate_commerce::{ListingError, SalesChannel, StockMirror, StockSend};

use crate::orders;
use crate::sales_channels::ChannelPorts;

/// The app's Stock Mirror; absent when the database did not open.
pub struct AppStockMirror(pub Arc<StockMirror>);

impl Global for AppStockMirror {}

pub fn mirror(cx: &App) -> Option<Arc<StockMirror>> {
    cx.try_global::<AppStockMirror>().map(|app| app.0.clone())
}

/// Sends the stock still to send to `channels` in one go, so the shares
/// that go down on one channel reach it before the ones that go up on
/// another.
pub async fn send(
    mirror: &StockMirror,
    channels: &[ChannelPorts],
) -> Result<StockSend, ListingError> {
    let listings: Vec<&dyn SalesChannel> = channels
        .iter()
        .map(|channel| channel.listings.as_ref())
        .collect();
    mirror.send(&listings).await
}

/// Sends the stock still to send to each connected channel, right after
/// an Order Sync of each (`orders::round`), and tells the owner what that
/// Sync brought. Each Listing keeps how its send went, so nothing here
/// waits for the answer.
pub fn send_after_movement(cx: &mut App) {
    let Some(round) = orders::sync_now(cx) else {
        return;
    };
    cx.spawn(async move |cx| {
        let outcome = round.await;
        cx.update(|cx| orders::finished(&outcome, cx));
    })
    .detach();
}
