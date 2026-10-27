//! The commissions dashboard (#81): a period's Commissions per currency,
//! split by status and grouped by Platform, Program, product, tracking tag
//! and Promotion Channel, to see what earns most (stories 81, 83 and 85).
//! Cancelled sales never count; reversals stay out of what was earned. The
//! channel comes from the Affiliate Link whose marking the sale carries
//! (#101, ADR 0031); a tag with no link only shows by tracking. Beside
//! them, the Promotion Spends the caller read from the Marketing (#196, ADR
//! 0037 decision 7): each group's spend, profit, ROAS and spend per sale.
//! Profit lives within one currency: a group holding money in two, with a
//! spend on one side, has none, since nothing is converted.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use mascate_kernel::{ChannelSpend, Currency, Money, RecordId, Roas, spent_enough_to_judge};
use rust_decimal::Decimal;

use crate::commissions::Commission;
use crate::source::{AffiliatePlatform, CommissionStatus};
use crate::spending::Shares;
use crate::totals::CurrencyTotals;

/// A Program as the dashboard groups it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProgramRef {
    pub id: RecordId,
    pub platform: AffiliatePlatform,
    pub name: String,
}

/// The Commissions of one Platform, Program, product, tag or channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommissionGroup<K> {
    pub of: K,
    pub totals: CurrencyTotals,
    pub commissions: usize,
    /// Pending, approved or paid: the sales that did not fall through.
    pub sales: usize,
    /// Its part of the period's Promotion Spends in this currency: a
    /// channel's own spend, else the shares of its Commissions.
    pub spent: Money,
    /// It holds money in another currency too, with a spend on one side:
    /// nothing is converted, so it has no profit.
    pub mixed_currencies: bool,
}

impl<K> CommissionGroup<K> {
    /// What it earned less what it spent.
    pub fn profit(&self) -> Option<Money> {
        (!self.mixed_currencies).then(|| less(self.totals.earned(), self.spent))
    }

    /// The profit should the pending Commissions be approved.
    pub fn profit_if_pending_approve(&self) -> Option<Money> {
        (!self.mixed_currencies).then(|| less(self.totals.reachable(), self.spent))
    }

    /// What it earned for each unit spent; `None` without a spend.
    pub fn roas(&self) -> Option<Roas> {
        if self.mixed_currencies {
            return None;
        }
        roas(self.totals.earned(), self.spent)
    }

    /// What each sale cost; `None` without a spend or a sale.
    pub fn spent_per_sale(&self) -> Option<Money> {
        let sales = Decimal::from(self.sales);
        (!self.spent.amount().is_zero() && !sales.is_zero())
            .then(|| Money::new(self.spent.amount() / sales, self.spent.currency()))
    }

    /// Whether to suggest pausing what it paid for: it spent more than it
    /// earned, and at least [`mascate_kernel::ADS_LOSS_MIN_COST`]. Only
    /// ever a suggestion: the app never pauses an ad on another Platform.
    pub fn suggests_pausing(&self) -> bool {
        self.profit().is_some_and(|profit| profit.is_negative())
            && spent_enough_to_judge(self.spent)
    }
}

fn less(earned: Money, spent: Money) -> Money {
    Money::new(earned.amount() - spent.amount(), earned.currency())
}

fn roas(earned: Money, spent: Money) -> Option<Roas> {
    Roas::of(earned, spent).expect("one report's currency")
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
    /// The most profit first, those without one last.
    Profit,
    /// By name, the untagged last.
    Name,
}

/// The Commissions and Promotion Spends in one currency, summed up and
/// grouped five ways. Each grouping holds every Commission once, so its
/// groups add up to `totals`; `by_channel` holds every spend, and the other
/// groupings all of it but `unshared`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommissionReport {
    pub currency: Currency,
    pub totals: CurrencyTotals,
    pub commissions: usize,
    /// The period's Promotion Spends.
    pub spent: Money,
    /// What channels spent with nothing earned on them: only `by_channel`
    /// holds it, since no Commission takes a share.
    pub unshared: Money,
    pub by_platform: Vec<CommissionGroup<AffiliatePlatform>>,
    pub by_program: Vec<CommissionGroup<ProgramRef>>,
    pub by_product: Vec<CommissionGroup<String>>,
    /// `None` gathers the Commissions without a tag.
    pub by_tracking: Vec<CommissionGroup<Option<String>>>,
    /// By Promotion Channel id, `None` (no channel) always last; a channel
    /// that spent shows even without a sale. The names live in the
    /// Marketing, so ties and [`GroupOrder::Name`] go by id until the
    /// caller names them with [`CommissionReport::arrange_channels`].
    pub by_channel: Vec<CommissionGroup<Option<RecordId>>>,
}

