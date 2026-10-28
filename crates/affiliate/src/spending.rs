//! The Promotion Spends laid on the Commissions they paid for (#196, ADR
//! 0037 decision 7). The caller reads each channel's spend from the
//! Marketing; a channel's spend in a currency falls on the Commissions
//! earned on it in that currency, by what each earned, so a product's share
//! follows what it earned and the parts add up to the channel's spend
//! exactly. A Commission that earned nothing gets no share, and a channel
//! on which nothing was earned keeps its spend to itself.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use mascate_kernel::{ChannelSpend, Money, RecordId};
use rust_decimal::Decimal;

use crate::affiliate_links::AffiliateLink;
use crate::commissions::Commission;
use crate::source::AffiliatePlatform;

/// The decimal places of a share: far below a cent, and few enough that
/// adding shares up never runs out of digits, so their sums stay exact.
const SHARE_PLACES: u32 = 10;

/// `total` split by `weights`; a weight of zero or less gets nothing, and
/// `None` when no weight is above zero. The last part takes what the
/// others left, so the parts add up to `total` exactly.
fn split<K>(total: Money, weights: Vec<(K, Decimal)>) -> Option<Vec<(K, Money)>> {
    let weights: Vec<(K, Decimal)> = weights
        .into_iter()
        .filter(|(_, weight)| *weight > Decimal::ZERO)
        .collect();
    let whole: Decimal = weights.iter().map(|(_, weight)| *weight).sum();
    let last = weights.len().checked_sub(1)?;
    let mut left = total.amount();
    Some(
        weights
            .into_iter()
            .enumerate()
            .map(|(at, (key, weight))| {
                let part = if at == last {
                    left
                } else {
                    (total.amount() * weight / whole).round_dp(SHARE_PLACES)
                };
                left -= part;
                (key, Money::new(part, total.currency()))
            })
            .collect(),
    )
}

/// `spends` summed per channel and currency, in the order of the channel
/// ids; nothing at or below zero.
fn summed(spends: &[ChannelSpend]) -> Vec<ChannelSpend> {
    let mut sums: BTreeMap<(RecordId, &'static str), ChannelSpend> = BTreeMap::new();
    for spend in spends {
        let key = (spend.channel, spend.amount.currency().code());
        let sum = sums.entry(key).or_insert(ChannelSpend {
            channel: spend.channel,
            amount: Money::zero(spend.amount.currency()),
        });
        sum.amount = Money::new(
            sum.amount.amount() + spend.amount.amount(),
            spend.amount.currency(),
        );
    }
    sums.into_values()
        .filter(|spend| spend.amount.amount() > Decimal::ZERO)
        .collect()
}

/// What `commission` earned that weighs on its channel's spend in `spend`'s
/// currency.
fn earned_on(commission: &Commission, spend: &ChannelSpend) -> Option<Decimal> {
    (commission.channel == Some(spend.channel)
        && commission.status.is_earned()
        && commission.amount.currency() == spend.amount.currency())
    .then(|| commission.amount.amount())
}

/// The spends laid on the Commissions.
pub(crate) struct Shares {
    /// Each earned Commission's share, by its id.
    pub by_commission: HashMap<RecordId, Money>,
    /// Each channel's spend, summed per currency.
    pub by_channel: Vec<ChannelSpend>,
    /// The spends of the channels on which nothing was earned in their
    /// currency.
    pub unshared: Vec<ChannelSpend>,
}

impl Shares {
    pub(crate) fn of(commissions: &[&Commission], spends: &[ChannelSpend]) -> Self {
        let by_channel = summed(spends);
        let mut by_commission = HashMap::new();
        let mut unshared = Vec::new();
        for spend in &by_channel {
            let weights = commissions
                .iter()
                .filter_map(|commission| Some((commission.id, earned_on(commission, spend)?)))
                .collect();
            match split(spend.amount, weights) {
                Some(parts) => by_commission.extend(parts),
                None => unshared.push(*spend),
            }
        }
        Self {
            by_commission,
            by_channel,
            unshared,
        }
    }

    /// `commission`'s share, if it has one.
    pub(crate) fn of_commission(&self, commission: &Commission) -> Option<Money> {
        self.by_commission.get(&commission.id).copied()
    }
}

/// The part of each channel's spend that falls on `platform`, for a Work
/// Context that shows only it: by what each Platform earned on the channel
/// in its currency, as the report splits it among the Commissions. A
/// channel on which nothing was earned falls evenly on the Platforms of its
/// live Affiliate Links, and on none without a link. `commissions` are the
/// period's of every Platform.
pub fn spends_on(
    platform: AffiliatePlatform,
    spends: &[ChannelSpend],
    commissions: &[Commission],
    links: &[AffiliateLink],
) -> Vec<ChannelSpend> {
    let commissions: Vec<&Commission> = commissions.iter().collect();
    let shares = Shares::of(&commissions, spends);
    let mut on_platform: Vec<ChannelSpend> = Vec::new();
    for commission in commissions
        .iter()
        .filter(|commission| commission.platform == platform)
    {
        if let (Some(share), Some(channel)) = (shares.of_commission(commission), commission.channel)
        {
            on_platform.push(ChannelSpend {
                channel,
                amount: share,
            });
        }
    }
    for spend in &shares.unshared {
        let platforms: BTreeSet<AffiliatePlatform> = links
            .iter()
            .filter(|link| link.channel == spend.channel)
            .map(|link| link.program.platform)
            .collect();
        let even = platforms
            .into_iter()
            .map(|platform| (platform, Decimal::ONE))
            .collect();
        for (_, part) in split(spend.amount, even)
            .unwrap_or_default()
            .into_iter()
            .filter(|(of, _)| *of == platform)
        {
            on_platform.push(ChannelSpend {
                channel: spend.channel,
                amount: part,
            });
        }
    }
    summed(&on_platform)
}
