//! Prospects (#219, ADR 0039): products of an affiliate Platform the owner
//! does not promote yet, found by a reader or typed by hand, kept by the
//! Platform's natural key with each signal's origin and time, ranked by
//! their score. "Quero divulgar" turns one into a provisional Program;
//! "Descartar" keeps it out of every later read.

use std::collections::HashMap;

use libsql::{Connection, TransactionBehavior, params};
use mascate_kernel::{RecordId, channel_day, folded_words};
use mascate_platform::{Migration, StoredRow, stored};

use crate::commissions::{Affiliate, AffiliateError, platform_at};
use crate::prospect_score::{Score, is_bet, score};
use crate::prospect_signals::{SignalKind, SignalOrigin, SignalReading, Signals};
use crate::prospect_source::{FoundProspect, ProspectCategory, ProspectSource};
use crate::provisional_programs::NewProgram;
use crate::rules::Program;
use crate::source::AffiliatePlatform;

pub(crate) const CREATE_PROSPECTS: Migration = Migration {
    version: 9,
    name: "create affiliate Prospects and their signals",
    risky: false,
    sql: "CREATE TABLE affiliate_prospects (
        id               TEXT PRIMARY KEY,
        platform         TEXT NOT NULL,
        product_key      TEXT NOT NULL,
        name             TEXT NOT NULL,
        program_id       TEXT REFERENCES affiliate_programs (id),
        dismissed_reason TEXT,
        dismissed_at     TEXT,
        created_at       TEXT NOT NULL,
        updated_at       TEXT NOT NULL,
        deleted_at       TEXT
    );
    CREATE UNIQUE INDEX affiliate_prospects_by_key
        ON affiliate_prospects (platform, product_key)
        WHERE deleted_at IS NULL;
    CREATE TABLE affiliate_prospect_signals (
        id          TEXT PRIMARY KEY,
        prospect_id TEXT NOT NULL REFERENCES affiliate_prospects (id),
        signal      TEXT NOT NULL,
        value       TEXT NOT NULL,
        origin      TEXT NOT NULL,
        read_at     TEXT NOT NULL,
        created_at  TEXT NOT NULL,
        updated_at  TEXT NOT NULL,
        deleted_at  TEXT
    );
    CREATE UNIQUE INDEX affiliate_prospect_signals_by_kind
        ON affiliate_prospect_signals (prospect_id, signal)
        WHERE deleted_at IS NULL;",
};

/// A Prospect with its score.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prospect {
    pub id: RecordId,
    pub platform: AffiliatePlatform,
    /// The product's id on the Platform, or the link the owner pasted.
    pub key: String,
    pub name: String,
    pub signals: Signals,
    /// Where each signal it has came from, and when it took its value.
    pub readings: Vec<SignalReading>,
    pub score: Score,
    /// New, good and with few affiliates (ADR 0039, decision 5).
    pub bet: bool,
    /// The Program that promotes its product: the one "Quero divulgar"
    /// created, or one the owner already had ("já divulga").
    pub program: Option<RecordId>,
    /// The owner chose it with "Quero divulgar".
    pub chosen: bool,
    /// Why the owner dismissed it, if they did.
    pub dismissed: Option<String>,
}

impl Prospect {
    pub fn reading(&self, kind: SignalKind) -> Option<&SignalReading> {
        self.readings.iter().find(|reading| reading.kind == kind)
    }
}

/// What one read of a [`ProspectSource`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProspectsRead {
    pub found: usize,
    pub added: usize,
    /// Found again, but dismissed before: they stay out of the list.
    pub dismissed: usize,
}

/// The natural key of a product the owner pastes the link of: the address
/// without its query, fragment or trailing slash, its host in lowercase.
/// `None` unless it is a web address.
pub fn prospect_key_of(link: &str) -> Option<String> {
    let link = link.trim();
    let (scheme, rest) = link.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if host.is_empty() {
        return None;
    }
    let path = path.trim_end_matches('/');
    Some(match path {
        "" => format!("https://{}", host.to_ascii_lowercase()),
        path => format!("https://{}/{path}", host.to_ascii_lowercase()),
    })
}