impl CommissionReport {
    /// One report per currency of `commissions` and `spends`, the period's
    /// Commissions of every status and what each Promotion Channel spent,
    /// in the order of the currency codes. The cancelled Commissions are
    /// left out: there never was money in them. A currency with only
    /// cancelled Commissions and no spend has no report.
    pub fn of(commissions: &[Commission], spends: &[ChannelSpend], order: GroupOrder) -> Vec<Self> {
        let mut by_currency: BTreeMap<&'static str, (Currency, Vec<&Commission>)> = BTreeMap::new();
        for commission in commissions {
            if commission.status != CommissionStatus::Cancelled {
                let currency = commission.amount.currency();
                by_currency
                    .entry(currency.code())
                    .or_insert((currency, Vec::new()))
                    .1
                    .push(commission);
            }
        }
        let live: Vec<&Commission> = by_currency
            .values()
            .flat_map(|(_, commissions)| commissions.iter().copied())
            .collect();
        let shares = Shares::of(&live, spends);
        for spend in &shares.by_channel {
            let currency = spend.amount.currency();
            by_currency
                .entry(currency.code())
                .or_insert((currency, Vec::new()));
        }
        let mut reports: Vec<Self> = by_currency
            .into_values()
            .map(|(currency, commissions)| Self::of_one_currency(currency, &commissions, &shares))
            .collect();
        mark_mixed(&mut reports, |report| &mut report.by_platform);
        mark_mixed(&mut reports, |report| &mut report.by_program);
        mark_mixed(&mut reports, |report| &mut report.by_product);
        mark_mixed(&mut reports, |report| &mut report.by_tracking);
        mark_mixed(&mut reports, |report| &mut report.by_channel);
        for report in &mut reports {
            report.sort(order);
        }
        reports
    }

    fn of_one_currency(currency: Currency, commissions: &[&Commission], shares: &Shares) -> Self {
        let spent_in = |spends: &[ChannelSpend]| {
            let amounts = spends
                .iter()
                .filter(|spend| spend.amount.currency() == currency)
                .map(|spend| spend.amount);
            Money::sum(currency, amounts).expect("filtered to one currency")
        };
        let mut by_channel = groups(commissions, currency, shares, |c| c.channel);
        for spend in shares
            .by_channel
            .iter()
            .filter(|spend| spend.amount.currency() == currency)
        {
            match by_channel
                .iter_mut()
                .find(|group| group.of == Some(spend.channel))
            {
                Some(group) => group.spent = spend.amount,
                None => by_channel.push(CommissionGroup {
                    of: Some(spend.channel),
                    totals: CurrencyTotals::new(currency),
                    commissions: 0,
                    sales: 0,
                    spent: spend.amount,
                    mixed_currencies: false,
                }),
            }
        }
        Self {
            currency,
            totals: sum(commissions, currency),
            commissions: commissions.len(),
            spent: spent_in(&shares.by_channel),
            unshared: spent_in(&shares.unshared),
            by_platform: groups(commissions, currency, shares, |c| c.platform),
            by_program: groups(commissions, currency, shares, |c| ProgramRef {
                id: c.program,
                platform: c.platform,
                name: c.program_name.clone(),
            }),
            by_product: groups(commissions, currency, shares, |c| {
                c.product.trim().to_owned()
            }),
            by_tracking: groups(commissions, currency, shares, |c| {
                tag(c.tracking.as_deref())
            }),
            by_channel,
        }
    }

