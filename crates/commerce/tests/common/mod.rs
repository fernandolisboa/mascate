//! What the tests of Listings and prices share: an in-memory Sales Channel
//! and listings to fill it with, and an in-memory channel to publish drafts
//! to.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Mutex;

use mascate_commerce::{
    AttributeValue, BilledOrder, Buyer, CatalogProduct, CategoryAttribute, CategoryPrediction,
    ChannelBilling, ChannelCategory, ChannelIssue, ChannelListing, ChannelOrder, ChannelOrderLine,
    ChannelOrders, ChannelPayments, ChannelStock, ListingPublisher, ListingStatus,
    ListingToPublish, OrderStatus, PaymentRelease, PublishedListing, Receiver, Requirement,
    SaleFee, SalesChannel, Shipment, ShipmentStatus, ShippingLabels, Variation,
};
use mascate_kernel::{
    Currency, ListingType, Money, Percentage, PlatformError, RecordId, Timestamp,
};
use rust_decimal::Decimal;

pub fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

pub fn percent(text: &str) -> Percentage {
    Percentage::new(Decimal::from_str(text).unwrap()).unwrap()
}

pub fn listing(id: &str, title: &str) -> ChannelListing {
    ChannelListing {
        id: id.into(),
        variation: None,
        title: title.into(),
        price: brl("89.90"),
        available_quantity: 5,
        status: ListingStatus::Active,
        link: Some(format!("https://produto.mercadolivre.com.br/{id}")),
        listing_type: Some(ListingType::Classic),
        category: Some("MLB3697".into()),
        seller_sku: None,
    }
}

pub fn variation(of: &ChannelListing, id: &str, name: &str) -> ChannelListing {
    ChannelListing {
        variation: Some(Variation {
            id: id.into(),
            name: name.into(),
        }),
        ..of.clone()
    }
}

pub fn product(id: u128, sku: &str, name: &str) -> CatalogProduct {
    CatalogProduct {
        id: RecordId::from_u128(id),
        sku: sku.into(),
        name: name.into(),
    }
}

/// A Sales Channel answering from memory: the listings it has, which of
/// them it lists as active or paused, its sale fee by price band, and what
/// it was asked.
pub struct Channel {
    pub listings: Mutex<Vec<ChannelListing>>,
    pub asked: Mutex<Vec<Vec<String>>>,
    /// Fails every call while set.
    pub failure: Mutex<Option<PlatformError>>,
    /// The sale fee from each price on, lowest price first.
    pub fee_bands: Mutex<Vec<(Money, SaleFee)>>,
    /// The prices it was asked the fee at, in order.
    pub fee_asked: Mutex<Vec<Money>>,
    /// The Listing Types it was asked the fee for, in order.
    pub fee_types: Mutex<Vec<ListingType>>,
    /// Each listing id and price it was told to set, in order.
    pub prices_set: Mutex<Vec<(String, Money)>>,
    /// Each listing id and the stock it was told to set, in order.
    pub stocks_set: Mutex<Vec<(String, Vec<ChannelStock>)>>,
    /// Fails only stock calls while set.
    pub stock_failure: Mutex<Option<PlatformError>>,
    /// Each listing id paused or activated, in order: `true` for paused.
    pub statuses_set: Mutex<Vec<(String, bool)>>,
    /// Listings the owner paused, which units coming back do not reactivate.
    pub paused_by_seller: Mutex<HashSet<String>>,
    /// The owner's Orders, as the channel has them now.
    pub orders: Mutex<Vec<ChannelOrder>>,
    /// The time each read of Orders started from, in order.
    pub orders_asked: Mutex<Vec<Timestamp>>,
    /// The shipments whose labels were asked for, in order.
    pub labels_asked: Mutex<Vec<String>>,
    /// What a label comes as, when not a small PDF.
    pub label_answer: Mutex<Option<Vec<u8>>>,
    /// What the billing has billed so far, by Order.
    pub billed: Mutex<Vec<BilledOrder>>,
    /// The Orders each read of the billing asked about, in order.
    pub billing_asked: Mutex<Vec<Vec<String>>>,
    /// The name of each category the channel has, by id.
    pub categories: Mutex<HashMap<String, String>>,
    /// The categories whose names were asked for, in order.
    pub categories_asked: Mutex<Vec<String>>,
    /// Fails only naming a category while set.
    pub category_failure: Mutex<Option<PlatformError>>,
    /// The wallet's payments, as it reports them now.
    pub payments: Mutex<Vec<PaymentRelease>>,
    /// The Orders each read of the wallet asked about, in order.
    pub payments_asked: Mutex<Vec<Vec<String>>>,
}

