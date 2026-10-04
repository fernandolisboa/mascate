//! Produtos (#9): every Product by SKU, and each one's sheet with its SKU,
//! its folder of files (drop files on the sheet to copy them there), its
//! target margin (#19) and the price history of its Supplier Offers.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Entity, EventEmitter, ExternalPaths, SharedString, Subscription, Window, div, px,
};
use mascate_catalog::{
    AddedFiles, Catalog, CatalogError, NotCopied, OfferHistory, Product, ProductFile,
};
use mascate_commerce::TargetMargin;
use mascate_kernel::RecordId;

use crate::appearance::look;
use crate::catalog::{self, NO_DATABASE, day, failure};
use crate::drafts::StartDraft;
use crate::forms::{Outcome, notice, percent_text};
use crate::kit;
use crate::layout;
use crate::parts::ScreenParts;
use crate::pricing;

/// A Product's sheet as last read.
struct Sheet {
    product: Product,
    folder: PathBuf,
    files: Vec<ProductFile>,
    offers: Vec<OfferHistory>,
    /// Its target margin, or why it could not be read.
    target: Result<TargetMargin, String>,
}

pub struct ProductsScreen {
    products: Vec<Product>,
    /// The Product whose sheet is open.
    open: Option<RecordId>,
    sheet: Option<Sheet>,
    sku: Entity<InputState>,
    target: Entity<InputState>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<StartDraft> for ProductsScreen {}

impl ProductsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sku = cx.new(|cx| InputState::new(window, cx).placeholder("SKU"));
        let target = cx.new(|cx| InputState::new(window, cx).placeholder("padrão"));
        let subscriptions =
            vec![
                cx.subscribe_in(&sku, window, |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.rename_sku(window, cx);
                    }
                }),
            ];
        let mut screen = Self {
            products: Vec::new(),
            open: None,
            sheet: None,
            sku,
            target,
            busy: false,
            reads: 0,
            outcome: None,
            _subscriptions: subscriptions,
        };
        screen.refresh(window, cx);
        screen
    }

    /// Opens a Product's sheet.
    pub fn open(&mut self, product: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        if self.open != Some(product) {
            self.sheet = None;
            self.outcome = None;
        }
        self.open = Some(product);
        self.refresh(window, cx);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = None;
        self.sheet = None;
        self.outcome = None;
        self.refresh(window, cx);
    }

    /// Reads the catalog again, as when the screen comes into view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx, |_, _| async { Ok(None) }, |_, _: (), _, _| {});
    }

    /// Runs `change` on the open Product off the UI thread, then reads the
    /// list and the sheet again and hands what the change returned to `then`.
    fn run<T, F, Fut>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: F,
        then: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) where
        T: Send + 'static,
        F: FnOnce(Arc<Catalog>, Option<RecordId>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<T>, CatalogError>> + Send,
    {
        let (Some(catalog), Some(pricing)) = (catalog::catalog(cx), pricing::pricing(cx)) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let open = self.open;
        let working = cx.background_executor().spawn(async move {
            let changed = change(catalog.clone(), open).await;
            let read = async {
                let products = catalog.products().await?;
                let sheet = match open {
                    Some(id) => {
                        let product = catalog.product(id).await?;
                        Some(Sheet {
                            folder: catalog.folder(&product),
                            files: catalog.files(id).await?,
                            offers: catalog.product_offers(id).await?,
                            target: pricing
                                .target_margin(id)
                                .await
                                .map_err(|error| pricing::target_failure(&error)),
                            product,
                        })
                    }
                    None => None,
                };
                Ok::<_, CatalogError>((products, sheet))
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
                        Ok((products, sheet)) => {
                            this.products = products;
                            // A refused SKU stays in the field to be fixed.
                            let shown = this.sheet.as_ref().map(|sheet| &sheet.product);
                            if let Some(sheet) = &sheet
                                && shown != Some(&sheet.product)
                            {
                                let sku = sheet.product.sku.to_string();
                                this.sku
                                    .update(cx, |input, cx| input.set_value(sku, window, cx));
                            }
                            // The field shows the Product's own target, empty
                            // for the default.
                            let shown_target = this
                                .sheet
                                .as_ref()
                                .map(|sheet| (sheet.product.id, &sheet.target));
                            if let Some(sheet) = &sheet
                                && shown_target != Some((sheet.product.id, &sheet.target))
                            {
                                let own = match &sheet.target {
                                    Ok(target) if target.own => percent_text(target.margin),
                                    _ => String::new(),
                                };
                                this.target
                                    .update(cx, |input, cx| input.set_value(own, window, cx));
                            }
                            this.sheet = sheet;
                        }
                        Err(error) => {
                            this.outcome = Some(Outcome::Failed(failure(&error).into()));
                        }
                    }
                }
                match changed {
                    Ok(Some(value)) => then(this, value, window, cx),
                    Ok(None) => {}
                    Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn rename_sku(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sku = self.sku.read(cx).value().to_string();
        self.outcome = None;
        self.run(
            window,
            cx,
            move |catalog, open| async move {
                let Some(product) = open else {
                    return Ok(None);
                };
                catalog.rename_sku(product, &sku).await.map(Some)
            },
            |this, product: Product, _, _| {
                this.outcome = Some(Outcome::Done(
                    format!(
                        "SKU agora é {}; a pasta foi renomeada junto, com os arquivos.",
                        product.sku
                    )
                    .into(),
                ));
            },
        );
    }

    /// Sets the open Product's own target margin from the field or, with
    /// `default`, goes back to the default.
    fn save_target(&mut self, default: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(product), Some(pricing)) = (self.open, pricing::pricing(cx)) else {
            return;
        };
        let margin = if default {
            None
        } else {
            match pricing::parse_target(&self.target.read(cx).value()) {
                Some(margin) => Some(margin),
                None => {
                    self.outcome = Some(Outcome::Failed(
                        "Digite a margem alvo de 0 até menos de 100% (ex.: 25 ou 22,5).".into(),
                    ));
                    cx.notify();
                    return;
                }
            }
        };
        self.outcome = None;
        let saving = cx.background_executor().spawn(async move {
            pricing
                .set_target_margin(product, margin)
                .await
                .map_err(|error| pricing::target_failure(&error))
        });
        cx.spawn_in(window, async move |this, cx| {
            let saved = saving.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.outcome = Some(match saved {
                    Ok(()) => Outcome::Done(
                        match margin {
                            Some(margin) => format!(
                                "Margem alvo de {} salva; a sugestão de preço dos anúncios \
                                 deste produto parte dela.",
                                margin.to_pt_br()
                            ),
                            None => "O produto voltou para a margem alvo padrão.".into(),
                        }
                        .into(),
                    ),
                    Err(error) => Outcome::Failed(error.into()),
                });
                this.refresh(window, cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn add_files(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.outcome = None;
        self.run(
            window,
            cx,
            move |catalog, open| async move {
                let Some(product) = open else {
                    return Ok(None);
                };
                catalog.add_files(product, &paths).await.map(Some)
            },
            |this, added: AddedFiles, _, _| this.outcome = Some(added_outcome(&added)),
        );
    }

    fn render_list(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = look(cx).tokens;
        if self.products.is_empty() {
            return vec![
                div()
                    .text_sm()
                    .text_color(t.text2)
                    .child(
                        "Nenhum produto ainda. Em Ofertas, registre uma oferta e use \"Criar \
                         produto\".",
                    )
                    .into_any_element(),
            ];
        }
        self.products
            .iter()
            .enumerate()
            .map(|(index, product)| {
                let id = product.id;
                h_flex()
                    .id(("product", index))
                    .gap_3()
                    .p_3()
                    .rounded(t.radius_lg)
                    .border(t.border_width)
                    .border_color(t.frame)
                    .bg(t.surface)
                    .cursor_pointer()
                    .hover(|row| row.border_color(t.accent_edge))
                    .child(kit::tag(product.sku.to_string(), t.accent_text, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(product.name.clone()),
                    )
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .size(px(16.))
                            .text_color(t.text2),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| this.open(id, window, cx)))
                    .into_any_element()
            })
            .collect()
    }

    fn render_sheet(&self, sheet: &Sheet, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = look(cx).tokens;
        let card = || {
            v_flex()
                .gap_3()
                .p_4()
                .rounded(t.radius_lg)
                .border(t.border_width)
                .border_color(t.frame)
                .bg(t.surface)
        };
        let folder = sheet.folder.clone();

        let identity = card()
            .child(kit::section_heading("SKU e pasta"))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(240.)).child(Input::new(&self.sku).small()))
                    .child(
                        Button::new("rename-sku")
                            .label("Renomear SKU")
                            .outline()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.rename_sku(window, cx)),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(
                        Icon::new(IconName::Folder)
                            .size(px(14.))
                            .text_color(t.text2),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_sm()
                            .text_color(t.text2)
                            .child(folder.display().to_string()),
                    )
                    .child(
                        Button::new("open-folder")
                            .label("Abrir pasta")
                            .ghost()
                            .small()
                            .on_click(move |_, _, cx| cx.open_with_system(&folder)),
                    ),
            );

        let files = card()
            .id("product-files")
            .child(kit::section_heading("Arquivos"))
            .child(
                v_flex()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .py_5()
                    .rounded(t.radius_lg)
                    .border_2()
                    .border_dashed()
                    .border_color(t.border_strong)
                    .bg(t.sunken)
                    .text_sm()
                    .text_color(t.text2)
                    .child(
                        Icon::new(IconName::FolderOpen)
                            .size(px(20.))
                            .text_color(t.text2),
                    )
                    .child("Arraste fotos, notas de compra e prints para esta ficha.")
                    .child(div().text_xs().child(
                        "O app copia para a pasta do produto; os originais ficam onde estão.",
                    )),
            )
            .children(sheet.files.iter().enumerate().map(|(index, file)| {
                let path = file.path.clone();
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::File).size(px(14.)).text_color(t.text2))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(file.name.clone()),
                    )
                    .child(div().text_xs().text_color(t.text2).child(size(file.bytes)))
                    .child(
                        Button::new(("open-file", index))
                            .label("Abrir")
                            .ghost()
                            .small()
                            .on_click(move |_, _, cx| cx.open_with_system(&path)),
                    )
                    .into_any_element()
            }))
            // Dropping anywhere on the card copies; the card lights up while
            // files are over it.
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.border_color(t.accent).bg(t.selected)
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.add_files(paths.paths().to_vec(), window, cx)
            }));

        let offers = card()
            .child(kit::section_heading("Ofertas e histórico de preço"))
            .when(sheet.offers.is_empty(), |card| {
                card.child(
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child("Nenhuma oferta ligada. Ligue ofertas a este produto em Ofertas."),
                )
            })
            .children(
                sheet
                    .offers
                    .iter()
                    .map(|history| price_history(history, cx)),
            );

        let target = card()
            .child(kit::section_heading("Margem alvo"))
            .child(div().text_sm().text_color(t.text2).child(match &sheet.target {
                Ok(target) if target.own => format!(
                    "Este produto tem margem alvo própria de {}. A sugestão de preço dos \
                     anúncios dele parte dela.",
                    target.margin.to_pt_br()
                ),
                Ok(target) => format!(
                    "Sem margem alvo própria: vale a padrão, {}, de Configurações › Preços e \
                     margens. Digite uma para este produto.",
                    target.margin.to_pt_br()
                ),
                Err(error) => error.clone(),
            }))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(120.)).child(Input::new(&self.target).small()))
                    .child(div().text_sm().child("%"))
                    .child(
                        Button::new("save-target")
                            .label("Salvar margem alvo")
                            .outline()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_target(false, window, cx)
                            })),
                    )
                    .when(sheet.target.as_ref().is_ok_and(|t| t.own), |row| {
                        row.child(
                            Button::new("default-target")
                                .label("Usar a padrão")
                                .ghost()
                                .small()
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_target(true, window, cx)
                                })),
                        )
                    }),
            );

        vec![
            identity.into_any_element(),
            target.into_any_element(),
            files.into_any_element(),
            offers.into_any_element(),
        ]
    }
}

