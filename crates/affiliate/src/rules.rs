//! Promotion rules (#98, ADR 0031 decisions 1 and 2): what each Program
//! allows, forbids or leaves unread, the check that warns before a
//! promotion breaks one, and the three Restricted Features that refuse
//! instead of warning while their flag is off.
//!
//! A Program's rules start as its Platform's defaults: a rule the owner
//! never edited is stored empty and read as the default, so a Program born
//! in a Sync and one born before these rules existed both get them, and no
//! Sync ever writes a rule.

use libsql::params;
use mascate_kernel::{RecordId, Timestamp, folded_words};
use mascate_platform::{Flag, FlagKind, Migration, Phase, StoredRow, stored};

use crate::commissions::{Affiliate, AffiliateError, platform_at};
use crate::source::AffiliatePlatform;

pub(crate) const ADD_PROMOTION_RULES: Migration = Migration {
    version: 4,
    name: "add promotion rules to Programs",
    risky: false,
    sql: "ALTER TABLE affiliate_programs ADD COLUMN paid_search TEXT;
    ALTER TABLE affiliate_programs ADD COLUMN boosted_posts TEXT;
    ALTER TABLE affiliate_programs ADD COLUMN email TEXT;
    ALTER TABLE affiliate_programs ADD COLUMN direct_message TEXT;
    ALTER TABLE affiliate_programs ADD COLUMN offline TEXT;
    ALTER TABLE affiliate_programs ADD COLUMN shortener TEXT;
    ALTER TABLE affiliate_programs ADD COLUMN brand_keywords TEXT;
    ALTER TABLE affiliate_programs ADD COLUMN price_needs_time INTEGER;",
};

/// A Shopee affiliate link in paid search.
pub const SHOPEE_PAID_SEARCH: Flag = Flag {
    key: "affiliate.shopee_paid_search",
    name: "Link da Shopee em busca paga",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Two,
    reason: "Os termos da Shopee Afiliados (4.3(i)) proíbem anunciar link de afiliado em busca \
             paga, como Google Ads e Bing Ads.",
    risk: "A Shopee pode cancelar as comissões dessas vendas e encerrar a sua conta de afiliado.",
};

/// A keyword with a Platform's brand in paid search.
pub const BRAND_KEYWORDS: Flag = Flag {
    key: "affiliate.brand_keywords",
    name: "Palavra-chave com a marca da plataforma em busca paga",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Two,
    reason: "Amazon, Shopee (4.3(c)) e Hotmart proíbem comprar, em busca paga, palavra-chave \
             com a marca delas, como amazon, kindle, shopee ou hotmart.",
    risk: "A plataforma pode cancelar as comissões dessas vendas e encerrar a sua conta de \
           afiliado.",
};

/// An Amazon price the owner typed, shown in a promotion.
pub const AMAZON_TYPED_PRICE: Flag = Flag {
    key: "affiliate.amazon_typed_price",
    name: "Preço da Amazon digitado por você",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Two,
    reason: "A política do Associados da Amazon (versão de 14/04/2026) só deixa mostrar o preço \
             que vem de um link fornecido pela Amazon ou da API de produtos. Desligada, o post da \
             Amazon sai sem preço, só com o produto, o link e a identificação.",
    risk: "Um preço desatualizado ou sem a data e a hora pode levar a Amazon a reter as \
           comissões e encerrar a sua conta de Associado.",
};

/// Where a Program stands on a practice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Standing {
    Allowed,
    Forbidden,
    /// Terms nobody has read yet: warns like a forbidden one (ADR 0006).
    Unknown,
}

impl Standing {
    pub const ALL: [Standing; 3] = [Standing::Allowed, Standing::Forbidden, Standing::Unknown];

    fn code(self) -> &'static str {
        match self {
            Standing::Allowed => "allowed",
            Standing::Forbidden => "forbidden",
            Standing::Unknown => "unknown",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|standing| standing.code() == code)
    }
}

