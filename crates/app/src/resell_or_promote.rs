//! Revender ou divulgar (#86, story 91): a Product's Realized Margin from
//! selling it next to the Commissions earned promoting the products the
//! owner linked to it, per currency, on the Product's sheet and on the
//! Painel. The app reads the sales from Commerce and the Commissions from
//! Affiliate, and Finance compares them (ADR 0026, ADR 0030).

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::component::{StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Div, Entity, Hsla, Subscription, Window, div, px};
use mascate_affiliate::{
    Affiliate, Commission, CommissionFilter, CommissionStatus, PromotedProduct,
};
use mascate_finance::{Ahead, LinkedCommission, ResellOrPromote};
use mascate_kernel::{Money, RecordId, Timestamp};

use crate::ads::AdsSyncs;
use crate::affiliate::{self, CommissionChanges};
use crate::appearance::{Tokens, look};
use crate::dashboard::{SaleSources, SoldPeriod};
use crate::kit;
use crate::orders::OrderSyncs;
use crate::sales::{amount_text, margin_cell, orders_text, table_row};
use crate::span::{SpanChanged, SpanPicker};

/// The approved and paid Commissions of sales made during `during`.
pub(crate) async fn earned(
    affiliate: &Affiliate,
    during: Range<Timestamp>,
) -> Result<Vec<Commission>, String> {
    affiliate
        .commissions(&CommissionFilter {
            statuses: vec![CommissionStatus::Approved, CommissionStatus::Paid],
            sold: during,
        })
        .await
        .map_err(|e| affiliate::failure(&e))
}

/// Each of `earned` whose product the owner linked to a Product.
fn linked(earned: &[Commission]) -> Vec<LinkedCommission> {
    earned
        .iter()
        .filter_map(|commission| {
            Some(LinkedCommission {
                product: commission.linked_product?,
                amount: commission.amount,
            })
        })
        .collect()
}

/// The linked Products of a period on the Painel.
pub(crate) struct Compared {
    /// By the Product's name, each with its name.
    pub rows: Vec<(String, ResellOrPromote)>,
    /// Products the owner linked a promoted product to.
    pub linked: usize,
    /// Products promoted, linked or not.
    pub promoted: usize,
}

/// Every Product a promoted product is linked to, reselling against
/// promoting in the period `sold` covers.
pub(crate) async fn compare_linked(
    affiliate: &Affiliate,
    sold: &SoldPeriod,
    earned: &[Commission],
) -> Result<Compared, String> {
    let promoted = affiliate
        .promoted_products()
        .await
        .map_err(|e| affiliate::failure(&e))?;
    let mut products: Vec<(&String, RecordId)> = promoted
        .iter()
        .filter_map(|promoted| promoted.linked_to)
        // A link to a Product deleted from the Catalog compares nothing.
        .filter_map(|id| Some((sold.names.get(&id)?, id)))
        .collect();
    products.sort();
    products.dedup();
    let ids: Vec<RecordId> = products.iter().map(|(_, id)| *id).collect();
    let rows = ResellOrPromote::of(&ids, &sold.lines, &linked(earned))
        .map_err(|e| format!("Valores em moedas diferentes: {e}"))?
        .into_iter()
        .map(|compared| (sold.names[&compared.product].clone(), compared))
        .collect();
    Ok(Compared {
        rows,
        linked: ids.len(),
        promoted: promoted.len(),
    })
}

/// The tag that says which side left more money.
fn verdict(compared: &ResellOrPromote, t: &Tokens) -> (&'static str, Hsla) {
    match compared.ahead() {
        Some(Ahead::Reselling) => ("revender rende mais", t.success),
        Some(Ahead::Promoting) => ("divulgar rende mais", t.accent_text),
        Some(Ahead::Even) => ("empate", t.text2),
        None => ("sem custo para comparar", t.text2),
    }
}

