//! Short Links (#158, ADR 0035 decision 8): an Affiliate Link shortened by
//! an external shortener that counts its clicks, at most one per link,
//! created when the owner asks. Shortening is idempotent: the path comes
//! from the Affiliate Link, so asking again gives back the same Short Link.
//! The Platforms whose terms forbid a shortener refuse it while their
//! Restricted Feature is off.

use std::ops::RangeInclusive;

use chrono::NaiveDate;
use libsql::{TransactionBehavior, params};
use mascate_kernel::{PlatformError, RecordId};
use mascate_platform::{Flag, FlagKind, Migration, Phase, stored};

use crate::affiliate_links::AffiliateLink;
use crate::clicks::DayClicks;
use crate::commissions::{Affiliate, AffiliateError};
use crate::rules::{Promotion, RuleWarning};
use crate::source::AffiliatePlatform;

pub(crate) const CREATE_SHORT_LINKS: Migration = Migration {
    version: 6,
    name: "create the Short Links of Affiliate Links",
    risky: false,
    sql: "CREATE TABLE affiliate_short_links (
        id            TEXT PRIMARY KEY,
        link_id       TEXT NOT NULL REFERENCES affiliate_links (id),
        short         TEXT NOT NULL,
        shortener_key TEXT NOT NULL,
        created_at    TEXT NOT NULL,
        updated_at    TEXT NOT NULL,
        deleted_at    TEXT
    );
    CREATE UNIQUE INDEX affiliate_short_links_by_link
        ON affiliate_short_links (link_id)
        WHERE deleted_at IS NULL;",
};

/// A Short Link in front of an Amazon affiliate link.
pub const AMAZON_SHORT_LINK: Flag = Flag {
    key: "affiliate.amazon_short_link",
    name: "Link curto da Amazon",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "A política do Associados da Amazon não paga comissão por clique que passa por um \
             link de redirecionamento, como o de um encurtador, que esconde o link da Amazon.",
    risk: "A Amazon pode não pagar as comissões dessas vendas e encerrar a sua conta de \
           Associado.",
};

/// A Short Link in front of a Shopee affiliate link.
pub const SHOPEE_SHORT_LINK: Flag = Flag {
    key: "affiliate.shopee_short_link",
    name: "Link curto da Shopee por outro encurtador",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "Os termos da Shopee Afiliados (2.2) proíbem alterar o link de afiliado. O link \
             curto que a própria Shopee devolve continua valendo.",
    risk: "A Shopee pode cancelar as comissões dessas vendas e encerrar a sua conta de \
           afiliado.",
};

/// A Short Link in front of a Mercado Livre affiliate link.
pub const MERCADO_LIVRE_SHORT_LINK: Flag = Flag {
    key: "affiliate.mercado_livre_short_link",
    name: "Link curto do Mercado Livre",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "O programa de afiliados do Mercado Livre proíbe encurtar ou mascarar o link de \
             afiliado (cláusula 1.8, pelo que o Mercado Livre responde em público; a página do \
             termo não pôde ser lida).",
    risk: "O Mercado Livre pode cancelar as comissões dessas vendas e encerrar a sua conta de \
           afiliado.",
};

/// The Restricted Feature a Short Link of `platform` needs, when its terms
/// forbid a shortener.
pub fn short_link_flag(platform: AffiliatePlatform) -> Option<Flag> {
    match platform {
        AffiliatePlatform::Amazon => Some(AMAZON_SHORT_LINK),
        AffiliatePlatform::Shopee => Some(SHOPEE_SHORT_LINK),
        AffiliatePlatform::MercadoLivre => Some(MERCADO_LIVRE_SHORT_LINK),
        AffiliatePlatform::Hotmart
        | AffiliatePlatform::Kiwify
        | AffiliatePlatform::Eduzz
        | AffiliatePlatform::Monetizze
        | AffiliatePlatform::Magalu
        | AffiliatePlatform::Other => None,
    }
}

/// A link as the shortener made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortened {
    /// What the owner shares.
    pub url: String,
    /// The shortener's id for it, to read its clicks.
    pub key: String,
}

/// How an external shortener shortens a link on the owner's domain and
/// counts its clicks.
pub trait LinkShortener: Send + Sync {
    /// The Short Link at `path` that leads to `link`. The same link and
    /// path give back the same Short Link.
    fn shorten(&self, link: &str, path: &str) -> Result<Shortened, PlatformError>;

    /// The clicks on the Short Link the shortener calls `key`, on each of
    /// `days` of Brasília; `NotFound` when the shortener no longer has it.
    fn clicks(
        &self,
        key: &str,
        days: RangeInclusive<NaiveDate>,
    ) -> Result<Vec<DayClicks>, PlatformError>;
}

/// What shortening a link gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortenedLink {
    /// The Affiliate Link, its Short Link included.
    pub link: AffiliateLink,
    /// The rules a shortener breaks or may break on its Program.
    pub warnings: Vec<RuleWarning>,
    /// The link already had its Short Link, given back as it was.
    pub existed: bool,
}

/// How many characters of the link's id a path keeps: 36^8 holds its last
/// 41 bits, random in a UUIDv7, so two links never share a path.
const PATH_CHARS: usize = 8;
const PATH_BITS: u32 = 41;

/// The path of `link`'s Short Link: always the same for the same link,
/// lowercase letters and digits only.
fn path_of(link: RecordId) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut left = link.as_u128() & ((1 << PATH_BITS) - 1);
    let mut path = [b'0'; PATH_CHARS];
    for at in (0..PATH_CHARS).rev() {
        path[at] = DIGITS[(left % 36) as usize];
        left /= 36;
    }
    String::from_utf8_lossy(&path).into_owned()
}

impl Affiliate {
    /// Gives `link` its Short Link through `shortener`, after its Program's
    /// rules: a Platform whose terms forbid a shortener refuses while its
    /// Restricted Feature is off, any other rule only warns. A link that
    /// already has one gives it back without asking the shortener again.
    pub async fn shorten(
        &self,
        link: RecordId,
        shortener: &dyn LinkShortener,
    ) -> Result<ShortenedLink, AffiliateError> {
        let found = self.link(link).await?;
        let warnings = self
            .ensure_allowed(
                found.program.id,
                &Promotion {
                    shortened: true,
                    ..Promotion::default()
                },
            )
            .await?;
        if found.short.is_some() {
            return Ok(ShortenedLink {
                link: found,
                warnings,
                existed: true,
            });
        }
        let shortened = shortener.shorten(&found.tagged, &path_of(link))?;

        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let inserted = transaction
            .execute(
                "INSERT INTO affiliate_short_links
                     (id, link_id, short, shortener_key, created_at, updated_at)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?5
                 WHERE EXISTS (
                     SELECT 1 FROM affiliate_links WHERE id = ?2 AND deleted_at IS NULL
                 )
                 ON CONFLICT (link_id) WHERE deleted_at IS NULL DO NOTHING",
                params![
                    self.ids.next_id().to_string(),
                    link.to_string(),
                    shortened.url,
                    shortened.key,
                    stored(self.clock.now())
                ],
            )
            .await?;
        transaction.commit().await?;
        Ok(ShortenedLink {
            link: self.link(link).await?,
            warnings,
            // Another call shortened it in between.
            existed: inserted == 0,
        })
    }
}