    fn sort(&mut self, order: GroupOrder) {
        sort(&mut self.by_platform, order, |of| {
            Some(of.name().to_owned())
        });
        sort(&mut self.by_program, order, |of| Some(of.name.clone()));
        sort(&mut self.by_product, order, |of| Some(of.clone()));
        sort(&mut self.by_tracking, order, Clone::clone);
        self.arrange_channels(order, |_| String::new());
    }

    /// Orders `by_channel` by `order`, with each channel named by `name`;
    /// the Commissions without a channel stay last whatever they sum.
    pub fn arrange_channels(&mut self, order: GroupOrder, name: impl Fn(RecordId) -> String) {
        sort(&mut self.by_channel, order, |of| of.map(&name));
        // A stable sort keeps the order above among the channels.
        self.by_channel.sort_by_key(|group| group.of.is_none());
    }

    /// What was earned less what was spent, in this currency alone.
    pub fn profit(&self) -> Money {
        less(self.totals.earned(), self.spent)
    }

    /// The profit should the pending Commissions be approved.
    pub fn profit_if_pending_approve(&self) -> Money {
        less(self.totals.reachable(), self.spent)
    }

    /// What was earned for each unit spent; `None` without a spend.
    pub fn roas(&self) -> Option<Roas> {
        roas(self.totals.earned(), self.spent)
    }
}

/// Marks the groups of a key found in several currencies, one of them with
/// a spend: they have no profit.
fn mark_mixed<K: Ord + Clone>(
    reports: &mut [CommissionReport],
    groups: fn(&mut CommissionReport) -> &mut Vec<CommissionGroup<K>>,
) {
    let mut seen: BTreeMap<K, (usize, bool)> = BTreeMap::new();
    for report in reports.iter_mut() {
        for group in groups(report).iter() {
            let (currencies, spent) = seen.entry(group.of.clone()).or_default();
            *currencies += 1;
            *spent |= !group.spent.amount().is_zero();
        }
    }
    for report in reports.iter_mut() {
        for group in groups(report).iter_mut() {
            let (currencies, spent) = seen[&group.of];
            group.mixed_currencies = currencies > 1 && spent;
        }
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
    shares: &Shares,
    key: impl Fn(&Commission) -> K,
) -> Vec<CommissionGroup<K>> {
    let mut grouped: BTreeMap<K, Vec<&Commission>> = BTreeMap::new();
    for commission in commissions {
        grouped.entry(key(commission)).or_default().push(commission);
    }
    grouped
        .into_iter()
        .map(|(of, commissions)| {
            let shared = commissions
                .iter()
                .filter_map(|commission| shares.of_commission(commission));
            CommissionGroup {
                of,
                totals: sum(&commissions, currency),
                commissions: commissions.len(),
                sales: commissions
                    .iter()
                    .filter(|commission| commission.status != CommissionStatus::Reversed)
                    .count(),
                spent: Money::sum(currency, shared).expect("shares in the group's currency"),
                mixed_currencies: false,
            }
        })
        .collect()
}

/// Orders `groups` by `order`; ties go by name, groups without one last.
fn sort<K>(
    groups: &mut Vec<CommissionGroup<K>>,
    order: GroupOrder,
    name: impl Fn(&K) -> Option<String>,
) {
    let amount = |group: &CommissionGroup<K>| match order {
        GroupOrder::Earned => Some(group.totals.earned().amount()),
        GroupOrder::Pending => Some(group.totals.pending.amount()),
        GroupOrder::Reversed => Some(group.totals.reversed.amount()),
        GroupOrder::Profit => group.profit().map(|profit| profit.amount()),
        GroupOrder::Name => None,
    };
    let mut named: Vec<(Option<String>, CommissionGroup<K>)> = groups
        .drain(..)
        .map(|group| (name(&group.of), group))
        .collect();
    named.sort_by(|(a_name, a), (b_name, b)| {
        most_first(amount(a), amount(b)).then_with(|| match (a_name, b_name) {
            (Some(a), Some(b)) => a.cmp(b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        })
    });
    groups.extend(named.into_iter().map(|(_, group)| group));
}

/// The larger first, a missing one last.
fn most_first(a: Option<Decimal>, b: Option<Decimal>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}
