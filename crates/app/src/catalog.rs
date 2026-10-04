//! What the Ofertas and Produtos screens (#9) share: the catalog module and
//! how its values and failures read in Portuguese.

use std::sync::Arc;

use chrono::Local;
use gpui_kit::{App, Global};
use mascate_catalog::{Catalog, CatalogError, InvalidLink, InvalidSku, Product, SupplierOffer};
use mascate_kernel::Timestamp;

use crate::forms::Choice;

/// The app's catalog; absent when the database did not open.
pub struct AppCatalog(pub Arc<Catalog>);

impl Global for AppCatalog {}

pub fn catalog(cx: &App) -> Option<Arc<Catalog>> {
    cx.try_global::<AppCatalog>().map(|app| app.0.clone())
}

pub const NO_DATABASE: &str = "O banco de dados não abriu; a tela Hoje diz por quê.";

pub fn day(at: Timestamp) -> String {
    at.with_timezone(&Local).format("%d/%m/%Y").to_string()
}

/// "R$ 29,90 + R$ 5,00 de frete".
pub fn price_and_shipping(offer: &SupplierOffer) -> String {
    format!(
        "{} + {} de frete",
        offer.price.to_pt_br(),
        offer.shipping.to_pt_br()
    )
}

pub fn product_choice(product: &Product) -> Choice {
    Choice {
        id: product.id,
        title: format!("{} · {}", product.sku, product.name).into(),
    }
}

/// Why a catalog action failed, as the owner reads it.
pub fn failure(error: &CatalogError) -> String {
    match error {
        CatalogError::MissingSupplierName => "Digite o nome do fornecedor.".into(),
        CatalogError::MissingTitle => "Digite o título da oferta.".into(),
        CatalogError::MissingProductName => "Digite o nome do produto.".into(),
        CatalogError::SupplierExists(name) => format!("O fornecedor {name} já existe."),
        CatalogError::UnknownSupplier(_)
        | CatalogError::UnknownOffer(_)
        | CatalogError::UnknownProduct(_) => {
            "Esse item não existe mais; a lista foi atualizada.".into()
        }
        CatalogError::InvalidLink(InvalidLink::Empty) => "Cole o link da oferta.".into(),
        CatalogError::InvalidLink(InvalidLink::NotWeb) => {
            "O link precisa começar com http:// ou https://.".into()
        }
        CatalogError::InvalidLink(InvalidLink::NoHost) => {
            "O link não tem o endereço do site.".into()
        }
        CatalogError::InvalidLink(InvalidLink::Space) => {
            "O link tem um espaço no meio; copie e cole de novo.".into()
        }
        CatalogError::InvalidSku(InvalidSku::Empty) => "Digite um SKU.".into(),
        CatalogError::InvalidSku(InvalidSku::TooLong) => {
            format!(
                "O SKU pode ter até {} caracteres.",
                mascate_catalog::Sku::MAX_LENGTH
            )
        }
        CatalogError::InvalidSku(InvalidSku::Character(c)) => {
            format!("O SKU aceita só letras, números e hífen, não “{c}”.")
        }
        CatalogError::InvalidSku(InvalidSku::Reserved(sku)) => {
            format!("O Windows reserva o nome {sku} para dispositivos; escolha outro SKU.")
        }
        CatalogError::SkuTaken(sku) => format!("Já existe um produto com o SKU {sku}."),
        CatalogError::FolderTaken(path) => format!(
            "Já existe a pasta {} e ela não é de nenhum produto; mova ou apague essa pasta antes.",
            path.display()
        ),
        CatalogError::Negative => "Preço e frete não podem ser negativos.".into(),
        CatalogError::Currencies(_) => "Preço e frete precisam estar na mesma moeda.".into(),
        CatalogError::Folder { path, source } => format!(
            "Não consegui usar a pasta {}: {source}. Se um arquivo dela estiver aberto, feche e \
             tente de novo.",
            path.display()
        ),
        CatalogError::Unreadable(_) | CatalogError::Sql(_) => {
            format!("Não consegui ler ou gravar no banco: {error}")
        }
    }
}