impl Default for Channel {
    /// 14% of each sale, nothing fixed.
    fn default() -> Self {
        Self {
            listings: Mutex::default(),
            asked: Mutex::default(),
            failure: Mutex::default(),
            fee_bands: Mutex::new(vec![(
                brl("0"),
                SaleFee {
                    rate: percent("14"),
                    fixed: brl("0"),
                },
            )]),
            fee_asked: Mutex::default(),
            fee_types: Mutex::default(),
            prices_set: Mutex::default(),
            stocks_set: Mutex::default(),
            stock_failure: Mutex::default(),
            statuses_set: Mutex::default(),
            paused_by_seller: Mutex::default(),
            orders: Mutex::default(),
            orders_asked: Mutex::default(),
            labels_asked: Mutex::default(),
            label_answer: Mutex::default(),
            billed: Mutex::default(),
            billing_asked: Mutex::default(),
            categories: Mutex::new(HashMap::from([(
                "MLB3697".to_owned(),
                "Fones de Ouvido".to_owned(),
            )])),
            categories_asked: Mutex::default(),
            category_failure: Mutex::default(),
            payments: Mutex::default(),
            payments_asked: Mutex::default(),
        }
    }
}

impl Channel {
    pub fn has(&self, listings: Vec<ChannelListing>) {
        *self.listings.lock().unwrap() = listings;
    }

