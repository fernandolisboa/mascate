//! The Telegram queue (#104, ADR 0031 decision 8): an Offer Post the owner
//! confirmed for a time goes out on its own then, while the app is open or
//! in the tray. Each Telegram channel has its posting times on Brasília's
//! clock; "the next free time" is the first of them no other post of the
//! channel holds. Nothing old goes out: a post whose time passed with the
//! app closed, or whose price got older than its Program keeps one, waits
//! for the owner. Paused, the queue sends nothing.

use std::collections::HashSet;
use std::time::Duration;

use chrono::{NaiveTime, TimeDelta, Timelike};
use libsql::{Value, params};
use mascate_kernel::{Pause, RecordId, Timestamp, brasilia, channel_day};
use mascate_platform::{
    Database, Migration, StoredRow as _, load_single_row, save_single_row, stored,
};

use crate::offer_posts::{
    OfferFacts, OfferPost, OfferPosts, POST_QUERY, PostError, PostStatus, post_from,
};
use crate::publishing::{ConfirmedPost, PostDestination, PublishFailure};

pub(crate) const SCHEDULE_OFFER_POSTS: Migration = Migration {
    version: 9,
    name: "schedule Offer Posts at each Telegram channel's posting times",
    risky: false,
    sql: "ALTER TABLE marketing_promotion_channels
        ADD COLUMN posting_times TEXT NOT NULL DEFAULT '08:00 12:00 19:00 21:00';
    ALTER TABLE marketing_offer_posts ADD COLUMN scheduled_at TEXT;
    CREATE INDEX marketing_offer_posts_by_schedule
        ON marketing_offer_posts (status, scheduled_at);
    CREATE TABLE marketing_queue_settings (
        id         TEXT PRIMARY KEY,
        paused     INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );",
};

const SETTINGS_TABLE: &str = "marketing_queue_settings";
const SETTINGS_COLUMNS: &[&str] = &["paused"];

/// Minutes past its time a post still goes out on its own; later, the
/// offer and its price may have changed.
pub const LATE_AFTER_MINUTES: i64 = 30;

/// Hours a price whose Program asks for its time stays good: Amazon's
/// Associates policy keeps its content for 24 hours.
pub const PRICE_KEEPS_HOURS: i64 = 24;

/// The wait between two posts to the same chat; Telegram takes about one
/// message a second per chat.
pub const CHAT_SPACING: Duration = Duration::from_secs(5);

/// The posting times a Telegram channel starts with, on Brasília's clock.
pub fn default_posting_times() -> Vec<NaiveTime> {
    [8, 12, 19, 21]
        .into_iter()
        .filter_map(|hour| NaiveTime::from_hms_opt(hour, 0, 0))
        .collect()
}

/// `times` to the minute, earliest first, each once.
fn tidy_times(times: &[NaiveTime]) -> Vec<NaiveTime> {
    let mut tidy: Vec<NaiveTime> = times.iter().map(|time| to_minute_time(*time)).collect();
    tidy.sort();
    tidy.dedup();
    tidy
}

fn to_minute_time(time: NaiveTime) -> NaiveTime {
    NaiveTime::from_hms_opt(time.hour(), time.minute(), 0).unwrap_or(time)
}

/// `at` without its seconds: posts are scheduled to the minute.
fn to_minute(at: Timestamp) -> Timestamp {
    at.with_second(0)
        .and_then(|at| at.with_nanosecond(0))
        .unwrap_or(at)
}

