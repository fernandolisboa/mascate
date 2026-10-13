//! Produtos divulgados on the Afiliação screen (#86): each product the
//! owner promotes, per Program, linked to at most one Product of the
//! Catalog, so its sheet and the Painel compare reselling it with promoting
//! it. Affiliate keeps the link by the Product's id (ADR 0030).

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::Select;
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, EventEmitter, Subscription, Window, div, px};
use mascate_affiliate::{Affiliate, AffiliateError, PromotedProduct};
use mascate_catalog::{Catalog, Product};
use mascate_kernel::{RecordId, folded_words};

use crate::affiliate::{affiliate, failure};
use crate::appearance::look;
use crate::catalog::{self, product_choice};
use crate::forms::{Outcome, Picker, input, notice, picker, refill};
use crate::kit;
use crate::sales::table_row;

/// The rows shown before the owner searches.
const SHOWN: usize = 20;

/// A product was linked or unlinked: the comparisons changed.
pub struct ProductLinksChanged;

/// A promoted product by its Program and name.
type Promoted = (RecordId, String);

pub struct ProductLinksSection {
    promoted: Vec<PromotedProduct>,
    products: Vec<Product>,
    search: Entity<InputState>,
    /// The product whose Product is being picked.
    picking: Option<Promoted>,
    picker: Picker,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ProductLinksChanged> for ProductLinksSection {}

impl ProductLinksSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = input("Buscar produto divulgado", window, cx);
        let subscriptions = vec![cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })];
        Self {
            promoted: Vec::new(),
            products: Vec::new(),
            search,
            picking: None,
            picker: picker(window, cx),
            busy: false,
            reads: 0,
            outcome: None,
            _subscriptions: subscriptions,
        }
    }

    /// Reads the promoted products and the Catalog again.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx, |_| async { Ok(false) });
    }

    /// Runs `change` off the UI thread, then reads the promoted products
    /// and the Catalog again; `change` answers whether it changed a link.
    fn run<F, Fut>(&mut self, window: &mut Window, cx: &mut Context<Self>, change: F)
    where
        F: FnOnce(Arc<Affiliate>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<bool, AffiliateError>> + Send,
    {
        let (Some(affiliate), Some(catalog)) = (affiliate(cx), catalog::catalog(cx)) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(affiliate.clone()).await;
            (changed, read_both(&affiliate, &catalog).await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (changed, read_back) = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reads == read {
                    this.busy = false;
                    match read_back {
                        Ok((promoted, products)) => {
                            let choices = products.iter().map(product_choice).collect();
                            refill(&this.picker, choices, window, cx);
                            this.promoted = promoted;
                            this.products = products;
                        }
                        Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                    }
                }
                match changed {
                    Ok(true) => cx.emit(ProductLinksChanged),
                    Ok(false) => {}
                    Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn pick(&mut self, promoted: Promoted, window: &mut Window, cx: &mut Context<Self>) {
        let linked = self
            .promoted
            .iter()
            .find(|p| p.program.id == promoted.0 && p.product == promoted.1)
            .and_then(|p| p.linked_to);
        self.picker.update(cx, |select, cx| match linked {
            Some(product) => select.set_selected_value(&product, window, cx),
            None => select.set_selected_index(None, window, cx),
        });
        self.picking = Some(promoted);
        self.outcome = None;
        cx.notify();
    }

    fn link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((program, product)) = self.picking.clone() else {
            return;
        };
        let Some(to) = self.picker.read(cx).selected_value().copied() else {
            self.outcome = Some(Outcome::Failed("Escolha o produto do catálogo.".into()));
            cx.notify();
            return;
        };
        let name = self.product_name(to);
        self.picking = None;
        self.outcome = Some(Outcome::Done(
            format!(
                "{product} ligado a {name}: a ficha do produto e o Painel comparam revender com \
                 divulgar."
            )
            .into(),
        ));
        self.run(window, cx, move |affiliate| async move {
            affiliate.link_product(program, &product, to).await?;
            Ok(true)
        });
    }

    fn unlink(&mut self, promoted: Promoted, window: &mut Window, cx: &mut Context<Self>) {
        let (program, product) = promoted;
        self.picking = None;
        self.outcome = Some(Outcome::Done(
            format!("{product} não está mais ligado a um produto do catálogo.").into(),
        ));
        self.run(window, cx, move |affiliate| async move {
            affiliate.unlink_product(program, &product).await?;
            Ok(true)
        });
    }

    fn product_name(&self, id: RecordId) -> String {
        self.products
            .iter()
            .find(|product| product.id == id)
            .map_or_else(
                || "um produto que saiu do catálogo".into(),
                |product| product.name.clone(),
            )
    }

    /// The promoted products the search finds, the linked ones first, then
    /// those with the most Commissions.
    fn found(&self, cx: &App) -> Vec<&PromotedProduct> {
        let words = folded_words(&self.search.read(cx).value());
        let mut found: Vec<&PromotedProduct> = self
            .promoted
            .iter()
            .filter(|promoted| {
                let named = folded_words(&format!(
                    "{} {} {}",
                    promoted.program.platform.name(),
                    promoted.program.name,
                    promoted.product
                ));
                words
                    .iter()
                    .all(|word| named.iter().any(|name| name.starts_with(word.as_str())))
            })
            .collect();
        found.sort_by_key(|promoted| {
            (
                promoted.linked_to.is_none(),
                std::cmp::Reverse(promoted.commissions),
            )
        });
        found
    }

    fn render_row(
        &self,
        index: usize,
        promoted: &PromotedProduct,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let key: Promoted = (promoted.program.id, promoted.product.clone());
        let picking = self.picking.as_ref() == Some(&key);
        let linked = promoted.linked_to.map(|id| self.product_name(id));
        let pick_key = key.clone();
        let mut row = table_row(&t)
            .flex_wrap()
            .child(kit::tag(promoted.program.platform.name(), t.text2, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(220.))
                    .child(
                        div()
                            .font_medium()
                            .truncate()
                            .child(promoted.product.clone()),
                    )
                    .child(div().text_xs().text_color(t.text2).child(format!(
                        "{} · {}",
                        promoted.program.name,
                        match promoted.commissions {
                            1 => "1 comissão".to_owned(),
                            count => format!("{count} comissões"),
                        }
                    ))),
            )
            .child(match &linked {
                Some(name) => kit::tag(format!("ligado a {name}"), t.success, cx),
                None => div()
                    .text_xs()
                    .text_color(t.text2)
                    .child("sem produto do catálogo"),
            })
            .child(
                Button::new(("pick-linked", index))
                    .label(if linked.is_some() {
                        "Trocar"
                    } else {
                        "Ligar a um produto"
                    })
                    .outline()
                    .small()
                    .disabled(self.busy || self.products.is_empty())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick(pick_key.clone(), window, cx)
                    })),
            );
        if linked.is_some() {
            let unlink_key = key.clone();
            row = row.child(
                Button::new(("unlink", index))
                    .label("Desligar")
                    .ghost()
                    .small()
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.unlink(unlink_key.clone(), window, cx)
                    })),
            );
        }
        let form = picking.then(|| {
            h_flex()
                .flex_wrap()
                .gap_2()
                .items_end()
                .pl_3()
                .child(
                    div().w(px(320.)).child(
                        Select::new(&self.picker)
                            .search_placeholder("Buscar")
                            .small()
                            .placeholder("Produto do catálogo"),
                    ),
                )
                .child(
                    Button::new("confirm-product-link")
                        .label("Ligar")
                        .primary()
                        .small()
                        .loading(self.busy)
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.link(window, cx))),
                )
                .child(
                    Button::new("cancel-product-link")
                        .label("Cancelar")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.picking = None;
                            cx.notify();
                        })),
                )
        });
        v_flex()
            .gap_2()
            .child(row)
            .children(form)
            .into_any_element()
    }
}