    pub fn change(&self, id: &str, change: impl Fn(&mut ChannelListing)) {
        self.listings
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|listing| listing.id == id)
            .for_each(change);
    }

    pub fn remove(&self, id: &str, variation: Option<&str>) {
        self.listings.lock().unwrap().retain(|listing| {
            listing.id != id
                || variation.is_some_and(|wanted| {
                    listing.variation.as_ref().map(|v| v.id.as_str()) != Some(wanted)
                })
        });
    }

    pub fn last_asked(&self) -> Vec<String> {
        self.asked
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap_or_default()
    }

    /// Charges `fee` from `price` on, and the bands below it as before.
    pub fn charges_from(&self, price: &str, fee: SaleFee) {
        let mut bands = self.fee_bands.lock().unwrap();
        bands.push((brl(price), fee));
        bands.sort_by_key(|(from, _)| from.amount());
    }

    /// A buyer buys `order`; like Mercado Livre, the listings sold lose the
    /// units at once.
    pub fn sells(&self, order: ChannelOrder) {
        for line in &order.lines {
            self.change(&line.item, |listing| {
                if listing.variation.as_ref().map(|v| v.id.clone()) == line.variation {
                    listing.available_quantity =
                        listing.available_quantity.saturating_sub(line.quantity);
                }
            });
        }
        self.orders.lock().unwrap().push(order);
    }

    /// The channel changes the Order `id`, as of `at`.
    pub fn change_order(&self, id: &str, at: Timestamp, change: impl Fn(&mut ChannelOrder)) {
        for order in self.orders.lock().unwrap().iter_mut() {
            if order.id == id {
                change(order);
                order.updated_at = at;
            }
        }
    }

    fn check(&self) -> Result<(), PlatformError> {
        match self.failure.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl SalesChannel for Channel {
    fn listing_ids(&self) -> Result<Vec<String>, PlatformError> {
        self.check()?;
        let mut ids: Vec<String> = Vec::new();
        for listing in self.listings.lock().unwrap().iter() {
            if matches!(
                listing.status,
                ListingStatus::Active | ListingStatus::Paused
            ) && !ids.contains(&listing.id)
            {
                ids.push(listing.id.clone());
            }
        }
        Ok(ids)
    }

    fn listings(&self, ids: &[String]) -> Result<Vec<ChannelListing>, PlatformError> {
        self.check()?;
        self.asked.lock().unwrap().push(ids.to_vec());
        Ok(self
            .listings
            .lock()
            .unwrap()
            .iter()
            .filter(|listing| ids.contains(&listing.id))
            .cloned()
            .collect())
    }

    fn sale_fee(
        &self,
        _category: &str,
        price: Money,
        listing_type: ListingType,
    ) -> Result<SaleFee, PlatformError> {
        self.check()?;
        self.fee_asked.lock().unwrap().push(price);
        self.fee_types.lock().unwrap().push(listing_type);
        let bands = self.fee_bands.lock().unwrap();
        Ok(bands
            .iter()
            .rev()
            .find(|(from, _)| price.amount() >= from.amount())
            .map(|(_, fee)| *fee)
            .expect("a band from zero"))
    }

    fn set_price(&self, id: &str, price: Money) -> Result<(), PlatformError> {
        self.check()?;
        self.prices_set.lock().unwrap().push((id.to_owned(), price));
        self.change(id, |listing| listing.price = price);
        Ok(())
    }

    /// Like Mercado Livre, pauses a listing left with no units and puts it
    /// back on sale when units return, unless the owner paused it.
    fn set_stock(&self, id: &str, stock: &[ChannelStock]) -> Result<(), PlatformError> {
        self.check()?;
        if let Some(error) = self.stock_failure.lock().unwrap().clone() {
            return Err(error);
        }
        self.stocks_set
            .lock()
            .unwrap()
            .push((id.to_owned(), stock.to_vec()));
        for wanted in stock {
            self.change(id, |listing| {
                if listing.variation.as_ref().map(|v| v.id.clone()) == wanted.variation {
                    listing.available_quantity = wanted.available_quantity;
                }
            });
        }
        let total: u32 = self
            .listings
            .lock()
            .unwrap()
            .iter()
            .filter(|listing| listing.id == id)
            .map(|listing| listing.available_quantity)
            .sum();
        let by_seller = self.paused_by_seller.lock().unwrap().contains(id);
        self.change(id, |listing| {
            listing.status = match (listing.status, total) {
                (ListingStatus::Active, 0) => ListingStatus::Paused,
                (ListingStatus::Paused, 1..) if !by_seller => ListingStatus::Active,
                (status, _) => status,
            }
        });
        Ok(())
    }

    fn pause(&self, id: &str) -> Result<(), PlatformError> {
        self.check()?;
        self.statuses_set
            .lock()
            .unwrap()
            .push((id.to_owned(), true));
        self.paused_by_seller.lock().unwrap().insert(id.to_owned());
        self.change(id, |listing| listing.status = ListingStatus::Paused);
        Ok(())
    }

    fn activate(&self, id: &str) -> Result<(), PlatformError> {
        self.check()?;
        self.statuses_set
            .lock()
            .unwrap()
            .push((id.to_owned(), false));
        self.paused_by_seller.lock().unwrap().remove(id);
        self.change(id, |listing| listing.status = ListingStatus::Active);
        Ok(())
    }

    fn category(&self, id: &str) -> Result<ChannelCategory, PlatformError> {
        self.check()?;
        self.categories_asked.lock().unwrap().push(id.to_owned());
        if let Some(error) = self.category_failure.lock().unwrap().clone() {
            return Err(error);
        }
        let name = self.categories.lock().unwrap().get(id).cloned();
        Ok(category(id, &name.ok_or(PlatformError::NotFound)?))
    }
}

impl ChannelPayments for Channel {
    fn releases(&self, orders: &[String]) -> Result<Vec<PaymentRelease>, PlatformError> {
        self.check()?;
        self.payments_asked.lock().unwrap().push(orders.to_vec());
        Ok(self
            .payments
            .lock()
            .unwrap()
            .iter()
            .filter(|payment| orders.contains(&payment.order))
            .cloned()
            .collect())
    }
}

impl ChannelOrders for Channel {
    fn orders_changed_since(&self, since: Timestamp) -> Result<Vec<ChannelOrder>, PlatformError> {
        self.check()?;
        self.orders_asked.lock().unwrap().push(since);
        Ok(self
            .orders
            .lock()
            .unwrap()
            .iter()
            .filter(|order| order.updated_at >= since)
            .cloned()
            .collect())
    }
}

impl ChannelBilling for Channel {
    fn billed_fees(&self, orders: &[String]) -> Result<Vec<BilledOrder>, PlatformError> {
        self.check()?;
        self.billing_asked.lock().unwrap().push(orders.to_vec());
        Ok(self
            .billed
            .lock()
            .unwrap()
            .iter()
            .filter(|billed| orders.contains(&billed.order))
            .cloned()
            .collect())
    }
}

impl ShippingLabels for Channel {
    fn label_pdf(&self, shipment: &str) -> Result<Vec<u8>, PlatformError> {
        self.check()?;
        self.labels_asked.lock().unwrap().push(shipment.to_owned());
        Ok(self
            .label_answer
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| format!("%PDF-1.4 label {shipment}").into_bytes()))
    }
}

