//! The Telegram queue as the app runs it (#104): a turn every minute, for as
//! long as the app runs, window open or not, the first one at start. A
//! turn sends the Offer Posts due while the Telegram Connection is up, and
//! always sets aside, for the owner, what is late or has an expired price.

use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Weekday};
use gpui_kit::{App, BorrowAppContext as _};
use mascate_integrations::ConnectionState;
use mascate_kernel::{ThreadPause, Timestamp, brasilia};

use crate::offer_posts::{PostChanges, offer_posts};
use crate::telegram;

/// How often the queue looks for due posts.
const EVERY: Duration = Duration::from_secs(60);

/// Starts the queue's turns.
pub fn start(cx: &mut App) {
    if offer_posts(cx).is_none() {
        return;
    }
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        loop {
            let turn = cx.update(|cx| {
                let posts = offer_posts(cx)?;
                let destination = telegram::adapter(cx)
                    .filter(|_| telegram::state(cx) == Some(ConnectionState::Connected));
                Some(cx.background_executor().spawn(async move {
                    match destination {
                        Some(telegram) => posts
                            .send_due(telegram.as_ref(), &ThreadPause)
                            .await
                            .map(|round| {
                                for (post, error) in &round.skipped {
                                    eprintln!("the queue could not publish post {post}: {error}");
                                }
                                (round.held > 0 || !round.posts.is_empty(), round.expired)
                            }),
                        None => posts.hold_back().await.map(|held| (held > 0, false)),
                    }
                }))
            });
            if let Some(turn) = turn {
                let outcome = turn.await;
                cx.update(|cx| match outcome {
                    Ok((changed, expired)) => {
                        if expired {
                            telegram::expired(cx);
                        }
                        if changed {
                            cx.update_default_global::<PostChanges, _>(|changes, _| changes.0 += 1);
                        }
                    }
                    Err(error) => eprintln!("the Telegram queue failed: {error}"),
                });
            }
            executor.timer(EVERY).await;
        }
    })
    .detach();
}

/// When a post goes out, as the owner reads it, on Brasília's clock: "qui,
/// 16/10 às 08:00".
pub fn post_time(at: Timestamp) -> String {
    let local = at.with_timezone(&brasilia());
    format!(
        "{}, {}",
        short_weekday(local.weekday()),
        local.format("%d/%m às %H:%M")
    )
}

/// "seg", "sáb": a weekday as dates show it.
pub fn short_weekday(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "seg",
        Weekday::Tue => "ter",
        Weekday::Wed => "qua",
        Weekday::Thu => "qui",
        Weekday::Fri => "sex",
        Weekday::Sat => "sáb",
        Weekday::Sun => "dom",
    }
}

/// A time of day as the owner types it: `8`, `8h`, `8h30`, `08:30`.
fn typed_time(text: &str) -> Option<NaiveTime> {
    let text = text.trim().to_lowercase();
    let (hour, minute) = match text.split_once([':', 'h']) {
        Some((hour, minute)) => (hour, minute),
        None => (text.as_str(), ""),
    };
    let hour: u32 = hour.trim().parse().ok()?;
    let minute: u32 = match minute.trim() {
        "" => 0,
        minute if minute.len() == 2 => minute.parse().ok()?,
        _ => return None,
    };
    NaiveTime::from_hms_opt(hour, minute, 0)
}

/// Posting times typed with commas or spaces between them, as `8h, 12:30`.
/// `Err` names the first one that is not a time.
pub fn typed_times(text: &str) -> Result<Vec<NaiveTime>, String> {
    text.split([',', ';', ' '])
        .filter(|part| !part.trim().is_empty())
        .map(|part| typed_time(part).ok_or_else(|| part.trim().to_owned()))
        .collect()
}

/// Posting times as the owner reads and types them: "08:00, 12:00".
pub fn times_text(times: &[NaiveTime]) -> String {
    times
        .iter()
        .map(|time| time.format("%H:%M").to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// A day and time as the owner types it, on Brasília's clock: `16/10
/// 19:30` in the year of `now`, or `16/10/2027 19:30`.
pub fn typed_moment(text: &str, now: Timestamp) -> Option<Timestamp> {
    let (day, time) = text.trim().split_once(char::is_whitespace)?;
    let time = typed_time(time)?;
    let mut parts = day.trim().split('/');
    let (day, month) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    let year = match parts.next() {
        Some(year) => year.parse().ok()?,
        None => now.with_timezone(&brasilia()).year(),
    };
    if parts.next().is_some() {
        return None;
    }
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    NaiveDateTime::new(date, time)
        .and_local_timezone(brasilia())
        .single()
        .map(|at| at.to_utc())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn time(hour: u32, minute: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(hour, minute, 0).unwrap()
    }

    #[test]
    fn posting_times_read_the_way_the_owner_writes_them() {
        assert_eq!(
            typed_times("8, 12h, 19h30; 21:00  07:05"),
            Ok(vec![
                time(8, 0),
                time(12, 0),
                time(19, 30),
                time(21, 0),
                time(7, 5)
            ])
        );
        assert_eq!(typed_times(""), Ok(Vec::new()));
        assert_eq!(typed_times("8, 25h"), Err("25h".to_owned()));
        assert_eq!(typed_times("8:5"), Err("8:5".to_owned()));
        assert_eq!(typed_times("meio-dia"), Err("meio-dia".to_owned()));
        assert_eq!(times_text(&[time(8, 0), time(19, 30)]), "08:00, 19:30");
    }

    #[test]
    fn a_typed_moment_is_on_brasilias_clock() {
        let now = brasilia()
            .with_ymd_and_hms(2026, 10, 14, 9, 0, 0)
            .unwrap()
            .to_utc();
        let expected = brasilia()
            .with_ymd_and_hms(2026, 10, 16, 19, 30, 0)
            .unwrap()
            .to_utc();
        assert_eq!(typed_moment("16/10 19:30", now), Some(expected));
        assert_eq!(typed_moment(" 16/10/2026 19h30 ", now), Some(expected));
        assert_eq!(typed_moment("31/02 19:30", now), None);
        assert_eq!(typed_moment("16/10", now), None);
        assert_eq!(typed_moment("16/10/2026/1 19:30", now), None);
        assert_eq!(post_time(expected), "sex, 16/10 às 19:30");
    }
}