/// A way of promoting that a Program may allow or forbid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Practice {
    /// Ads on Google, Bing and other search engines.
    PaidSearch,
    /// Paying to boost a post on the owner's own social accounts.
    BoostedPosts,
    Email,
    /// A direct message or SMS nobody asked for.
    DirectMessage,
    /// Print, events, anything outside the internet.
    Offline,
    /// A link shortener in front of the affiliate link.
    Shortener,
}

impl Practice {
    pub const ALL: [Practice; 6] = [
        Practice::PaidSearch,
        Practice::BoostedPosts,
        Practice::Email,
        Practice::DirectMessage,
        Practice::Offline,
        Practice::Shortener,
    ];
}

/// What a Program allows when its products are promoted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionRules {
    /// By [`Practice::ALL`]'s order.
    standings: [Standing; 6],
    /// Words with the Platform's brand that paid search must not bid on.
    pub brand_keywords: Vec<String>,
    /// A price shown must carry the date and time it was seen.
    pub price_needs_time: bool,
}

impl PromotionRules {
    /// Every practice at `standing`, no brand keyword, no price rule.
    pub fn all(standing: Standing) -> Self {
        Self {
            standings: [standing; 6],
            brand_keywords: Vec::new(),
            price_needs_time: false,
        }
    }

    /// What `platform` allows by its terms (`docs/marketing.md` section 3).
    /// On infoproduct Platforms the producer decides per product, so every
    /// practice starts unread there.
    pub fn defaults(platform: AffiliatePlatform) -> Self {
        let unread = Self::all(Standing::Unknown);
        match platform {
            AffiliatePlatform::Shopee => Self {
                brand_keywords: vec!["shopee".into()],
                ..unread
                    .with(Practice::PaidSearch, Standing::Forbidden)
                    .with(Practice::Email, Standing::Forbidden)
                    .with(Practice::BoostedPosts, Standing::Allowed)
            },
            AffiliatePlatform::Amazon => Self {
                brand_keywords: vec!["amazon".into(), "kindle".into()],
                price_needs_time: true,
                ..unread
                    .with(Practice::Offline, Standing::Forbidden)
                    .with(Practice::Email, Standing::Forbidden)
                    .with(Practice::DirectMessage, Standing::Forbidden)
                    .with(Practice::Shortener, Standing::Forbidden)
            },
            AffiliatePlatform::Hotmart => Self {
                brand_keywords: vec!["hotmart".into()],
                ..unread
            },
            AffiliatePlatform::Kiwify
            | AffiliatePlatform::Eduzz
            | AffiliatePlatform::Monetizze
            | AffiliatePlatform::MercadoLivre
            | AffiliatePlatform::Magalu
            | AffiliatePlatform::Other => unread,
        }
    }

    pub fn with(mut self, practice: Practice, standing: Standing) -> Self {
        self.set(practice, standing);
        self
    }

    pub fn standing(&self, practice: Practice) -> Standing {
        self.standings[practice as usize]
    }

    pub fn set(&mut self, practice: Practice, standing: Standing) {
        self.standings[practice as usize] = standing;
    }

    /// Every rule `promotion` breaks or may break: a practice it takes up
    /// that is forbidden or unread, a brand keyword in paid search, a price
    /// without the time it was seen. Nothing it lists stops the promotion;
    /// the owner reads and decides.
    pub fn check(&self, promotion: &Promotion) -> Vec<RuleWarning> {
        let mut warnings: Vec<RuleWarning> = promotion
            .practices()
            .into_iter()
            .filter_map(|practice| match self.standing(practice) {
                Standing::Allowed => None,
                standing => Some(RuleWarning::Practice { practice, standing }),
            })
            .collect();
        warnings.extend(
            self.brand_keywords_in(promotion)
                .into_iter()
                .map(|keyword| RuleWarning::BrandKeyword(keyword.to_owned())),
        );
        if self.price_needs_time && promotion.price.is_some_and(|price| price.seen_at.is_none()) {
            warnings.push(RuleWarning::PriceWithoutTime);
        }
        warnings
    }