/// A live Program, as a Prospect is matched against it.
struct ProgramKey {
    id: RecordId,
    platform: AffiliatePlatform,
    key: String,
    names: Vec<Vec<String>>,
}

/// The live Prospects of `?1` (any Platform when null), only `?2` when
/// not null, the dismissed ones too when `?3`.
const PROSPECT_QUERY: &str = "SELECT id, platform, product_key, name, program_id, dismissed_reason
     FROM affiliate_prospects
     WHERE deleted_at IS NULL AND (?1 IS NULL OR platform = ?1) AND (?2 IS NULL OR id = ?2)
       AND (?3 OR dismissed_at IS NULL)";

impl Affiliate {
    /// Keeps `found` by its Platform and key: each signal it has replaces
    /// the one kept, with `origin` and the time, unless its value is the
    /// same; the others stay. Saving the same again changes nothing, and a
    /// dismissed Prospect stays dismissed.
    pub async fn save_prospect(
        &self,
        found: &FoundProspect,
        origin: SignalOrigin,
    ) -> Result<RecordId, AffiliateError> {
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let (id, _) = self.keep_prospect(&transaction, found, origin).await?;
        transaction.commit().await?;
        Ok(id)
    }

    /// Reads `source` once, in `category` or its best sellers, and keeps
    /// every Prospect it shows. Nothing is kept unless everything was read.
    pub async fn read_prospects(
        &self,
        source: &dyn ProspectSource,
        category: Option<&ProspectCategory>,
    ) -> Result<ProspectsRead, AffiliateError> {
        let found = source.prospects(category)?;
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let mut read = ProspectsRead {
            found: found.len(),
            ..ProspectsRead::default()
        };
        for prospect in &found {
            if prospect.platform != source.platform() {
                continue;
            }
            let (_, kept) = self
                .keep_prospect(&transaction, prospect, source.origin())
                .await?;
            match kept {
                Kept::Added => read.added += 1,
                Kept::Dismissed => read.dismissed += 1,
                Kept::Known => {}
            }
        }
        transaction.commit().await?;
        Ok(read)
    }

    /// Reads the page of `prospect` from `source`, its own Platform's, and
    /// keeps what it shows the same way as a list read: the Prospect as it
    /// is kept afterwards.
    pub async fn read_prospect_details(
        &self,
        source: &dyn ProspectSource,
        prospect: RecordId,
    ) -> Result<Prospect, AffiliateError> {
        let known = self.prospect(prospect).await?;
        if known.platform != source.platform() {
            return Err(AffiliateError::NoSuchProspect);
        }
        let read = source.details(&FoundProspect {
            platform: known.platform,
            key: known.key,
            name: known.name,
            signals: known.signals,
        })?;
        if read.platform != known.platform {
            return Err(AffiliateError::NoSuchProspect);
        }
        let id = self.save_prospect(&read, source.origin()).await?;
        self.prospect(id).await
    }

    /// The Prospects not dismissed, of `platform` or of every Platform,
    /// the best score first.
    pub async fn prospects(
        &self,
        platform: Option<AffiliatePlatform>,
    ) -> Result<Vec<Prospect>, AffiliateError> {
        let mut prospects: Vec<Prospect> = self.read_prospects_where(platform, None).await?;
        prospects.sort_by(|a, b| {
            b.score
                .total
                .cmp(&a.score.total)
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(prospects)
    }

    /// One Prospect, dismissed or not.
    pub async fn prospect(&self, id: RecordId) -> Result<Prospect, AffiliateError> {
        self.read_prospects_where(None, Some(id))
            .await?
            .pop()
            .ok_or(AffiliateError::NoSuchProspect)
    }

    /// How many Prospects of `platform`, or of every Platform, the owner
    /// dismissed.
    pub async fn dismissed_prospects(
        &self,
        platform: Option<AffiliatePlatform>,
    ) -> Result<u64, AffiliateError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT COUNT(*) FROM affiliate_prospects
                 WHERE deleted_at IS NULL AND dismissed_at IS NOT NULL
                   AND (?1 IS NULL OR platform = ?1)",
                params![platform.map(AffiliatePlatform::code)],
            )
            .await?;
        let row = rows.next().await?.ok_or(AffiliateError::NoSuchProspect)?;
        Ok(row.get(0)?)
    }

