//! Affiliate Links (#99, ADR 0031 decision 3): the affiliate link the owner
//! pasted for a Program's product, tagged with the marking of the Promotion
//! Channel where it is promoted. The channel is kept only by its id in the
//! Marketing; the marking comes back in each Conversion's tracking, so it
//! belongs to one channel per Platform.

use libsql::{TransactionBehavior, params};
use mascate_kernel::{RecordId, Timestamp};
use mascate_platform::{Migration, StoredRow, stored};

use crate::commissions::{Affiliate, AffiliateError, platform_at};
use crate::report::ProgramRef;
use crate::rules::{Promotion, RuleWarning};
use crate::tagging::LinkTagger;

pub(crate) const CREATE_AFFILIATE_LINKS: Migration = Migration {
    version: 5,
    name: "create the Affiliate Links tagged by Promotion Channel",
    risky: false,
    sql: "CREATE TABLE affiliate_links (
        id         TEXT PRIMARY KEY,
        program_id TEXT NOT NULL REFERENCES affiliate_programs (id),
        product    TEXT NOT NULL,
        channel_id TEXT NOT NULL,
        tag        TEXT NOT NULL,
        pasted     TEXT NOT NULL,
        tagged     TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE UNIQUE INDEX affiliate_links_by_channel
        ON affiliate_links (program_id, product, channel_id)
        WHERE deleted_at IS NULL;
    CREATE INDEX affiliate_links_by_tag ON affiliate_links (tag);",
};

/// The longest marking a link takes.
pub const TAG_MAX_CHARS: usize = 40;

/// A link to create, as the owner asks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAffiliateLink {
    pub program: RecordId,
    /// As the Platform names it.
    pub product: String,
    /// The affiliate link the Platform gave the owner.
    pub pasted: String,
    /// The Promotion Channel's id in the Marketing.
    pub channel: RecordId,
    /// The marking: the channel's own, or the Amazon tracking id the owner
    /// created for it.
    pub tag: String,
    /// How the channel promotes, in the Affiliate's words, for the rules.
    pub promotion: Promotion,
}

/// An affiliate link tagged for a Promotion Channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffiliateLink {
    pub id: RecordId,
    pub program: ProgramRef,
    pub product: String,
    pub channel: RecordId,
    pub tag: String,
    pub pasted: String,
    /// What the owner shares.
    pub tagged: String,
    pub created_at: Timestamp,
}

/// What creating a link gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedLink {
    pub link: AffiliateLink,
    /// The rules the channel breaks or may break, for the owner to read.
    pub warnings: Vec<RuleWarning>,
    /// The Program's product already had a link for the channel, given
    /// back as it was.
    pub existed: bool,
}

const LINK_QUERY: &str =
    "SELECT l.id, p.id, p.platform, p.name, l.product, l.channel_id, l.tag, l.pasted, l.tagged,
         l.created_at
     FROM affiliate_links l
     JOIN affiliate_programs p ON p.id = l.program_id
     WHERE l.deleted_at IS NULL";

impl Affiliate {
    /// Every link, by Platform, Program, product and creation.
    pub async fn links(&self) -> Result<Vec<AffiliateLink>, AffiliateError> {
        let mut rows = self.database.connection().query(LINK_QUERY, ()).await?;
        let mut links = Vec::new();
        while let Some(row) = rows.next().await? {
            links.push(link_from(&row)?);
        }
        links.sort_by(|a, b| {
            (
                a.program.platform.name(),
                &a.program.name,
                &a.product,
                a.created_at,
                a.id,
            )
                .cmp(&(
                    b.program.platform.name(),
                    &b.program.name,
                    &b.product,
                    b.created_at,
                    b.id,
                ))
        });
        Ok(links)
    }