pub(crate) fn posting_times_text(times: &[NaiveTime]) -> String {
    tidy_times(times)
        .iter()
        .map(|time| time.format("%H:%M").to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn posting_times_from(text: &str) -> Option<Vec<NaiveTime>> {
    text.split_whitespace()
        .map(|time| NaiveTime::parse_from_str(time, "%H:%M").ok())
        .collect()
}

/// The first of `times`, on Brasília's clock, after `now` and on no minute
/// `taken` holds: today's if one is left, else the next day's. `None` only
/// without times.
pub fn next_free_time(
    times: &[NaiveTime],
    taken: &[Timestamp],
    now: Timestamp,
) -> Option<Timestamp> {
    let times = tidy_times(times);
    if times.is_empty() {
        return None;
    }
    let taken: HashSet<Timestamp> = taken.iter().copied().map(to_minute).collect();
    let today = now.with_timezone(&brasilia()).date_naive();
    // Every day after today holds each time, so one per taken post and one
    // more is always enough.
    (0..=taken.len() as i64 + 1)
        .map(|offset| today + TimeDelta::days(offset))
        .flat_map(|day| {
            times.iter().filter_map(move |time| {
                day.and_time(*time)
                    .and_local_timezone(brasilia())
                    .single()
                    .map(|at| at.to_utc())
            })
        })
        .find(|at| *at > now && !taken.contains(at))
}

/// Whether the price of `facts` may still go out at `at`: a Program that
/// asks for the price's time keeps one [`PRICE_KEEPS_HOURS`].
pub(crate) fn price_fresh_at(facts: &OfferFacts, at: Timestamp) -> bool {
    match &facts.price {
        Some(price) if facts.price_needs_time => {
            at - price.seen_at <= TimeDelta::hours(PRICE_KEEPS_HOURS)
        }
        _ => true,
    }
}

/// Takes the scheduled posts of `channel` out of the queue, as drafts.
pub(crate) async fn unschedule(
    database: &Database,
    channel: RecordId,
    now: Timestamp,
) -> Result<u64, libsql::Error> {
    database
        .connection()
        .execute(
            "UPDATE marketing_offer_posts
             SET status = ?1, scheduled_at = NULL, updated_at = ?2
             WHERE channel_id = ?3 AND status = ?4 AND deleted_at IS NULL",
            params![
                PostStatus::Draft.code(),
                stored(now),
                channel.to_string(),
                PostStatus::Scheduled.code()
            ],
        )
        .await
}

/// When a confirmed post goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostTime {
    /// The first of the channel's posting times no other post holds.
    NextFree,
    /// The time the owner picked, to the minute.
    At(Timestamp),
}

/// What one turn of the queue did.
#[derive(Debug, Default)]
pub struct QueueRound {
    /// The posts it handed to the destination, as they ended up.
    pub posts: Vec<OfferPost>,
    /// How many posts it found late or with an expired price, to wait for
    /// the owner.
    pub held: u64,
    /// The queue was paused, so nothing went out.
    pub paused: bool,
    /// The destination refused the login: the rest wait for the next turn.
    pub expired: bool,
    /// The due posts that could not go out, and why; each tries again on
    /// the next turn until it is late.
    pub skipped: Vec<(RecordId, PostError)>,
}

/// The queue as the owner reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostQueue {
    pub paused: bool,
    /// Waiting for their time, the earliest first, and any going out now.
    pub scheduled: Vec<OfferPost>,
    /// Late or with an expired price, waiting for the owner, the earliest
    /// first.
    pub waiting: Vec<OfferPost>,
    pub unconfirmed: Vec<OfferPost>,
    pub failed: Vec<OfferPost>,
    /// Published today, on Brasília's clock, the latest first.
    pub published_today: Vec<OfferPost>,
}

impl PostQueue {
    /// Whether the post `id` is in one of the lists.
    pub fn holds(&self, id: RecordId) -> bool {
        [
            &self.scheduled,
            &self.waiting,
            &self.unconfirmed,
            &self.failed,
            &self.published_today,
        ]
        .into_iter()
        .flatten()
        .any(|post| post.id == id)
    }
}

impl OfferPosts {
    /// The first posting time of `channel` after now that no scheduled
    /// post holds, `except` aside; `None` when the channel has no times.
    pub async fn next_free(
        &self,
        channel: RecordId,
        except: Option<RecordId>,
    ) -> Result<Option<Timestamp>, PostError> {
        let channel = self.channels.channel(channel).await?;
        let taken = self.taken(channel.id, except).await?;
        Ok(next_free_time(
            &channel.posting_times,
            &taken,
            self.clock.now(),
        ))
    }