/// An Order paid at `at` for the lines given, shipped to Ana in Recife.
pub fn order(id: &str, at: Timestamp, lines: Vec<ChannelOrderLine>) -> ChannelOrder {
    let total = Money::sum(
        Currency::Brl,
        lines
            .iter()
            .map(|line| line.unit_price.times(Decimal::from(line.quantity))),
    )
    .unwrap();
    ChannelOrder {
        id: id.into(),
        pack: None,
        status: OrderStatus::Paid,
        ordered_at: at,
        updated_at: at,
        lines,
        total,
        paid: Some(total.checked_add(brl("19.90")).unwrap()),
        shipping_paid: Some(brl("19.90")),
        refunded: Some(brl("0")),
        buyer: Some(Buyer {
            nickname: Some("ANA.COMPRA".into()),
            receiver: Some(Receiver {
                name: "Ana Souza".into(),
                address: Some("Rua da Aurora, 100, ap 12".into()),
                city: Some("Recife".into()),
                state: Some("Pernambuco".into()),
                zip_code: Some("50050000".into()),
            }),
        }),
        shipment: Some(Shipment {
            id: format!("4{id}"),
            status: ShipmentStatus::ReadyToShip,
            dispatch_by: Some(at + chrono::TimeDelta::days(1)),
            seller_cost: Some(brl("21.45")),
        }),
        returns: Vec::new(),
    }
}

/// `quantity` units of the listing `item`, or of its `variation`, at R$ 89,90.
pub fn sold(item: &str, variation: Option<&str>, quantity: u32) -> ChannelOrderLine {
    ChannelOrderLine {
        item: item.into(),
        variation: variation.map(Into::into),
        title: format!("Anúncio {item}"),
        variation_name: variation.map(|_| "Cor: Preto".into()),
        quantity,
        unit_price: brl("89.90"),
        sale_fee: Some(brl("12.59")),
        listing_type: Some(ListingType::Classic),
    }
}

pub fn category(id: &str, name: &str) -> ChannelCategory {
    ChannelCategory {
        id: id.into(),
        name: name.into(),
    }
}

pub fn attribute(id: &str, name: &str, requirement: Requirement) -> CategoryAttribute {
    CategoryAttribute {
        id: id.into(),
        name: name.into(),
        requirement,
    }
}

pub fn value(id: &str, value: &str) -> AttributeValue {
    AttributeValue {
        id: id.into(),
        value: value.into(),
    }
}

/// What a publish call does.
#[derive(Clone)]
pub enum Publishing {
    Creates,
    /// Creates the listing, but the answer never arrives.
    CreatesUnanswered,
    Fails(PlatformError),
}

/// A channel to publish to, answering from memory: its category
/// predictions and attributes, its validator's issues, the listings it has
/// by seller SKU, and what it was asked.
pub struct Publisher {
    pub predictions: Mutex<Vec<CategoryPrediction>>,
    pub attributes: Mutex<HashMap<String, Vec<CategoryAttribute>>>,
    pub issues: Mutex<Vec<ChannelIssue>>,
    pub publishing: Mutex<Publishing>,
    pub describing: Mutex<Option<PlatformError>>,
    /// Fails every call while set.
    pub failure: Mutex<Option<PlatformError>>,
    /// Each picture's file name and bytes.
    pub uploaded: Mutex<Vec<(String, Vec<u8>)>>,
    pub validated: Mutex<Vec<ListingToPublish>>,
    pub published: Mutex<Vec<ListingToPublish>>,
    pub described: Mutex<Vec<(String, String)>>,
    pub searched: Mutex<Vec<String>>,
    /// The listings it has, with their seller SKU.
    pub listings: Mutex<Vec<(String, PublishedListing)>>,
}