/// One link's prices, newest first, the lowest total marked.
fn price_history(history: &OfferHistory, cx: &gpui_kit::App) -> AnyElement {
    let t = look(cx).tokens;
    let lowest = history.lowest_total();
    let several = !history.earlier.is_empty();
    let link = history.latest.link.clone();
    let cell = |text: String| div().w(px(120.)).text_sm().child(text);
    v_flex()
        .gap_1()
        .pt_2()
        .border_t(t.border_width)
        .border_color(t.border)
        .child(div().font_medium().child(format!(
            "{} · {}",
            history.latest.supplier.name, history.latest.title
        )))
        .child(
            div()
                .id(SharedString::from(format!(
                    "history-link-{}",
                    history.latest.id
                )))
                .text_xs()
                .text_color(t.accent_text)
                .truncate()
                .cursor_pointer()
                .child(link.clone())
                .on_click(move |_, _, cx| cx.open_url(&link)),
        )
        .child(
            h_flex()
                .text_xs()
                .text_color(t.text2)
                .child(div().w(px(120.)).child("Data"))
                .child(div().w(px(120.)).child("Preço"))
                .child(div().w(px(120.)).child("Frete"))
                .child(div().w(px(120.)).child("Total")),
        )
        .children(history.offers().map(|offer| {
            let total = offer.total();
            h_flex()
                .items_center()
                .child(cell(day(offer.observed_at)))
                .child(cell(offer.price.to_pt_br()))
                .child(cell(offer.shipping.to_pt_br()))
                .child(cell(total.to_pt_br()).font_medium())
                .when(several && total == lowest, |row| {
                    row.child(kit::tag("menor preço", t.success, cx))
                })
        }))
        .into_any_element()
}