    /// The brand keywords a paid-search promotion bids on or writes, as
    /// whole words, case and accents aside. The terms forbid buying them
    /// in paid search; naming the Platform in a post is fine.
    fn brand_keywords_in(&self, promotion: &Promotion) -> Vec<&str> {
        if promotion.reach != Reach::PaidSearch {
            return Vec::new();
        }
        let written: Vec<Vec<String>> = promotion
            .keywords
            .iter()
            .chain([&promotion.text])
            .map(|text| folded_words(text))
            .collect();
        self.brand_keywords
            .iter()
            .filter(|keyword| {
                let brand = folded_words(keyword);
                !brand.is_empty()
                    && written
                        .iter()
                        .any(|words| words.windows(brand.len()).any(|run| run == brand))
            })
            .map(String::as_str)
            .collect()
    }

    /// The rules as kept: keywords trimmed, inner spaces collapsed, blank
    /// and repeated ones (case and accents aside) dropped.
    fn tidied(&self) -> Self {
        let mut keywords: Vec<String> = Vec::new();
        for keyword in &self.brand_keywords {
            let keyword = keyword.split_whitespace().collect::<Vec<_>>().join(" ");
            let folded = folded_words(&keyword);
            if !folded.is_empty() && !keywords.iter().any(|kept| folded_words(kept) == folded) {
                keywords.push(keyword);
            }
        }
        Self {
            brand_keywords: keywords,
            ..self.clone()
        }
    }
}

/// How far a promotion reaches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Reach {
    /// The owner's own channel or group, posted for free.
    #[default]
    OwnChannel,
    BoostedPost,
    PaidSearch,
    Email,
    DirectMessage,
    Offline,
}

impl Reach {
    /// The practice it takes up; posting on the owner's own channel is
    /// what affiliates do everywhere, so it takes none.
    pub fn practice(self) -> Option<Practice> {
        match self {
            Reach::OwnChannel => None,
            Reach::BoostedPost => Some(Practice::BoostedPosts),
            Reach::PaidSearch => Some(Practice::PaidSearch),
            Reach::Email => Some(Practice::Email),
            Reach::DirectMessage => Some(Practice::DirectMessage),
            Reach::Offline => Some(Practice::Offline),
        }
    }
}

/// A price a promotion shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShownPrice {
    /// The owner typed it, rather than a Platform's link or API giving it.
    pub typed: bool,
    pub seen_at: Option<Timestamp>,
}

/// A promotion as the caller describes it, in the Affiliate's words: the
/// app translates a Promotion Channel and an Offer Post into this (ADR
/// 0031, decision 1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Promotion {
    pub reach: Reach,
    /// The keywords paid search bids on.
    pub keywords: Vec<String>,
    pub text: String,
    /// The link goes through a shortener.
    pub shortened: bool,
    pub price: Option<ShownPrice>,
}

impl Promotion {
    fn practices(&self) -> Vec<Practice> {
        self.reach
            .practice()
            .into_iter()
            .chain(self.shortened.then_some(Practice::Shortener))
            .collect()
    }
}

/// A rule a promotion breaks or may break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleWarning {
    /// It takes up a practice the Program forbids or whose terms are unread.
    Practice {
        practice: Practice,
        standing: Standing,
    },
    /// It bids on or writes this brand keyword in paid search.
    BrandKeyword(String),
    /// It shows a price without the date and time it was seen.
    PriceWithoutTime,
    /// It uses a Restricted Feature the owner turned on.
    Restricted(Flag),
}

/// A Program with its promotion rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub id: RecordId,
    pub platform: AffiliatePlatform,
    pub name: String,
    pub rules: PromotionRules,
}

const PROGRAM_QUERY: &str =
    "SELECT id, platform, name, paid_search, boosted_posts, email, direct_message, offline,
         shortener, brand_keywords, price_needs_time
     FROM affiliate_programs WHERE deleted_at IS NULL";

impl Affiliate {
    /// Every Program, by Platform and name.
    pub async fn programs(&self) -> Result<Vec<Program>, AffiliateError> {
        let mut rows = self.database.connection().query(PROGRAM_QUERY, ()).await?;
        let mut programs = Vec::new();
        while let Some(row) = rows.next().await? {
            programs.push(program_from(&row)?);
        }
        programs.sort_by(|a, b| {
            (a.platform.name(), &a.name, a.id).cmp(&(b.platform.name(), &b.name, b.id))
        });
        Ok(programs)
    }