    /// The owner does not want `prospect`, for `reason`: it never shows
    /// again, whatever a later read finds.
    pub async fn dismiss_prospect(
        &self,
        prospect: RecordId,
        reason: &str,
    ) -> Result<(), AffiliateError> {
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(AffiliateError::NoReason);
        }
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE affiliate_prospects
                 SET dismissed_reason = ?1, dismissed_at = ?2, updated_at = ?2
                 WHERE id = ?3 AND deleted_at IS NULL",
                params![reason, stored(self.clock.now()), prospect.to_string()],
            )
            .await?;
        if changed == 0 {
            return Err(AffiliateError::NoSuchProspect);
        }
        Ok(())
    }

    /// "Quero divulgar": the Program that already promotes `prospect`'s
    /// product, else one added before its first sale (ADR 0037, decision
    /// 5), and the Prospect marked as chosen. Doing it again gives back the
    /// same Program.
    pub async fn promote_prospect(&self, prospect: RecordId) -> Result<Program, AffiliateError> {
        let wanted = self.prospect(prospect).await?;
        let program = match wanted.program {
            Some(program) => self.program(program).await?,
            None => {
                self.add_program(&NewProgram {
                    platform: wanted.platform,
                    name: String::new(),
                    product: wanted.name.clone(),
                })
                .await?
            }
        };
        if !wanted.chosen {
            self.database
                .connection()
                .execute(
                    "UPDATE affiliate_prospects SET program_id = ?1, updated_at = ?2 WHERE id = ?3",
                    params![
                        program.id.to_string(),
                        stored(self.clock.now()),
                        prospect.to_string()
                    ],
                )
                .await?;
        }
        Ok(program)
    }

    async fn keep_prospect(
        &self,
        on: &Connection,
        found: &FoundProspect,
        origin: SignalOrigin,
    ) -> Result<(RecordId, Kept), AffiliateError> {
        let (key, name) = (found.key.trim(), found.name.trim());
        if key.is_empty() {
            return Err(AffiliateError::NoProspectKey);
        }
        if name.is_empty() {
            return Err(AffiliateError::NoProduct);
        }
        if let Some(kind) = found.signals.impossible() {
            return Err(AffiliateError::ImpossibleSignal(kind));
        }
        let now = stored(self.clock.now());
        let mut kept_row = kept_prospect(on, found.platform, key).await?;
        if kept_row.is_none() {
            kept_row = self.adopt_pasted(on, found, key).await?;
        }
        if kept_row.is_none() {
            kept_row = same_affiliation_page(on, found).await?;
        }
        let (id, kept) = match kept_row {
            Some((id, kept_name, dismissed)) => {
                if kept_name != name {
                    on.execute(
                        "UPDATE affiliate_prospects SET name = ?1, updated_at = ?2 WHERE id = ?3",
                        params![name, now.clone(), id.to_string()],
                    )
                    .await?;
                }
                (
                    id,
                    if dismissed {
                        Kept::Dismissed
                    } else {
                        Kept::Known
                    },
                )
            }
            None => {
                let id = self.ids.next_id();
                on.execute(
                    "INSERT INTO affiliate_prospects
                         (id, platform, product_key, name, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                    params![
                        id.to_string(),
                        found.platform.code(),
                        key,
                        name,
                        now.clone()
                    ],
                )
                .await?;
                (id, Kept::Added)
            }
        };
        for (kind, value) in found.signals.texts() {
            let kept_signal = first_row(
                on,
                "SELECT id, value, origin FROM affiliate_prospect_signals
                 WHERE prospect_id = ?1 AND signal = ?2 AND deleted_at IS NULL",
                params![id.to_string(), kind.code()],
                |row| Ok((row.id_at(0)?, row.get::<String>(1)?, row.get::<String>(2)?)),
            )
            .await?;
            match kept_signal {
                Some((signal, kept_value, kept_origin)) => {
                    if kept_value != value || kept_origin != origin.code() {
                        on.execute(
                            "UPDATE affiliate_prospect_signals
                             SET value = ?1, origin = ?2, read_at = ?3, updated_at = ?3
                             WHERE id = ?4",
                            params![value, origin.code(), now.clone(), signal.to_string()],
                        )
                        .await?;
                    }
                }
                None => {
                    on.execute(
                        "INSERT INTO affiliate_prospect_signals
                             (id, prospect_id, signal, value, origin, read_at, created_at,
                              updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?6)",
                        params![
                            self.ids.next_id().to_string(),
                            id.to_string(),
                            kind.code(),
                            value,
                            origin.code(),
                            now.clone()
                        ],
                    )
                    .await?;
                }
            }
        }
        self.fold_same_product(on, id, found).await?;
        Ok((id, kept))
    }

    /// Folds into `id` every other live Prospect with `found`'s affiliation
    /// page: one product read from the market under one key and from the
    /// vitrine under another. Each signal `id` lacks moves over with its
    /// origin and time; the others go. A dismissal or a Program the other
    /// one had stays with the product.
    async fn fold_same_product(
        &self,
        on: &Connection,
        id: RecordId,
        found: &FoundProspect,
    ) -> Result<(), AffiliateError> {
        let Some(page) = found.signals.affiliation_page.as_deref() else {
            return Ok(());
        };
        let mut rows = on
            .query(
                "SELECT p.id FROM affiliate_prospects p
                 JOIN affiliate_prospect_signals s
                   ON s.prospect_id = p.id AND s.deleted_at IS NULL
                 WHERE p.platform = ?1 AND p.deleted_at IS NULL AND p.id <> ?2
                   AND s.signal = ?3 AND s.value = ?4",
                params![
                    found.platform.code(),
                    id.to_string(),
                    SignalKind::AffiliationPage.code(),
                    page
                ],
            )
            .await?;
        let mut others = Vec::new();
        while let Some(row) = rows.next().await? {
            others.push(row.id_at(0)?);
        }
        let now = stored(self.clock.now());
        for other in others {
            let (id, other) = (id.to_string(), other.to_string());
            on.execute(
                "UPDATE affiliate_prospect_signals SET deleted_at = ?1, updated_at = ?1
                 WHERE prospect_id = ?2 AND deleted_at IS NULL AND signal IN
                   (SELECT signal FROM affiliate_prospect_signals
                    WHERE prospect_id = ?3 AND deleted_at IS NULL)",
                params![now.clone(), other.clone(), id.clone()],
            )
            .await?;
            on.execute(
                "UPDATE affiliate_prospect_signals SET prospect_id = ?1, updated_at = ?2
                 WHERE prospect_id = ?3 AND deleted_at IS NULL",
                params![id.clone(), now.clone(), other.clone()],
            )
            .await?;
            on.execute(
                "UPDATE affiliate_prospects SET
                   program_id = COALESCE(program_id,
                     (SELECT program_id FROM affiliate_prospects WHERE id = ?2)),
                   dismissed_reason = COALESCE(dismissed_reason,
                     (SELECT dismissed_reason FROM affiliate_prospects WHERE id = ?2)),
                   dismissed_at = COALESCE(dismissed_at,
                     (SELECT dismissed_at FROM affiliate_prospects WHERE id = ?2)),
                   updated_at = ?3
                 WHERE id = ?1",
                params![id.clone(), other.clone(), now.clone()],
            )
            .await?;
            on.execute(
                "UPDATE affiliate_prospects SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                params![now.clone(), other],
            )
            .await?;
        }
        Ok(())
    }

    /// The Prospect the owner typed with the link of `found`'s page, now
    /// keyed by the Platform's own `key`, so one product is one Prospect
    /// whether pasted first or read first.
    async fn adopt_pasted(
        &self,
        on: &Connection,
        found: &FoundProspect,
        key: &str,
    ) -> Result<Option<(RecordId, String, bool)>, AffiliateError> {
        let Some(pasted) = found
            .signals
            .product_page
            .as_deref()
            .and_then(prospect_key_of)
            .filter(|pasted| pasted != key)
        else {
            return Ok(None);
        };
        let Some(kept) = kept_prospect(on, found.platform, &pasted).await? else {
            return Ok(None);
        };
        on.execute(
            "UPDATE affiliate_prospects SET product_key = ?1, updated_at = ?2 WHERE id = ?3",
            params![key, stored(self.clock.now()), kept.0.to_string()],
        )
        .await?;
        Ok(Some(kept))
    }

    /// The Prospects not dismissed of `platform`, or `only` dismissed or
    /// not.
    async fn read_prospects_where(
        &self,
        platform: Option<AffiliatePlatform>,
        only: Option<RecordId>,
    ) -> Result<Vec<Prospect>, AffiliateError> {
        let connection = self.database.connection();
        let mut rows = connection
            .query(
                PROSPECT_QUERY,
                params![
                    platform.map(AffiliatePlatform::code),
                    only.map(|id| id.to_string()),
                    only.is_some()
                ],
            )
            .await?;
        let mut found = Vec::new();
        while let Some(row) = rows.next().await? {
            found.push((
                row.id_at(0)?,
                platform_at(&row, 1)?,
                row.get::<String>(2)?,
                row.get::<String>(3)?,
                row.optional_id_at(4)?,
                row.get::<Option<String>>(5)?,
            ));
        }
        let mut signals = self.prospect_signals(connection).await?;
        let programs = live_programs(connection).await?;
        let today = channel_day(self.clock.now());
        Ok(found
            .into_iter()
            .map(|(id, platform, key, name, chosen, dismissed)| {
                let (signals, readings) = signals.remove(&id).unwrap_or_default();
                let chosen = chosen.filter(|chosen| programs.iter().any(|p| p.id == *chosen));
                let program = chosen.or_else(|| promoting(&programs, platform, &key, &name));
                Prospect {
                    score: score(&signals, today),
                    bet: is_bet(&signals, today),
                    id,
                    platform,
                    key,
                    name,
                    signals,
                    readings,
                    program,
                    chosen: chosen.is_some(),
                    dismissed,
                }
            })
            .collect())
    }

    /// Every live Prospect's signals. A signal this version does not know,
    /// from a newer one, is left out.
    async fn prospect_signals(
        &self,
        on: &Connection,
    ) -> Result<HashMap<RecordId, (Signals, Vec<SignalReading>)>, AffiliateError> {
        let mut rows = on
            .query(
                "SELECT prospect_id, signal, value, origin, read_at
                 FROM affiliate_prospect_signals WHERE deleted_at IS NULL
                 ORDER BY prospect_id, signal",
                (),
            )
            .await?;
        let mut kept: HashMap<RecordId, (Signals, Vec<SignalReading>)> = HashMap::new();
        while let Some(row) = rows.next().await? {
            let code: String = row.get(1)?;
            let Some(kind) = SignalKind::from_code(&code) else {
                continue;
            };
            let value: String = row.get(2)?;
            let origin: String = row.get(3)?;
            let origin = SignalOrigin::from_code(&origin)
                .ok_or_else(|| AffiliateError::Unreadable(origin.clone()))?;
            let (signals, readings) = kept.entry(row.id_at(0)?).or_default();
            if !signals.set_text(kind, &value) {
                return Err(AffiliateError::Unreadable(value));
            }
            readings.push(SignalReading {
                kind,
                origin,
                read_at: row.time_at(4)?,
            });
        }
        Ok(kept)
    }
}

