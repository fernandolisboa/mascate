//! What the tests of Listings and prices share: an in-memory Sales Channel
//! and listings to fill it with.

#![allow(dead_code)]

use std::str::FromStr;
use std::sync::Mutex;

use mascate_commerce::{
    CatalogProduct, ChannelListing, ListingStatus, SaleFee, SalesChannel, Variation,
};
use mascate_kernel::{Currency, ListingType, Money, Percentage, PlatformError, RecordId};
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
    /// Each listing id and price it was told to set, in order.
    pub prices_set: Mutex<Vec<(String, Money)>>,
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
            prices_set: Mutex::default(),
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
        _listing_type: ListingType,
    ) -> Result<SaleFee, PlatformError> {
        self.check()?;
        self.fee_asked.lock().unwrap().push(price);
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
}