    pub async fn program(&self, program: RecordId) -> Result<Program, AffiliateError> {
        let mut rows = self
            .database
            .connection()
            .query(
                &format!("{PROGRAM_QUERY} AND id = ?1"),
                params![program.to_string()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => program_from(&row),
            None => Err(AffiliateError::NoSuchProgram),
        }
    }

    /// Keeps the owner's `rules` for `program`, in place of its Platform's
    /// defaults or of the rules edited before.
    pub async fn set_promotion_rules(
        &self,
        program: RecordId,
        rules: &PromotionRules,
    ) -> Result<(), AffiliateError> {
        let rules = rules.tidied();
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE affiliate_programs SET paid_search = ?1, boosted_posts = ?2, email = ?3,
                     direct_message = ?4, offline = ?5, shortener = ?6, brand_keywords = ?7,
                     price_needs_time = ?8, updated_at = ?9
                 WHERE id = ?10 AND deleted_at IS NULL",
                params![
                    rules.standing(Practice::PaidSearch).code(),
                    rules.standing(Practice::BoostedPosts).code(),
                    rules.standing(Practice::Email).code(),
                    rules.standing(Practice::DirectMessage).code(),
                    rules.standing(Practice::Offline).code(),
                    rules.standing(Practice::Shortener).code(),
                    rules.brand_keywords.join("\n"),
                    i64::from(rules.price_needs_time),
                    stored(self.clock.now()),
                    program.to_string()
                ],
            )
            .await?;
        if changed == 0 {
            return Err(AffiliateError::NoSuchProgram);
        }
        Ok(())
    }

    /// The guard before a promotion of `program`: fails with the flag's
    /// reason when it needs a Restricted Feature that is off (a Shopee link
    /// or a brand keyword in paid search, a typed Amazon price); otherwise
    /// gives every rule it breaks or may break, the Restricted Features
    /// turned on included, for the owner to read.
    pub async fn ensure_allowed(
        &self,
        program: RecordId,
        promotion: &Promotion,
    ) -> Result<Vec<RuleWarning>, AffiliateError> {
        let program = self.program(program).await?;
        let mut warnings = program.rules.check(promotion);
        for flag in restricted_features(&program, promotion) {
            if !self.flags.is_on(&flag).await? {
                return Err(AffiliateError::Restricted(flag));
            }
            warnings.push(RuleWarning::Restricted(flag));
        }
        Ok(warnings)
    }
}

/// The Restricted Features `promotion` needs. They are declared by the
/// Platform's terms, so the owner's edits to the rules never lift them.
fn restricted_features(program: &Program, promotion: &Promotion) -> Vec<Flag> {
    let paid_search = promotion.reach == Reach::PaidSearch;
    [
        (paid_search && program.platform == AffiliatePlatform::Shopee)
            .then_some(SHOPEE_PAID_SEARCH),
        (paid_search && !program.rules.brand_keywords_in(promotion).is_empty())
            .then_some(BRAND_KEYWORDS),
        (program.platform == AffiliatePlatform::Amazon
            && promotion.price.is_some_and(|price| price.typed))
        .then_some(AMAZON_TYPED_PRICE),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn program_from(row: &libsql::Row) -> Result<Program, AffiliateError> {
    let platform = platform_at(row, 1)?;
    let mut rules = PromotionRules::defaults(platform);
    for (practice, at) in Practice::ALL.into_iter().zip(3..) {
        if let Some(code) = row.get::<Option<String>>(at)? {
            let standing = Standing::from_code(&code).ok_or(AffiliateError::Unreadable(code))?;
            rules.set(practice, standing);
        }
    }
    if let Some(keywords) = row.get::<Option<String>>(9)? {
        rules.brand_keywords = keywords
            .lines()
            .filter(|keyword| !keyword.is_empty())
            .map(str::to_owned)
            .collect();
    }
    if let Some(needs) = row.get::<Option<i64>>(10)? {
        rules.price_needs_time = needs != 0;
    }
    Ok(Program {
        id: row.id_at(0)?,
        platform,
        name: row.get(2)?,
        rules,
    })
}