/// The first row `query` gives, read by `read`.
async fn first_row<T>(
    on: &Connection,
    query: &str,
    params: impl libsql::params::IntoParams,
    read: impl FnOnce(&libsql::Row) -> Result<T, AffiliateError>,
) -> Result<Option<T>, AffiliateError> {
    let mut rows = on.query(query, params).await?;
    match rows.next().await? {
        Some(row) => Ok(Some(read(&row)?)),
        None => Ok(None),
    }
}

/// The live Prospect of `platform` keyed `key`: its id, its name and
/// whether it was dismissed.
async fn kept_prospect(
    on: &Connection,
    platform: AffiliatePlatform,
    key: &str,
) -> Result<Option<(RecordId, String, bool)>, AffiliateError> {
    first_row(
        on,
        "SELECT id, name, dismissed_at IS NOT NULL FROM affiliate_prospects
         WHERE platform = ?1 AND product_key = ?2 AND deleted_at IS NULL",
        params![platform.code(), key],
        |row| Ok((row.id_at(0)?, row.get::<String>(1)?, row.get::<bool>(2)?)),
    )
    .await
}

/// The live Prospect of `found`'s Platform with the same affiliation page,
/// under another key: the oldest, when there are more.
async fn same_affiliation_page(
    on: &Connection,
    found: &FoundProspect,
) -> Result<Option<(RecordId, String, bool)>, AffiliateError> {
    let Some(page) = found.signals.affiliation_page.as_deref() else {
        return Ok(None);
    };
    first_row(
        on,
        "SELECT p.id, p.name, p.dismissed_at IS NOT NULL FROM affiliate_prospects p
         JOIN affiliate_prospect_signals s ON s.prospect_id = p.id AND s.deleted_at IS NULL
         WHERE p.platform = ?1 AND p.deleted_at IS NULL
           AND s.signal = ?2 AND s.value = ?3
         ORDER BY p.created_at, p.id",
        params![
            found.platform.code(),
            SignalKind::AffiliationPage.code(),
            page
        ],
        |row| Ok((row.id_at(0)?, row.get::<String>(1)?, row.get::<bool>(2)?)),
    )
    .await
}

