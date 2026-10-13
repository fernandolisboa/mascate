//! The commissions dashboard (#81): a period's Commissions per currency,
//! split by status and grouped by Platform, Program, product, tracking tag
//! and Promotion Channel, to see what earns most (stories 81, 83 and 85).
//! Cancelled sales never count; reversals stay out of what was earned. The
//! channel comes from the Affiliate Link whose marking the sale carries
//! (#101, ADR 0031); a tag with no link only shows by tracking.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use mascate_kernel::{Currency, Money, RecordId};

use crate::commissions::Commission;
use crate::source::{AffiliatePlatform, CommissionStatus};
use crate::totals::CurrencyTotals;

/// A Program as the dashboard groups it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProgramRef {
    pub id: RecordId,
    pub platform: AffiliatePlatform,
    pub name: String,
}

/// The Commissions of one Platform, Program, product or tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommissionGroup<K> {
    pub of: K,
    pub totals: CurrencyTotals,
    pub commissions: usize,
}

/// How the groups are ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GroupOrder {
    /// The most earned first.
    #[default]
    Earned,
    /// The most pending first.
    Pending,
    /// The most reversed first.
    Reversed,
    /// By name, the untagged last.
    Name,
}

/// The Commissions in one currency, summed up and grouped five ways. Each
/// grouping holds every Commission once, so its groups add up to `totals`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommissionReport {
    pub currency: Currency,
    pub totals: CurrencyTotals,
    pub commissions: usize,
    pub by_platform: Vec<CommissionGroup<AffiliatePlatform>>,
    pub by_program: Vec<CommissionGroup<ProgramRef>>,
    pub by_product: Vec<CommissionGroup<String>>,
    /// `None` gathers the Commissions without a tag.
    pub by_tracking: Vec<CommissionGroup<Option<String>>>,
    /// By Promotion Channel id, `None` (no channel) always last. The names
    /// live in the Marketing, so ties and [`GroupOrder::Name`] go by id
    /// until the caller names them with [`CommissionReport::arrange_channels`].
    pub by_channel: Vec<CommissionGroup<Option<RecordId>>>,
}

impl CommissionReport {
    /// One report per currency of `commissions`, in the order of the
    /// currency codes, the cancelled ones left out: there never was money
    /// in them. A currency with only cancelled Commissions has no report.
    pub fn of(commissions: &[Commission], order: GroupOrder) -> Vec<Self> {
        let mut by_currency: BTreeMap<&'static str, Vec<&Commission>> = BTreeMap::new();
        for commission in commissions {
            if commission.status != CommissionStatus::Cancelled {
                by_currency
                    .entry(commission.amount.currency().code())
                    .or_default()
                    .push(commission);
            }
        }
        by_currency
            .into_values()
            .map(|commissions| Self::of_one_currency(&commissions, order))
            .collect()
    }

    fn of_one_currency(commissions: &[&Commission], order: GroupOrder) -> Self {
        let currency = commissions[0].amount.currency();
        let mut by_platform = groups(commissions, currency, |c| c.platform);
        let mut by_program = groups(commissions, currency, |c| ProgramRef {
            id: c.program,
            platform: c.platform,
            name: c.program_name.clone(),
        });
        let mut by_product = groups(commissions, currency, |c| c.product.trim().to_owned());
        let mut by_tracking = groups(commissions, currency, |c| tag(c.tracking.as_deref()));
        sort(&mut by_platform, order, |of| Some(of.name().to_owned()));
        sort(&mut by_program, order, |of| Some(of.name.clone()));
        sort(&mut by_product, order, |of| Some(of.clone()));
        sort(&mut by_tracking, order, Clone::clone);
        let mut report = Self {
            currency,
            totals: sum(commissions, currency),
            commissions: commissions.len(),
            by_platform,
            by_program,
            by_product,
            by_tracking,
            by_channel: groups(commissions, currency, |c| c.channel),
        };
        report.arrange_channels(order, |_| String::new());
        report
    }

    /// Orders `by_channel` by `order`, with each channel named by `name`;
    /// the Commissions without a channel stay last whatever they sum.
    pub fn arrange_channels(&mut self, order: GroupOrder, name: impl Fn(RecordId) -> String) {
        sort(&mut self.by_channel, order, |of| of.map(&name));
        // A stable sort keeps the order above among the channels.
        self.by_channel.sort_by_key(|group| group.of.is_none());
    }
}

/// A tag as the owner typed it in the link; blank is no tag.
fn tag(tracking: Option<&str>) -> Option<String> {
    tracking
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_owned)
}

fn sum(commissions: &[&Commission], currency: Currency) -> CurrencyTotals {
    let mut totals = CurrencyTotals::new(currency);
    for commission in commissions {
        totals.add(commission);
    }
    totals
}

fn groups<K: Ord>(
    commissions: &[&Commission],
    currency: Currency,
    key: impl Fn(&Commission) -> K,
) -> Vec<CommissionGroup<K>> {
    let mut grouped: BTreeMap<K, Vec<&Commission>> = BTreeMap::new();
    for commission in commissions {
        grouped.entry(key(commission)).or_default().push(commission);
    }
    grouped
        .into_iter()
        .map(|(of, commissions)| CommissionGroup {
            of,
            totals: sum(&commissions, currency),
            commissions: commissions.len(),
        })
        .collect()
}

/// Orders `groups` by `order`; ties go by name, groups without one last.
fn sort<K>(
    groups: &mut Vec<CommissionGroup<K>>,
    order: GroupOrder,
    name: impl Fn(&K) -> Option<String>,
) {
    let most_first = |pick: fn(&CurrencyTotals) -> Money| {
        move |a: &CommissionGroup<K>, b: &CommissionGroup<K>| {
            pick(&b.totals).amount().cmp(&pick(&a.totals).amount())
        }
    };
    let mut named: Vec<(Option<String>, CommissionGroup<K>)> = groups
        .drain(..)
        .map(|group| (name(&group.of), group))
        .collect();
    named.sort_by(|(a_name, a), (b_name, b)| {
        let first = match order {
            GroupOrder::Earned => most_first(CurrencyTotals::earned)(a, b),
            GroupOrder::Pending => most_first(|totals| totals.pending)(a, b),
            GroupOrder::Reversed => most_first(|totals| totals.reversed)(a, b),
            GroupOrder::Name => Ordering::Equal,
        };
        first.then_with(|| match (a_name, b_name) {
            (Some(a), Some(b)) => a.cmp(b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        })
    });
    groups.extend(named.into_iter().map(|(_, group)| group));
}