    /// The minutes the scheduled posts of `channel` hold, `except` aside.
    async fn taken(
        &self,
        channel: RecordId,
        except: Option<RecordId>,
    ) -> Result<Vec<Timestamp>, PostError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT id, scheduled_at FROM marketing_offer_posts
                 WHERE channel_id = ?1 AND status = ?2 AND deleted_at IS NULL",
                params![channel.to_string(), PostStatus::Scheduled.code()],
            )
            .await?;
        let mut taken = Vec::new();
        while let Some(row) = rows.next().await? {
            if Some(row.id_at(0)?) != except
                && let Some(at) = row.optional_time_at(1)?
            {
                taken.push(at);
            }
        }
        Ok(taken)
    }

    /// Puts the post the owner confirmed in the queue for `when`: the
    /// confirmation is the owner's consent for the post to go out then, as
    /// it is, on its own. Only a post still as the owner read it, at a
    /// time after now that no other post of the channel holds and before
    /// its price expires.
    pub async fn schedule(
        &self,
        post: ConfirmedPost,
        when: PostTime,
    ) -> Result<OfferPost, PostError> {
        let request = post.request();
        let saved = self.post(request.post).await?;
        if saved.status != request.prepared_status || saved.updated_at != request.prepared_at {
            return Err(PostError::Changed);
        }
        let now = self.clock.now();
        let at = match when {
            PostTime::NextFree => self
                .next_free(saved.draft.channel, Some(saved.id))
                .await?
                .ok_or(PostError::NoPostingTimes)?,
            PostTime::At(at) => {
                let at = to_minute(at);
                if at <= now {
                    return Err(PostError::PastTime);
                }
                if self
                    .taken(saved.draft.channel, Some(saved.id))
                    .await?
                    .into_iter()
                    .any(|taken| to_minute(taken) == at)
                {
                    return Err(PostError::TimeTaken);
                }
                at
            }
        };
        if !price_fresh_at(&saved.draft.facts, at) {
            return Err(PostError::OldPrice);
        }
        let claimed = self
            .database
            .connection()
            .execute(
                "UPDATE marketing_offer_posts
                 SET status = ?1, scheduled_at = ?2, failure = NULL, failure_detail = NULL,
                     updated_at = ?3
                 WHERE id = ?4 AND deleted_at IS NULL AND status = ?5 AND updated_at = ?6",
                params![
                    PostStatus::Scheduled.code(),
                    stored(at),
                    stored(now),
                    saved.id.to_string(),
                    request.prepared_status.code(),
                    stored(request.prepared_at)
                ],
            )
            .await?;
        if claimed == 0 {
            return Err(PostError::Changed);
        }
        self.post(saved.id).await
    }

    /// Takes the post out of the queue, back to a draft.
    pub async fn unschedule(&self, id: RecordId) -> Result<OfferPost, PostError> {
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE marketing_offer_posts
                 SET status = ?1, scheduled_at = NULL, updated_at = ?2
                 WHERE id = ?3 AND deleted_at IS NULL AND status IN (?4, ?5, ?6)",
                params![
                    PostStatus::Draft.code(),
                    stored(self.clock.now()),
                    id.to_string(),
                    PostStatus::Scheduled.code(),
                    PostStatus::Late.code(),
                    PostStatus::PriceExpired.code()
                ],
            )
            .await?;
        if changed == 0 {
            return Err(match self.post(id).await {
                Ok(post) => PostError::WrongStatus(post.status),
                Err(error) => error,
            });
        }
        self.post(id).await
    }

    /// The scheduled posts that go out at `now`, in schedule order: their
    /// time came less than [`LATE_AFTER_MINUTES`] ago and their price is
    /// still good.
    pub async fn due(&self, now: Timestamp) -> Result<Vec<OfferPost>, PostError> {
        let since = now - TimeDelta::minutes(LATE_AFTER_MINUTES);
        let mut rows = self
            .database
            .connection()
            .query(
                &format!(
                    "{} AND status = ?1 AND scheduled_at <= ?2 AND scheduled_at >= ?3
                     ORDER BY scheduled_at, id",
                    POST_QUERY
                ),
                params![PostStatus::Scheduled.code(), stored(now), stored(since)],
            )
            .await?;
        let mut due = Vec::new();
        while let Some(row) = rows.next().await? {
            let post = post_from(&row)?;
            if price_fresh_at(&post.draft.facts, now) {
                due.push(post);
            }
        }
        Ok(due)
    }

    /// Sets aside, for the owner, every scheduled post whose time passed
    /// more than [`LATE_AFTER_MINUTES`] ago, then every one due whose price
    /// expired; how many.
    pub async fn hold_back(&self) -> Result<u64, PostError> {
        let now = self.clock.now();
        let connection = self.database.connection();
        let late = connection
            .execute(
                "UPDATE marketing_offer_posts SET status = ?1, updated_at = ?2
                 WHERE status = ?3 AND deleted_at IS NULL AND scheduled_at < ?4",
                params![
                    PostStatus::Late.code(),
                    stored(now),
                    PostStatus::Scheduled.code(),
                    stored(now - TimeDelta::minutes(LATE_AFTER_MINUTES))
                ],
            )
            .await?;
        let expired = connection
            .execute(
                "UPDATE marketing_offer_posts SET status = ?1, updated_at = ?2
                 WHERE status = ?3 AND deleted_at IS NULL AND scheduled_at <= ?2
                     AND price_needs_time = 1 AND price IS NOT NULL AND price_seen_at < ?4",
                params![
                    PostStatus::PriceExpired.code(),
                    stored(now),
                    PostStatus::Scheduled.code(),
                    stored(now - TimeDelta::hours(PRICE_KEEPS_HOURS))
                ],
            )
            .await?;
        Ok(late + expired)
    }

    /// One turn of the queue: holds back what is late or has an expired
    /// price, then, unless paused, publishes the due posts one at a time,
    /// [`CHAT_SPACING`] apart in the same chat. A post that changed since
    /// it was read is left alone; one that may have gone out is never sent
    /// again on its own.
    pub async fn send_due(
        &self,
        destination: &dyn PostDestination,
        pause: &dyn Pause,
    ) -> Result<QueueRound, PostError> {
        let mut round = QueueRound {
            held: self.hold_back().await?,
            paused: self.queue_paused().await?,
            ..QueueRound::default()
        };
        if round.paused {
            return Ok(round);
        }
        let mut chats: HashSet<String> = HashSet::new();
        for post in self.due(self.clock.now()).await? {
            let request = match self.prepare(post.id).await {
                // Changed since it was read: it is not the post that was due.
                Ok(request)
                    if request.prepared_status != PostStatus::Scheduled
                        || request.prepared_at != post.updated_at =>
                {
                    continue;
                }
                Ok(request) => request,
                Err(PostError::NotFound) => continue,
                Err(error) => {
                    round.skipped.push((post.id, error));
                    continue;
                }
            };
            if !chats.insert(request.chat().to_owned()) {
                pause.pause(CHAT_SPACING);
            }
            if self.queue_paused().await? {
                round.paused = true;
                break;
            }
            match self.publish(destination, request.confirm()).await {
                Ok(published) => {
                    let expired = published.failure == Some(PublishFailure::Expired);
                    round.posts.push(published);
                    if expired {
                        round.expired = true;
                        break;
                    }
                }
                Err(PostError::Changed) => continue,
                Err(error) => round.skipped.push((post.id, error)),
            }
        }
        Ok(round)
    }

    /// The queue's lists now.
    pub async fn queue(&self) -> Result<PostQueue, PostError> {
        let today = channel_day(self.clock.now());
        let mut queue = PostQueue {
            paused: self.queue_paused().await?,
            ..PostQueue::default()
        };
        for post in self.posts().await? {
            match post.status {
                PostStatus::Scheduled | PostStatus::Sending => queue.scheduled.push(post),
                PostStatus::Late | PostStatus::PriceExpired => queue.waiting.push(post),
                PostStatus::Unconfirmed => queue.unconfirmed.push(post),
                PostStatus::Failed => queue.failed.push(post),
                PostStatus::Sent
                    if post
                        .sent
                        .as_ref()
                        .is_some_and(|sent| channel_day(sent.at) == today) =>
                {
                    queue.published_today.push(post)
                }
                PostStatus::Draft | PostStatus::Sent => {}
            }
        }
        queue
            .scheduled
            .sort_by_key(|post| (post.scheduled_at, post.id));
        queue
            .waiting
            .sort_by_key(|post| (post.scheduled_at, post.id));
        queue
            .published_today
            .sort_by_key(|post| std::cmp::Reverse(post.sent.as_ref().map(|sent| sent.at)));
        Ok(queue)
    }

    /// Whether the owner paused the queue.
    pub async fn queue_paused(&self) -> Result<bool, PostError> {
        let row =
            load_single_row(self.database.connection(), SETTINGS_TABLE, SETTINGS_COLUMNS).await?;
        Ok(match row {
            Some(row) => row.get::<i64>(0)? != 0,
            None => false,
        })
    }

    /// Pauses or resumes the queue. Resumed, it sends only what is not
    /// late by then.
    pub async fn set_queue_paused(&self, paused: bool) -> Result<(), PostError> {
        save_single_row(
            self.database.connection(),
            self.clock.as_ref(),
            self.ids.as_ref(),
            SETTINGS_TABLE,
            SETTINGS_COLUMNS,
            vec![Value::Integer(i64::from(paused))],
        )
        .await?;
        Ok(())
    }
}