/// "No período, revender deixou R$ 45,00 a mais que divulgar."
fn verdict_text(compared: &ResellOrPromote) -> String {
    let lead = compared.lead().map(Money::to_pt_br).unwrap_or_default();
    match compared.ahead() {
        Some(Ahead::Reselling) => {
            format!("No período, revender deixou {lead} a mais que divulgar.")
        }
        Some(Ahead::Promoting) => {
            format!("No período, divulgar rendeu {lead} a mais que revender.")
        }
        Some(Ahead::Even) => "No período, revender e divulgar renderam o mesmo.".into(),
        None => "As vendas do período não têm custo: as unidades não saíram do estoque do app, \
                 então não há margem para comparar."
            .into(),
    }
}

fn resold_text(compared: &ResellOrPromote) -> String {
    match &compared.reselling {
        Some(reselling) => reselling
            .margin
            .as_ref()
            .map_or_else(|| "sem custo".into(), amount_text),
        None => Money::zero(compared.currency).to_pt_br(),
    }
}

fn promoted_text(compared: &ResellOrPromote) -> String {
    compared
        .promoting
        .map_or(Money::zero(compared.currency), |promoting| promoting.earned)
        .to_pt_br()
}

/// Painel › Revender ou divulgar: one row per linked Product and currency.
pub(crate) fn painel_section(compared: &Compared, cx: &App) -> Option<AnyElement> {
    if compared.promoted == 0 {
        return None;
    }
    let t = look(cx).tokens;
    let section = v_flex()
        .gap_3()
        .child(kit::section_heading("Revender ou divulgar"));
    let note = |text: &'static str| div().text_sm().text_color(t.text2).child(text);
    if compared.linked == 0 {
        return Some(
            section
                .child(note(
                    "Nenhum produto divulgado ligado a um produto do catálogo. Ligue em \
                     Afiliação › Produtos divulgados para comparar a margem de revender com as \
                     comissões de divulgar.",
                ))
                .into_any_element(),
        );
    }
    if compared.rows.is_empty() {
        return Some(
            section
                .child(note(
                    "Nenhuma venda nem comissão ganha neste período nos produtos ligados.",
                ))
                .into_any_element(),
        );
    }
    let several = compared
        .rows
        .iter()
        .any(|(_, row)| row.currency != compared.rows[0].1.currency);
    let header = h_flex()
        .gap_3()
        .px_3()
        .text_xs()
        .text_color(t.text2)
        .child(div().flex_1().child("Produto"))
        .child(margin_cell().child("Margem revendendo"))
        .child(margin_cell().child("Comissões divulgando"))
        .child(margin_cell().child(""));
    let rows = compared.rows.iter().map(|(name, row)| {
        let (tag, ink) = verdict(row, &t);
        let name = if several {
            format!("{name} ({})", row.currency.code())
        } else {
            name.clone()
        };
        table_row(&t)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_medium()
                    .child(name),
            )
            .child(margin_cell().child(resold_text(row)))
            .child(margin_cell().child(promoted_text(row)))
            .child(
                margin_cell()
                    .flex()
                    .justify_end()
                    .child(kit::tag(tag, ink, cx)),
            )
            .into_any_element()
    });
    Some(
        section
            .child(v_flex().gap_2().child(header).children(rows))
            .child(div().text_xs().text_color(t.text2).child(
                "A margem é a realizada nas vendas do produto; as comissões são as aprovadas e \
                 pagas dos produtos divulgados ligados a ele em Afiliação, na moeda da \
                 plataforma e sem conversão. O detalhe fica na ficha de cada produto.",
            ))
            .into_any_element(),
    )
}

/// What one read of the card brings.
struct CardRead {
    /// The promoted products linked to the Product.
    promoted_as: Vec<PromotedProduct>,
    compared: Vec<ResellOrPromote>,
}

/// The card on a Product's sheet, with its own period.
pub struct ResellOrPromoteCard {
    span: Entity<SpanPicker>,
    product: Option<RecordId>,
    read: Option<Result<CardRead, String>>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    _subscriptions: Vec<Subscription>,
}

