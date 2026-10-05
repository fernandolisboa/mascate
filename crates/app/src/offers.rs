//! Ofertas (#9): Supplier Offers typed by hand or kept from a search on
//! Shopee (#12), each link with its price history, and the step from an
//! offer to a Product. Manual registration is a Product Source of its own,
//! not a stand-in for the Shopee API.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::Select;
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Entity, EventEmitter, Subscription, Window, div, px};
use mascate_catalog::{Catalog, CatalogError, NewSupplierOffer, OfferHistory, Product, Supplier};
use mascate_kernel::{Currency, Money, RecordId, parse_amount};

use crate::appearance::look;
use crate::catalog::{self, NO_DATABASE, day, failure, price_and_shipping, product_choice};
use crate::forms::{Choice, Outcome, Picker, input, notice, picker, refill};
use crate::kit;
use crate::layout;
use crate::parts::ScreenParts;
use crate::shopee_search::{OffersKept, ShopeeSearch};

/// Asks the window to show a Product's sheet.
pub struct OpenProduct(pub RecordId);

/// The step an offer's row is in, below the offer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    CreateProduct,
    LinkProduct,
}

pub struct OffersScreen {
    shopee: Entity<ShopeeSearch>,
    link: Entity<InputState>,
    title: Entity<InputState>,
    price: Entity<InputState>,
    shipping: Entity<InputState>,
    currency: Currency,
    supplier: Picker,
    new_supplier: Entity<InputState>,
    histories: Vec<OfferHistory>,
    products: Vec<Product>,
    /// The offer whose row is open, and for what.
    step: Option<(RecordId, Step)>,
    product_name: Entity<InputState>,
    product_sku: Entity<InputState>,
    product_pick: Picker,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    /// The outcome of the registration form.
    form: Option<Outcome>,
    /// The outcome of the last step on an offer.
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<OpenProduct> for OffersScreen {}

impl OffersScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let link = input("https://shopee.com.br/...", window, cx);
        let title = input("Como o fornecedor chama o item", window, cx);
        let price = input("29,90", window, cx);
        let shipping = input("0,00", window, cx);
        let new_supplier = input("Nome da loja ou do atacadista", window, cx);
        let product_name = input("Nome do produto", window, cx);
        let product_sku = input("SKU", window, cx);
        let supplier = picker(window, cx);
        let product_pick = picker(window, cx);
        let shopee = cx.new(|cx| ShopeeSearch::new(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &new_supplier,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.add_supplier(window, cx);
                    }
                },
            ),
            cx.subscribe_in(&shopee, window, |this, _, _: &OffersKept, window, cx| {
                this.refresh_offers(window, cx);
            }),
        ];
        let mut screen = Self {
            shopee,
            link,
            title,
            price,
            shipping,
            currency: Currency::Brl,
            supplier,
            new_supplier,
            histories: Vec::new(),
            products: Vec::new(),
            step: None,
            product_name,
            product_sku,
            product_pick,
            busy: false,
            reads: 0,
            form: None,
            outcome: None,
            _subscriptions: subscriptions,
        };
        screen.refresh_offers(window, cx);
        screen
    }

    /// Reads the catalog and the Shopee Connection again, as when the
    /// screen comes into view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shopee.update(cx, |shopee, cx| shopee.refresh(cx));
        self.refresh_offers(window, cx);
    }

    fn refresh_offers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx, |_| async { Ok(None) }, |_, _: (), _, _| {});
    }

    /// Runs `change` off the UI thread, then reads the catalog again and
    /// hands what the change returned to `then`.
    fn run<T, F, Fut>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: F,
        then: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) where
        T: Send + 'static,
        F: FnOnce(Arc<Catalog>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<T>, CatalogError>> + Send,
    {
        let Some(catalog) = catalog::catalog(cx) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(catalog.clone()).await;
            let read = async {
                Ok::<_, CatalogError>((
                    catalog.suppliers().await?,
                    catalog.offers().await?,
                    catalog.products().await?,
                ))
            }
            .await;
            (changed, read)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (changed, read_back) = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reads == read {
                    this.busy = false;
                    match read_back {
                        Ok((suppliers, histories, products)) => {
                            this.show(suppliers, histories, products, window, cx)
                        }
                        Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                    }
                }
                match changed {
                    Ok(Some(value)) => then(this, value, window, cx),
                    Ok(None) => {}
                    Err(error) => this.failed(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Shows a failure where the owner acted: the form, or the open row.
    fn failed(&mut self, error: CatalogError) {
        let text = Outcome::Failed(failure(&error).into());
        if self.step.is_some() {
            self.outcome = Some(text);
        } else {
            self.form = Some(text);
        }
    }

    fn show(
        &mut self,
        suppliers: Vec<Supplier>,
        histories: Vec<OfferHistory>,
        products: Vec<Product>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let choices = suppliers
            .iter()
            .map(|supplier| Choice {
                id: supplier.id,
                title: supplier.name.clone().into(),
            })
            .collect::<Vec<_>>();
        refill(&self.supplier, choices, window, cx);
        refill(
            &self.product_pick,
            products.iter().map(product_choice).collect(),
            window,
            cx,
        );
        if self
            .step
            .is_some_and(|(offer, _)| !histories.iter().any(|h| h.latest.id == offer))
        {
            self.step = None;
        }
        self.histories = histories;
        self.products = products;
    }

    fn add_supplier(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_supplier.read(cx).value().to_string();
        self.form = None;
        self.step = None;
        self.run(
            window,
            cx,
            move |catalog| async move { catalog.add_supplier(&name).await.map(Some) },
            |this, supplier: Supplier, window, cx| {
                this.new_supplier
                    .update(cx, |input, cx| input.set_value("", window, cx));
                this.supplier.update(cx, |select, cx| {
                    select.set_selected_value(&supplier.id, window, cx)
                });
                this.form = Some(Outcome::Done(
                    format!("Fornecedor {} cadastrado.", supplier.name).into(),
                ));
            },
        );
    }

    fn register(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step = None;
        let Some(&supplier) = self.supplier.read(cx).selected_value() else {
            self.form = Some(Outcome::Failed(
                "Escolha o fornecedor, ou cadastre um novo ao lado.".into(),
            ));
            cx.notify();
            return;
        };
        let amount = |input: &Entity<InputState>, empty: Option<&str>| {
            let text = input.read(cx).value();
            let text = if text.trim().is_empty() {
                empty.unwrap_or_default().to_owned()
            } else {
                text.to_string()
            };
            parse_amount(&text).map(|amount| Money::new(amount, self.currency))
        };
        let (Some(price), Some(shipping)) =
            (amount(&self.price, None), amount(&self.shipping, Some("0")))
        else {
            self.form = Some(Outcome::Failed(
                "Digite preço e frete como 29,90 (frete vazio conta como zero).".into(),
            ));
            cx.notify();
            return;
        };
        let offer = NewSupplierOffer {
            supplier,
            link: self.link.read(cx).value().to_string(),
            title: self.title.read(cx).value().to_string(),
            price,
            shipping,
        };
        self.form = None;
        self.run(
            window,
            cx,
            move |catalog| async move {
                let offer = catalog.register_offer(offer).await?;
                let product = match offer.product {
                    Some(product) => Some(catalog.product(product).await?),
                    None => None,
                };
                Ok(Some(product))
            },
            |this, product: Option<Product>, window, cx| {
                for input in [&this.link, &this.title, &this.price, &this.shipping] {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
                let text = match product {
                    Some(product) => format!(
                        "Oferta registrada no histórico de preço de {} · {}.",
                        product.sku, product.name
                    ),
                    None => "Oferta registrada.".to_owned(),
                };
                this.form = Some(Outcome::Done(text.into()));
            },
        );
    }

    fn open_step(
        &mut self,
        offer: RecordId,
        step: Step,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step = Some((offer, step));
        self.outcome = None;
        self.form = None;
        let Some(history) = self.histories.iter().find(|h| h.latest.id == offer) else {
            return;
        };
        if step == Step::CreateProduct {
            let title = history.latest.title.clone();
            self.product_name
                .update(cx, |input, cx| input.set_value(title.clone(), window, cx));
            self.product_sku
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.run(
                window,
                cx,
                move |catalog| async move { catalog.suggest_sku(&title).await.map(Some) },
                |this, sku: mascate_catalog::Sku, window, cx| {
                    this.product_sku
                        .update(cx, |input, cx| input.set_value(sku.to_string(), window, cx));
                },
            );
        }
        cx.notify();
    }

    fn create_product(&mut self, offer: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.product_name.read(cx).value().to_string();
        let sku = self.product_sku.read(cx).value().to_string();
        self.outcome = None;
        self.run(
            window,
            cx,
            move |catalog| async move {
                catalog.create_product(offer, &name, &sku).await.map(Some)
            },
            |this, product: Product, _, _| {
                this.step = None;
                this.outcome = Some(Outcome::Done(
                    format!(
                        "Produto {} criado, com a pasta para os arquivos dele.",
                        product.sku
                    )
                    .into(),
                ));
            },
        );
    }

    fn link_product(&mut self, offer: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(&product) = self.product_pick.read(cx).selected_value() else {
            self.outcome = Some(Outcome::Failed("Escolha o produto.".into()));
            cx.notify();
            return;
        };
        self.outcome = None;
        self.run(
            window,
            cx,
            move |catalog| async move {
                catalog.link_offer(offer, product).await?;
                catalog.product(product).await.map(Some)
            },
            |this, product: Product, _, _| {
                this.step = None;
                this.outcome = Some(Outcome::Done(
                    format!("Oferta ligada a {} · {}.", product.sku, product.name).into(),
                ));
            },
        );
    }

    fn render_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, input: AnyElement| {
            v_flex()
                .gap_1()
                .child(div().text_sm().font_medium().child(label))
                .child(input)
        };
        let currency = |value: Currency| {
            let on = self.currency == value;
            Button::new(("currency", value as usize))
                .label(value.symbol())
                .small()
                .map(|button| {
                    if on {
                        button.primary()
                    } else {
                        button.outline()
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.currency = value;
                    cx.notify();
                }))
        };
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(kit::section_heading("Nova oferta"))
            .child(field(
                "Link",
                Input::new(&self.link).small().into_any_element(),
            ))
            .child(field(
                "Título",
                Input::new(&self.title).small().into_any_element(),
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_end()
                    .child(
                        div().w(px(260.)).child(field(
                            "Fornecedor",
                            Select::new(&self.supplier)
                                .search_placeholder("Buscar")
                                .small()
                                .placeholder("Escolha o fornecedor")
                                .empty(|_, _| {
                                    div()
                                        .p_2()
                                        .text_sm()
                                        .child("Cadastre um fornecedor ao lado.")
                                })
                                .into_any_element(),
                        )),
                    )
                    .child(div().w(px(130.)).child(field(
                        "Preço",
                        Input::new(&self.price).small().into_any_element(),
                    )))
                    .child(div().w(px(130.)).child(field(
                        "Frete",
                        Input::new(&self.shipping).small().into_any_element(),
                    )))
                    .child(field(
                        "Moeda",
                        h_flex()
                            .gap_1()
                            .child(currency(Currency::Brl))
                            .child(currency(Currency::Usd))
                            .into_any_element(),
                    )),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("register-offer")
                            .label("Registrar oferta")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.register(window, cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text2)
                            .child("Fornecedor novo:"),
                    )
                    .child(
                        div()
                            .w(px(220.))
                            .child(Input::new(&self.new_supplier).small()),
                    )
                    .child(
                        Button::new("add-supplier")
                            .label("Cadastrar")
                            .outline()
                            .small()
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_supplier(window, cx)),
                            ),
                    ),
            )
            .children(self.form.as_ref().map(|outcome| notice(outcome, cx)))
            .into_any_element()
    }

    fn render_history(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let history = &self.histories[index];
        let offer = &history.latest;
        let id = offer.id;
        let link = offer.link.clone();
        let product = offer
            .product
            .and_then(|product| self.products.iter().find(|p| p.id == product));
        let step = self
            .step
            .filter(|(open, _)| *open == id)
            .map(|(_, step)| step);

        let earlier = history.earlier.first().map(|before| {
            let count = history.earlier.len() + 1;
            format!(
                "Antes: {} em {} · {count} preços registrados",
                before.total().to_pt_br(),
                day(before.observed_at)
            )
        });

        let actions = h_flex().gap_2().flex_wrap().map(|row| match product {
            Some(product) => {
                let product_id = product.id;
                row.items_center()
                    .child(kit::tag(product.sku.to_string(), t.accent_text, cx))
                    .child(div().text_sm().child(product.name.clone()))
                    .child(
                        Button::new(("open-product", index))
                            .label("Abrir ficha")
                            .icon(IconName::ChevronRight)
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(OpenProduct(product_id));
                            })),
                    )
            }
            None if step.is_none() => row
                .child(
                    Button::new(("create-product", index))
                        .label("Criar produto")
                        .outline()
                        .small()
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_step(id, Step::CreateProduct, window, cx)
                        })),
                )
                .when(!self.products.is_empty(), |row| {
                    row.child(
                        Button::new(("link-product", index))
                            .label("Ligar a um produto")
                            .ghost()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_step(id, Step::LinkProduct, window, cx)
                            })),
                    )
                }),
            None => row,
        });

        let cancel = Button::new(("cancel-step", index))
            .label("Cancelar")
            .ghost()
            .small()
            .on_click(cx.listener(|this, _, _, cx| {
                this.step = None;
                this.outcome = None;
                cx.notify();
            }));
        let step_form = step.map(|step| {
            let row = h_flex().flex_wrap().gap_2().items_end();
            match step {
                Step::CreateProduct => row
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(300.))
                            .child(div().text_sm().font_medium().child("Nome do produto"))
                            .child(Input::new(&self.product_name).small()),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(200.))
                            .child(div().text_sm().font_medium().child("SKU"))
                            .child(Input::new(&self.product_sku).small()),
                    )
                    .child(
                        Button::new(("confirm-create", index))
                            .label("Criar")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.create_product(id, window, cx)
                            })),
                    )
                    .child(cancel),
                Step::LinkProduct => row
                    .child(
                        div().w(px(320.)).child(
                            Select::new(&self.product_pick)
                                .search_placeholder("Buscar")
                                .small()
                                .placeholder("Produto"),
                        ),
                    )
                    .child(
                        Button::new(("confirm-link", index))
                            .label("Ligar")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.link_product(id, window, cx)
                            })),
                    )
                    .child(cancel),
            }
        });

        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if step.is_some() {
                t.accent_edge
            } else {
                t.frame
            })
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
                            .child(div().font_medium().child(offer.title.clone()))
                            .child(div().text_xs().text_color(t.text2).child(format!(
                                "{} · {}",
                                offer.supplier.name,
                                day(offer.observed_at)
                            )))
                            .child(
                                div()
                                    .id(("offer-link", index))
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
                            .gap_0p5()
                            .child(div().font_semibold().child(offer.total().to_pt_br()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.text2)
                                    .child(price_and_shipping(offer)),
                            ),
                    ),
            )
            .children(earlier.map(|text| div().text_xs().text_color(t.text2).child(text)))
            .child(actions)
            .children(step_form)
            .when(step.is_some(), |card| {
                card.children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            })
            .into_any_element()
    }
}

impl Render for OffersScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let mut parts = ScreenParts::new("Ofertas");
        if catalog::catalog(cx).is_none() {
            parts
                .notices
                .push(kit::error_notice(NO_DATABASE, cx).into_any_element());
            return layout::screen(parts, cx);
        }
        // A step's outcome shows on its row; a finished one, on top.
        if self.step.is_none() {
            parts
                .notices
                .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        }
        parts.content.push(self.shopee.clone().into_any_element());
        parts.content.push(self.render_form(cx));
        parts.content.push(
            v_flex()
                .gap_3()
                .child(kit::section_heading("Ofertas registradas"))
                .when(self.histories.is_empty(), |list| {
                    list.child(div().text_sm().text_color(t.text2).child(
                        "Nenhuma oferta ainda. Guarde uma da busca na Shopee ou cole o link de \
                         um produto que você achou, com preço e frete; registrar o mesmo link de \
                         novo guarda o histórico de preço.",
                    ))
                })
                .children((0..self.histories.len()).map(|index| self.render_history(index, cx)))
                .into_any_element(),
        );
        layout::screen(parts, cx)
    }
}