/// What keeping a found Prospect did.
enum Kept {
    Added,
    Known,
    Dismissed,
}

async fn live_programs(on: &Connection) -> Result<Vec<ProgramKey>, AffiliateError> {
    let mut rows = on
        .query(
            "SELECT id, platform, program_key, name, product
             FROM affiliate_programs WHERE deleted_at IS NULL",
            (),
        )
        .await?;
    let mut programs = Vec::new();
    while let Some(row) = rows.next().await? {
        let name: String = row.get(3)?;
        let product: Option<String> = row.get(4)?;
        programs.push(ProgramKey {
            id: row.id_at(0)?,
            platform: platform_at(&row, 1)?,
            key: row.get(2)?,
            names: std::iter::once(name)
                .chain(product)
                .map(|name| folded_words(&name))
                .collect(),
        });
    }
    Ok(programs)
}

/// The Program the owner already promotes the product with: the one with
/// the product's id as its key, as a Sync creates it, or one named like
/// it, case, accents and spacing aside.
fn promoting(
    programs: &[ProgramKey],
    platform: AffiliatePlatform,
    key: &str,
    name: &str,
) -> Option<RecordId> {
    let wanted = folded_words(name);
    programs
        .iter()
        .filter(|program| program.platform == platform)
        .find(|program| program.key == key || program.names.contains(&wanted))
        .map(|program| program.id)
}