    /// Tags the pasted link for the channel through `tagger`, after the
    /// Program's rules: a Restricted Feature that is off refuses, any other
    /// rule only warns. The same Program, product and channel give back
    /// the link they already have. A marking another channel used on the
    /// Platform is refused, so each Conversion points to one channel.
    pub async fn create_link(
        &self,
        new: &NewAffiliateLink,
        tagger: &dyn LinkTagger,
    ) -> Result<CreatedLink, AffiliateError> {
        let product = new.product.trim();
        if product.is_empty() {
            return Err(AffiliateError::NoProduct);
        }
        let tag = new.tag.trim();
        if !is_tag(tag) {
            return Err(AffiliateError::BadTag);
        }
        let program = self.program(new.program).await?;
        if tagger.platform() != program.platform {
            return Err(AffiliateError::WrongTagger);
        }
        let warnings = self.ensure_allowed(program.id, &new.promotion).await?;
        if let Some(link) = self.link_for(new.program, product, new.channel).await? {
            return Ok(CreatedLink {
                link,
                warnings,
                existed: true,
            });
        }
        let tagged = tagger.tag(new.pasted.trim(), tag)?;

        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        // Deleted links count too: Conversions from their clicks keep
        // arriving until the Platform settles them.
        let taken = transaction
            .query(
                "SELECT 1 FROM affiliate_links l
                 JOIN affiliate_programs p ON p.id = l.program_id
                 WHERE p.platform = ?1 AND l.tag = ?2 AND l.channel_id <> ?3",
                params![program.platform.code(), tag, new.channel.to_string()],
            )
            .await?
            .next()
            .await?
            .is_some();
        if taken {
            return Err(AffiliateError::TagTaken(tag.to_owned()));
        }
        let id = self.ids.next_id();
        let now = self.clock.now();
        let inserted = transaction
            .execute(
                "INSERT INTO affiliate_links
                     (id, program_id, product, channel_id, tag, pasted, tagged, created_at,
                      updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                 ON CONFLICT (program_id, product, channel_id) WHERE deleted_at IS NULL
                 DO NOTHING",
                params![
                    id.to_string(),
                    program.id.to_string(),
                    product,
                    new.channel.to_string(),
                    tag,
                    new.pasted.trim(),
                    tagged.clone(),
                    stored(now)
                ],
            )
            .await?;
        transaction.commit().await?;
        if inserted == 0 {
            // Another call created it in between.
            let link = self
                .link_for(new.program, product, new.channel)
                .await?
                .ok_or(AffiliateError::NoSuchLink)?;
            return Ok(CreatedLink {
                link,
                warnings,
                existed: true,
            });
        }
        Ok(CreatedLink {
            link: AffiliateLink {
                id,
                program: ProgramRef {
                    id: program.id,
                    platform: program.platform,
                    name: program.name,
                },
                product: product.to_owned(),
                channel: new.channel,
                tag: tag.to_owned(),
                pasted: new.pasted.trim().to_owned(),
                tagged,
                created_at: now,
            },
            warnings,
            existed: false,
        })
    }

    /// Takes a link off the list; its marking stays with its channel.
    pub async fn delete_link(&self, link: RecordId) -> Result<(), AffiliateError> {
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE affiliate_links SET deleted_at = ?1, updated_at = ?1
                 WHERE id = ?2 AND deleted_at IS NULL",
                params![stored(self.clock.now()), link.to_string()],
            )
            .await?;
        if changed == 0 {
            return Err(AffiliateError::NoSuchLink);
        }
        Ok(())
    }

    async fn link_for(
        &self,
        program: RecordId,
        product: &str,
        channel: RecordId,
    ) -> Result<Option<AffiliateLink>, AffiliateError> {
        let mut rows = self
            .database
            .connection()
            .query(
                &format!(
                    "{LINK_QUERY} AND l.program_id = ?1 AND l.product = ?2 AND l.channel_id = ?3"
                ),
                params![program.to_string(), product, channel.to_string()],
            )
            .await?;
        rows.next().await?.map(|row| link_from(&row)).transpose()
    }
}

/// A marking every Platform carries in a link as it is: lowercase letters,
/// digits, `_` and `-`.
fn is_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.chars().count() <= TAG_MAX_CHARS
        && tag
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn link_from(row: &libsql::Row) -> Result<AffiliateLink, AffiliateError> {
    Ok(AffiliateLink {
        id: row.id_at(0)?,
        program: ProgramRef {
            id: row.id_at(1)?,
            platform: platform_at(row, 2)?,
            name: row.get(3)?,
        },
        product: row.get(4)?,
        channel: row.id_at(5)?,
        tag: row.get(6)?,
        pasted: row.get(7)?,
        tagged: row.get(8)?,
        created_at: row.time_at(9)?,
    })
}