async fn read_both(
    affiliate: &Affiliate,
    catalog: &Catalog,
) -> Result<(Vec<PromotedProduct>, Vec<Product>), String> {
    let promoted = affiliate
        .promoted_products()
        .await
        .map_err(|e| failure(&e))?;
    let products = catalog.products().await.map_err(|e| catalog::failure(&e))?;
    Ok((promoted, products))
}

impl Render for ProductLinksSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let mut section = v_flex()
            .gap_3()
            .child(kit::section_heading("Produtos divulgados"))
            .child(div().text_sm().text_color(t.text2).child(
                "Ligue o produto que você divulga a um produto do catálogo para ver, na ficha \
                 do produto e no Painel, a margem de revendê-lo ao lado das comissões de \
                 divulgá-lo. Cada produto divulgado se liga a um produto do catálogo, no máximo.",
            ))
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        if self.promoted.is_empty() {
            return section.child(div().text_sm().text_color(t.text2).child(
                "Nenhum produto divulgado ainda: eles aparecem aqui com a primeira comissão.",
            ));
        }
        if self.products.is_empty() {
            section = section.child(
                div()
                    .text_sm()
                    .text_color(t.text2)
                    .child("O catálogo não tem produtos. Crie um em Ofertas para ligar a ele."),
            );
        }
        let found = self.found(cx);
        let total = found.len();
        let rows: Vec<AnyElement> = found
            .into_iter()
            .take(SHOWN)
            .cloned()
            .collect::<Vec<_>>()
            .iter()
            .enumerate()
            .map(|(index, promoted)| self.render_row(index, promoted, cx))
            .collect();
        section
            .child(div().w(px(320.)).child(Input::new(&self.search).small()))
            .when(total == 0, |section| {
                section.child(
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child("Nenhum produto divulgado com esse nome."),
                )
            })
            .children(rows)
            .when(total > SHOWN, |section| {
                section.child(div().text_xs().text_color(t.text2).child(format!(
                    "Mostrando {SHOWN} de {total}; busque pelo nome para achar os outros."
                )))
            })
    }
}
