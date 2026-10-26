//! What every search for offers on Ofertas shares, the Shopee API (#12) and
//! the crawlers (#152) alike: the card of a found item, keeping items as
//! Supplier Offers and how counts and orders read in Portuguese.

use std::sync::Arc;

use gpui_kit::component::button::Button;
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, div};
use mascate_catalog::{
    Catalog, CatalogError, FoundOffer, KeptOffers, OfferOrder, ProductSource, SearchedOffer,
};
use rust_decimal::Decimal;

use crate::appearance::look;
use crate::kit;

/// The card of `item`, the `index`th found: title, facts and link on the
/// left, price and `actions` on the right, `below` under both. `name`
/// keeps the link's element id apart from other searches'. A source whose
/// zero sales may only mean the page did not say passes `zero_sales_known`
/// false, and the card leaves them out.
#[allow(clippy::too_many_arguments)]
pub fn item_card(
    name: &'static str,
    index: usize,
    item: &SearchedOffer,
    zero_sales_known: bool,
    actions: Vec<AnyElement>,
    below: Option<AnyElement>,
    cx: &App,
) -> AnyElement {
    let t = look(cx).tokens;
    let found = &item.found;
    let link = found.link.clone();
    let mut facts = vec![found.shop.name.clone()];
    if found.sales > 0 || zero_sales_known {
        facts.push(sold_text(found.sales));
    }
    if let Some(commission) = found.commission {
        facts.push(format!("comissão {}", commission.to_pt_br()));
    }
    if let Some(rating) = found.rating.filter(|rating| !rating.is_zero()) {
        facts.push(format!("nota {}", stars_text(rating)));
    }
    let price = match (found.price, found.highest_price) {
        (Some(price), Some(highest)) if highest.amount() > price.amount() => {
            format!("{} a {}", price.to_pt_br(), highest.to_pt_br())
        }
        (Some(price), _) => price.to_pt_br(),
        (None, _) => "Sem preço na busca".to_owned(),
    };
    v_flex()
        .gap_2()
        .p_3()
        .rounded(t.radius_lg)
        .border(t.border_width)
        .border_color(t.frame)
        .bg(t.surface)
        .child(
            h_flex()
                .gap_3()
                .items_start()
                .justify_between()
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .gap_0p5()
                        .child(div().font_medium().child(found.title.clone()))
                        .child(div().text_xs().text_color(t.text2).child(facts.join(" · ")))
                        .child(
                            div()
                                .id((name, index))
                                .text_xs()
                                .text_color(t.accent_text)
                                .truncate()
                                .cursor_pointer()
                                .child(link.clone())
                                .on_click(move |_, _, cx| cx.open_url(&link)),
                        ),
                )
                .child(
                    v_flex()
                        .flex_none()
                        .items_end()
                        .gap_1()
                        .child(div().font_semibold().child(price))
                        .children(actions),
                ),
        )
        .children(below)
        .into_any_element()
}

/// "Nas ofertas" when `item` is kept as it is, else the button that keeps
/// it, calling `keep` on the search that found it.
pub fn keep_action<T: 'static>(
    index: usize,
    item: &SearchedOffer,
    busy: bool,
    keep: fn(&mut T, Vec<FoundOffer>, &mut Context<T>),
    cx: &mut Context<T>,
) -> AnyElement {
    if item.up_to_date() {
        return kit::tag("Nas ofertas", look(cx).tokens.success, cx).into_any_element();
    }
    if item.found.price.is_none() {
        return div()
            .text_xs()
            .text_color(look(cx).tokens.text2)
            .child("Sem preço, não dá para guardar")
            .into_any_element();
    }
    let offer = item.found.clone();
    let label = match &item.kept {
        Some(kept) => format!("Guardar preço novo (era {})", kept.price.to_pt_br()),
        None => "Guardar nas ofertas".to_owned(),
    };
    Button::new(("keep-offer", index))
        .label(label)
        .outline()
        .small()
        .disabled(busy)
        .on_click(cx.listener(move |this, _, _, cx| keep(this, vec![offer.clone()], cx)))
        .into_any_element()
}

/// Keeps `offers`, found in `source`, and marks `listed` again with what is
/// kept now.
pub async fn keep(
    catalog: &Arc<Catalog>,
    source: ProductSource,
    offers: &[FoundOffer],
    listed: Vec<FoundOffer>,
) -> Result<(KeptOffers, Vec<SearchedOffer>), CatalogError> {
    let report = catalog.keep_found_offers(source, offers).await?;
    let items = catalog.with_kept(source, listed).await?;
    Ok((report, items))
}

pub fn kept_text(report: &KeptOffers) -> String {
    let mut parts = Vec::new();
    match report.added {
        0 => {}
        1 => parts.push("1 oferta guardada".to_owned()),
        added => parts.push(format!("{added} ofertas guardadas")),
    }
    match report.unchanged {
        0 => {}
        1 => parts.push("1 já estava nas ofertas".to_owned()),
        unchanged => parts.push(format!("{unchanged} já estavam nas ofertas")),
    }
    match report.skipped {
        0 => {}
        1 => parts.push("1 sem título, link ou preço ficou de fora".to_owned()),
        skipped => parts.push(format!(
            "{skipped} sem título, link ou preço ficaram de fora"
        )),
    }
    let mut text = if parts.is_empty() {
        "Nada para guardar.".to_owned()
    } else {
        format!("{}.", parts.join("; "))
    };
    if report.added > 0 {
        text.push_str(" As novas entram nas Oportunidades no próximo Sync.");
    }
    text
}

pub fn order_name(order: OfferOrder) -> &'static str {
    match order {
        OfferOrder::Sales => "Mais vendidos",
        OfferOrder::Relevance => "Relevância",
        OfferOrder::LowestPrice => "Menor preço",
        OfferOrder::HighestCommission => "Maior comissão",
    }
}

/// "12.400 vendidos".
fn sold_text(sales: u32) -> String {
    let digits = sales.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push('.');
        }
        grouped.push(digit);
    }
    match sales {
        1 => "1 vendido".into(),
        _ => format!("{grouped} vendidos"),
    }
}

/// "4,9".
pub fn stars_text(stars: Decimal) -> String {
    stars.round_dp(1).normalize().to_string().replace('.', ",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sales_read_with_thousands_grouped() {
        assert_eq!(sold_text(0), "0 vendidos");
        assert_eq!(sold_text(1), "1 vendido");
        assert_eq!(sold_text(830), "830 vendidos");
        assert_eq!(sold_text(12_400), "12.400 vendidos");
        assert_eq!(sold_text(1_234_567), "1.234.567 vendidos");
    }

    #[test]
    fn kept_offers_read_as_a_sentence() {
        let report = KeptOffers {
            added: 2,
            unchanged: 1,
            skipped: 0,
        };
        assert_eq!(
            kept_text(&report),
            "2 ofertas guardadas; 1 já estava nas ofertas. As novas entram nas Oportunidades \
             no próximo Sync."
        );
        assert_eq!(kept_text(&KeptOffers::default()), "Nada para guardar.");
    }
}