fn added_outcome(added: &AddedFiles) -> Outcome {
    let copied = match added.copied.len() {
        0 => String::new(),
        1 => "1 arquivo copiado para a pasta.".to_owned(),
        n => format!("{n} arquivos copiados para a pasta."),
    };
    if added.not_copied.is_empty() {
        return Outcome::Done(copied.into());
    }
    let not_copied: Vec<String> = added
        .not_copied
        .iter()
        .map(|(path, reason)| {
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into(),
            );
            match reason {
                NotCopied::Folder => format!("{name} é uma pasta (arraste os arquivos dela)"),
                NotCopied::AlreadyThere => format!("{name} já está na pasta"),
                NotCopied::Failed(why) => format!("{name}: {why}"),
            }
        })
        .collect();
    Outcome::Failed(
        format!("{copied} Não copiei: {}.", not_copied.join("; "))
            .trim()
            .to_owned()
            .into(),
    )
}

/// "12 KB", "3,4 MB".
fn size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    match bytes {
        b if b < KB => format!("{b} B"),
        b if b < MB => format!("{} KB", b.div_ceil(KB)),
        b => format!("{:.1} MB", b as f64 / MB as f64).replace('.', ","),
    }
}

impl Render for ProductsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut parts = match &self.sheet {
            Some(sheet) => ScreenParts::new(sheet.product.name.clone()),
            None => ScreenParts::new("Produtos"),
        };
        if catalog::catalog(cx).is_none() {
            parts
                .notices
                .push(kit::error_notice(NO_DATABASE, cx).into_any_element());
            return layout::screen(parts, cx);
        }
        parts
            .notices
            .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        if let Some(sheet) = &self.sheet {
            let product = sheet.product.id;
            parts.actions.push(
                Button::new("start-draft")
                    .label("Criar anúncio")
                    .icon(IconName::Plus)
                    .outline()
                    .small()
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(StartDraft(product))))
                    .into_any_element(),
            );
        }
        if self.open.is_some() {
            parts.actions.push(
                Button::new("back-to-products")
                    .label("Todos os produtos")
                    .icon(IconName::ArrowLeft)
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))
                    .into_any_element(),
            );
        }
        let content = match &self.sheet {
            Some(sheet) => self.render_sheet(sheet, cx),
            None if self.open.is_some() => Vec::new(),
            None => self.render_list(cx),
        };
        parts.content.extend(content);
        layout::screen(parts, cx)
    }
}