impl Default for Publisher {
    /// Predicts "Fones de Ouvido", which requires a brand and a model,
    /// recommends a GTIN and has an optional colour.
    fn default() -> Self {
        Self {
            predictions: Mutex::new(vec![CategoryPrediction {
                category: category("MLB196208", "Fones de Ouvido"),
                attributes: vec![value("BRAND", "Lenovo"), value("COLOR", "Preto")],
            }]),
            attributes: Mutex::new(HashMap::from([(
                "MLB196208".to_owned(),
                vec![
                    attribute("BRAND", "Marca", Requirement::Required),
                    attribute("MODEL", "Modelo", Requirement::Required),
                    attribute(
                        "GTIN",
                        "Código universal de produto",
                        Requirement::Recommended,
                    ),
                    attribute("COLOR", "Cor", Requirement::Optional),
                    attribute("WEIGHT", "Peso", Requirement::Optional),
                ],
            )])),
            issues: Mutex::default(),
            publishing: Mutex::new(Publishing::Creates),
            describing: Mutex::default(),
            failure: Mutex::default(),
            uploaded: Mutex::default(),
            validated: Mutex::default(),
            published: Mutex::default(),
            described: Mutex::default(),
            searched: Mutex::default(),
            listings: Mutex::default(),
        }
    }
}

impl Publisher {
    fn check(&self) -> Result<(), PlatformError> {
        match self.failure.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub fn publishes(&self, publishing: Publishing) {
        *self.publishing.lock().unwrap() = publishing;
    }

    pub fn publish_calls(&self) -> usize {
        self.published.lock().unwrap().len()
    }
}

impl ListingPublisher for Publisher {
    fn currency(&self) -> Currency {
        Currency::Brl
    }

    fn predict_categories(&self, _title: &str) -> Result<Vec<CategoryPrediction>, PlatformError> {
        self.check()?;
        Ok(self.predictions.lock().unwrap().clone())
    }

    fn category_attributes(&self, category: &str) -> Result<Vec<CategoryAttribute>, PlatformError> {
        self.check()?;
        self.attributes
            .lock()
            .unwrap()
            .get(category)
            .cloned()
            .ok_or(PlatformError::NotFound)
    }

    fn upload_picture(&self, file_name: &str, bytes: &[u8]) -> Result<String, PlatformError> {
        self.check()?;
        let mut uploaded = self.uploaded.lock().unwrap();
        uploaded.push((file_name.to_owned(), bytes.to_vec()));
        Ok(format!("PIC-{}", uploaded.len()))
    }

    fn validate(&self, listing: &ListingToPublish) -> Result<Vec<ChannelIssue>, PlatformError> {
        self.check()?;
        self.validated.lock().unwrap().push(listing.clone());
        Ok(self.issues.lock().unwrap().clone())
    }

    fn publish(&self, listing: &ListingToPublish) -> Result<PublishedListing, PlatformError> {
        self.check()?;
        self.published.lock().unwrap().push(listing.clone());
        let mut listings = self.listings.lock().unwrap();
        let created = PublishedListing {
            id: format!("MLB{}", 9_000 + listings.len()),
            status: if listing.available_quantity == 0 {
                ListingStatus::Paused
            } else {
                ListingStatus::Active
            },
            link: Some(format!(
                "https://produto.mercadolivre.com.br/MLB{}",
                9_000 + listings.len()
            )),
        };
        match self.publishing.lock().unwrap().clone() {
            Publishing::Creates => {
                listings.push((listing.seller_sku.clone(), created.clone()));
                Ok(created)
            }
            Publishing::CreatesUnanswered => {
                listings.push((listing.seller_sku.clone(), created));
                Err(PlatformError::Failed("the connection dropped".into()))
            }
            Publishing::Fails(error) => Err(error),
        }
    }

    fn describe(&self, id: &str, description: &str) -> Result<(), PlatformError> {
        self.check()?;
        if let Some(error) = self.describing.lock().unwrap().clone() {
            return Err(error);
        }
        self.described
            .lock()
            .unwrap()
            .push((id.to_owned(), description.to_owned()));
        Ok(())
    }

    fn find_by_seller_sku(&self, seller_sku: &str) -> Result<Vec<PublishedListing>, PlatformError> {
        self.check()?;
        self.searched.lock().unwrap().push(seller_sku.to_owned());
        Ok(self
            .listings
            .lock()
            .unwrap()
            .iter()
            .filter(|(sku, _)| sku == seller_sku)
            .map(|(_, listing)| listing.clone())
            .collect())
    }
}