impl ResellOrPromoteCard {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let span = cx.new(|cx| SpanPicker::new(window, cx));
        // Sales, ad costs and Commissions all move what the card shows.
        let subscriptions = vec![
            cx.observe_global::<OrderSyncs>(Self::refresh),
            cx.observe_global::<AdsSyncs>(Self::refresh),
            cx.observe_global::<CommissionChanges>(Self::refresh),
            cx.subscribe(&span, |this, _, _: &SpanChanged, cx| this.refresh(cx)),
        ];
        Self {
            span,
            product: None,
            read: None,
            reads: 0,
            _subscriptions: subscriptions,
        }
    }

    /// Compares `product`, or nothing.
    pub fn show(&mut self, product: Option<RecordId>, cx: &mut Context<Self>) {
        if self.product != product {
            self.read = None;
        }
        self.product = product;
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (Some(product), Some(sales), Some(affiliate)) =
            (self.product, SaleSources::of(cx), affiliate::affiliate(cx))
        else {
            return;
        };
        let during = self.span.read(cx).during();
        self.reads += 1;
        let read_number = self.reads;
        let reading = cx
            .background_executor()
            .spawn(async move { read_card(product, sales, affiliate, during).await });
        cx.spawn(async move |this, cx| {
            let read = reading.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads == read_number {
                    this.read = Some(read);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

async fn read_card(
    product: RecordId,
    sales: SaleSources,
    affiliate: Arc<Affiliate>,
    during: Range<Timestamp>,
) -> Result<CardRead, String> {
    let sold = sales.read(during.clone()).await?;
    let earned = earned(&affiliate, during).await?;
    let promoted_as = affiliate
        .promoted_products()
        .await
        .map_err(|e| affiliate::failure(&e))?
        .into_iter()
        .filter(|promoted| promoted.linked_to == Some(product))
        .collect();
    let compared = ResellOrPromote::of(&[product], &sold.lines, &linked(&earned))
        .map_err(|e| format!("Valores em moedas diferentes: {e}"))?;
    Ok(CardRead {
        promoted_as,
        compared,
    })
}

fn side(title: &'static str, figure: String, details: Vec<String>, t: &Tokens) -> Div {
    v_flex()
        .flex_1()
        .min_w(px(220.))
        .gap_1()
        .p_3()
        .rounded(t.radius_lg)
        .border(t.border_width)
        .border_color(t.border)
        .bg(t.sunken)
        .child(div().text_sm().text_color(t.text2).child(title))
        .child(div().text_lg().font_semibold().child(figure))
        .children(
            details
                .into_iter()
                .map(|line| div().text_xs().text_color(t.text2).child(line)),
        )
}

fn units_text(units: u32) -> String {
    match units {
        1 => "1 unidade".into(),
        units => format!("{units} unidades"),
    }
}

fn commissions_text(count: usize) -> String {
    match count {
        1 => "1 comissão".into(),
        count => format!("{count} comissões"),
    }
}

/// One currency: the two sides and which left more.
fn comparison(compared: &ResellOrPromote, several: bool, cx: &App) -> AnyElement {
    let t = look(cx).tokens;
    let resold_details = match &compared.reselling {
        Some(reselling) => {
            let mut details = vec![format!(
                "{} · {} · receita {}",
                orders_text(reselling.orders),
                units_text(reselling.units),
                reselling.revenue.to_pt_br()
            )];
            if let Some(per_unit) = reselling.per_unit {
                details.push(format!("{} por unidade", per_unit.to_pt_br()));
            }
            if reselling.without_cost > 0 {
                details.push(format!(
                    "{} sem custo, fora da margem",
                    match reselling.without_cost {
                        1 => "1 venda".to_owned(),
                        count => format!("{count} vendas"),
                    }
                ));
            }
            details
        }
        None => vec!["Nenhuma venda no período.".into()],
    };
    let promoted_details = match &compared.promoting {
        Some(promoting) => vec![
            commissions_text(promoting.commissions),
            format!("{} por comissão", promoting.per_commission.to_pt_br()),
        ],
        None => vec!["Nenhuma comissão aprovada ou paga no período.".into()],
    };
    let (tag, ink) = verdict(compared, &t);
    v_flex()
        .gap_2()
        .when(several, |column| {
            column.child(
                div()
                    .text_sm()
                    .font_medium()
                    .child(format!("Em {}", compared.currency.code())),
            )
        })
        .child(
            h_flex()
                .flex_wrap()
                .gap_3()
                .child(side(
                    "Revendendo: margem realizada",
                    resold_text(compared),
                    resold_details,
                    &t,
                ))
                .child(side(
                    "Divulgando: comissões ganhas",
                    promoted_text(compared),
                    promoted_details,
                    &t,
                )),
        )
        .child(
            h_flex()
                .flex_wrap()
                .gap_2()
                .items_center()
                .child(kit::tag(tag, ink, cx))
                .child(div().text_sm().child(verdict_text(compared))),
        )
        .into_any_element()
}

impl Render for ResellOrPromoteCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let note = |text: String| div().text_sm().text_color(t.text2).child(text);
        let mut card = v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .child(kit::section_heading("Revender ou divulgar"))
                    .child(self.span.clone()),
            );
        match &self.read {
            None => {}
            Some(Err(error)) => card = card.child(kit::error_notice(error.clone(), cx)),
            Some(Ok(read)) => {
                card = card.child(note(if read.promoted_as.is_empty() {
                    "Nenhum produto divulgado ligado a este. Em Afiliação › Produtos \
                     divulgados, ligue o produto de um programa a este produto para comparar a \
                     margem de revender com as comissões de divulgar."
                        .into()
                } else {
                    let names: Vec<String> = read
                        .promoted_as
                        .iter()
                        .map(|promoted| format!("{} · {}", promoted.program.name, promoted.product))
                        .collect();
                    format!("Divulgado como: {}.", names.join("; "))
                }));
                if read.compared.is_empty() {
                    card = card.child(note(
                        "Nenhuma venda nem comissão ganha neste período.".into(),
                    ));
                }
                let several = read.compared.len() > 1;
                card = card.children(
                    read.compared
                        .iter()
                        .map(|compared| comparison(compared, several, cx)),
                );
                card = card.child(div().text_xs().text_color(t.text2).child(
                    "Revendendo: a margem realizada das vendas deste produto (receita menos \
                     tarifas, Product Ads, imposto e custo). Divulgando: as comissões aprovadas e \
                     pagas dos produtos ligados, na moeda da plataforma e sem conversão; \
                     estornos e vendas canceladas ficam fora.",
                ));
            }
        }
        card
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mascate_finance::{Promoting, Reselling};
    use mascate_kernel::{Currency, Margin};
    use rust_decimal::Decimal;

    fn brl(amount: i64) -> Money {
        Money::new(Decimal::from(amount), Currency::Brl)
    }

    fn compared(margin: Option<i64>, earned: i64) -> ResellOrPromote {
        ResellOrPromote {
            product: RecordId::from_u128(1),
            currency: Currency::Brl,
            reselling: Some(Reselling {
                orders: 1,
                units: 1,
                revenue: brl(100),
                margin: margin.map(|amount| Margin {
                    amount: brl(amount),
                    percent: None,
                }),
                without_cost: usize::from(margin.is_none()),
                per_unit: margin.map(brl),
            }),
            promoting: Some(Promoting {
                commissions: 1,
                earned: brl(earned),
                per_commission: brl(earned),
            }),
        }
    }

    #[test]
    fn the_verdict_names_the_side_ahead_and_by_how_much() {
        assert_eq!(
            verdict_text(&compared(Some(75), 30)),
            "No período, revender deixou R$ 45,00 a mais que divulgar."
        );
        assert_eq!(
            verdict_text(&compared(Some(10), 30)),
            "No período, divulgar rendeu R$ 20,00 a mais que revender."
        );
        assert!(verdict_text(&compared(Some(30), 30)).contains("o mesmo"));
        assert!(verdict_text(&compared(None, 30)).contains("não há margem para comparar"));
    }
}
