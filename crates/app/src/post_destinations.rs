//! Where an Offer Post goes out through the app, by its Promotion Channel's
//! kind: Telegram (#103) or Pinterest (#155). Each destination has its
//! adapter, its Connection and its own words for what went wrong.

use std::sync::Arc;

use gpui_kit::App;
use mascate_marketing::{ChannelKind, PostDestination, PublishFailure};

use crate::{pinterest, telegram};

/// Whether posts of a channel of `kind` go out through the app.
pub(crate) fn publishes(kind: ChannelKind) -> bool {
    destination_name(kind).is_some()
}

/// The destination's name, as the owner reads it.
pub(crate) fn destination_name(kind: ChannelKind) -> Option<&'static str> {
    match kind {
        ChannelKind::TelegramChannel => Some("Telegram"),
        ChannelKind::Pinterest => Some("Pinterest"),
        ChannelKind::WhatsAppGroup
        | ChannelKind::Instagram
        | ChannelKind::TikTok
        | ChannelKind::PaidSearch
        | ChannelKind::Other => None,
    }
}

/// The adapter posts of a channel of `kind` go out through.
pub(crate) fn adapter(kind: ChannelKind, cx: &App) -> Option<Arc<dyn PostDestination>> {
    match kind {
        ChannelKind::TelegramChannel => {
            telegram::adapter(cx).map(|adapter| adapter as Arc<dyn PostDestination>)
        }
        ChannelKind::Pinterest => {
            pinterest::adapter(cx).map(|adapter| adapter as Arc<dyn PostDestination>)
        }
        _ => None,
    }
}

/// Why posts of a channel of `kind` cannot go out now; `None` while its
/// Connection is up.
pub(crate) fn publishing_off(kind: ChannelKind, cx: &App) -> Option<&'static str> {
    match kind {
        ChannelKind::TelegramChannel => telegram::state(cx)
            .as_ref()
            .and_then(telegram::publishing_off),
        ChannelKind::Pinterest => pinterest::state(cx)
            .as_ref()
            .and_then(pinterest::publishing_off),
        _ => None,
    }
}

/// Why a post of a channel of `kind` did not go out, or may not have.
pub(crate) fn publish_failure(kind: ChannelKind, failure: &PublishFailure) -> String {
    match kind {
        ChannelKind::Pinterest => pinterest::publish_failure(failure),
        _ => telegram::publish_failure(failure),
    }
}

/// Notes that the destination of `kind` refused its login.
pub(crate) fn expired(kind: ChannelKind, cx: &App) {
    match kind {
        ChannelKind::TelegramChannel => telegram::expired(cx),
        ChannelKind::Pinterest => pinterest::expired(cx),
        _ => {}
    }
}
