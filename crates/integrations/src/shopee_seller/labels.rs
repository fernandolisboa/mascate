//! The label of a Shopee Order, for commerce's labels port (ADR 0033):
//! Shopee prints it only once the shipment is arranged, so the adapter
//! arranges it first (`get_shipping_parameter`, then `ship_order` for a
//! drop-off or a pickup), waits for the carrier's tracking number, then has
//! Shopee make the document and downloads it as a PDF. Shopee keeps what
//! each step did, and every step starts by reading it back: a click after
//! a lost answer, a restart or a shipment arranged in the Seller Center
//! goes on from there, and a parcel is never arranged twice.

use std::collections::BTreeSet;
use std::time::Duration;

use mascate_commerce::{LabelError, ShippingLabels};
use mascate_kernel::PlatformError;
use serde_json::{Value, json};

use super::answers::{
    DocumentResult, DocumentResults, OrderDetails, PackageDetail, PackageDetails, PickupAddress,
    ShippingParameter, TrackingNumber,
};
use super::{ShopeeSeller, is_order_sn};

const ORDER_DETAIL: &str = "/api/v2/order/get_order_detail";
const PACKAGE_DETAIL: &str = "/api/v2/order/get_package_detail";
const SHIPPING_PARAMETER: &str = "/api/v2/logistics/get_shipping_parameter";
const SHIP_ORDER: &str = "/api/v2/logistics/ship_order";
const TRACKING_NUMBER: &str = "/api/v2/logistics/get_tracking_number";
const DOCUMENT_PARAMETER: &str = "/api/v2/logistics/get_shipping_document_parameter";
const CREATE_DOCUMENT: &str = "/api/v2/logistics/create_shipping_document";
const DOCUMENT_RESULT: &str = "/api/v2/logistics/get_shipping_document_result";
const DOWNLOAD_DOCUMENT: &str = "/api/v2/logistics/download_shipping_document";

/// Waiting for the tracking number or the document, the app reads every
/// few seconds for half a minute; past that, the owner asks again later and
/// the flow goes on from where it stopped.
const READS: u32 = 10;
const READ_EVERY: Duration = Duration::from_secs(3);

/// Times an answer to `ship_order` can be lost before the owner is told.
const ARRANGE_TRIES: u32 = 3;

/// The 10 x 15 cm label for thermal printers, when Shopee suggests none.
const THERMAL_LABEL: &str = "THERMAL_AIR_WAYBILL";

impl ShippingLabels for ShopeeSeller {
    /// The labels of every parcel of the Order `shipment` (its number), in
    /// one PDF, its shipment arranged first when Shopee still waits for
    /// it.
    fn label_pdf(&self, shipment: &str) -> Result<Vec<u8>, LabelError> {
        if !is_order_sn(shipment) {
            return Err(PlatformError::NotFound.into());
        }
        let packages = self.arranged_packages(shipment)?;
        let mut tracked = Vec::new();
        for package in &packages {
            tracked.push((package.as_str(), self.tracking_number(shipment, package)?));
        }
        let kind = self.document_kind(shipment, &packages)?;
        let documents: Vec<Value> = tracked
            .iter()
            .map(|(package, tracking)| {
                json!({
                    "order_sn": shipment,
                    "package_number": package,
                    "tracking_number": tracking,
                    "shipping_document_type": kind,
                })
            })
            .collect();
        let created: DocumentResults = self.post(
            CREATE_DOCUMENT,
            &json!({ "order_list": documents }).to_string(),
        )?;
        refusal(&created.result_list)?;
        self.ready_documents(shipment, &packages, &kind)?;
        let order_list: Vec<Value> = packages
            .iter()
            .map(|package| json!({ "order_sn": shipment, "package_number": package }))
            .collect();
        let body = json!({ "shipping_document_type": kind, "order_list": order_list });
        Ok(self.post_for_document(DOWNLOAD_DOCUMENT, &body.to_string())?)
    }
}

impl ShopeeSeller {
    /// The parcels of `order` whose labels can be printed, each arranged
    /// once: one Shopee still waits for is arranged, and after a lost
    /// answer the parcels are read again before asking again.
    fn arranged_packages(&self, order: &str) -> Result<Vec<String>, LabelError> {
        let mut arranged = BTreeSet::new();
        let mut lost = 0;
        loop {
            let packages = self.packages(order)?;
            let Some(waiting) = packages.iter().find(|package| {
                awaits_arranging(package) && !arranged.contains(&package.package_number)
            }) else {
                let printable: Vec<String> = packages
                    .into_iter()
                    .filter(|package| {
                        is_arranged(package) || arranged.contains(&package.package_number)
                    })
                    .map(|package| package.package_number)
                    .collect();
                return if printable.is_empty() {
                    Err(LabelError::NotWaiting)
                } else {
                    Ok(printable)
                };
            };
            match self.arrange(order, &waiting.package_number) {
                Ok(()) => {
                    arranged.insert(waiting.package_number.clone());
                }
                // Shopee may have arranged it all the same: the next read
                // tells before asking again.
                Err(LabelError::Platform(
                    error @ (PlatformError::Failed(_) | PlatformError::RateLimited),
                )) => {
                    lost += 1;
                    if lost == ARRANGE_TRIES {
                        return Err(error.into());
                    }
                    self.pause.pause(READ_EVERY);
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// The parcels of `order` as Shopee has them now; an Order that cannot
    /// ship says why.
    fn packages(&self, order: &str) -> Result<Vec<PackageDetail>, LabelError> {
        let details: OrderDetails = self.get(
            ORDER_DETAIL,
            &[
                ("order_sn_list", order),
                ("response_optional_fields", "package_list"),
            ],
        )?;
        let Some(detail) = details
            .order_list
            .into_iter()
            .find(|detail| detail.order_sn == order)
        else {
            return Err(PlatformError::NotFound.into());
        };
        match detail.order_status.as_str() {
            "READY_TO_SHIP" | "PROCESSED" | "RETRY_SHIP" => {}
            "INVOICE_PENDING" => return Err(LabelError::AwaitingInvoice),
            // Unpaid, shipped, delivered, cancelled or with a cancellation
            // the buyer asked for and the seller has not answered.
            _ => return Err(LabelError::NotWaiting),
        }
        let numbers: Vec<String> = detail
            .package_list
            .into_iter()
            .map(|package| package.package_number)
            .collect();
        if numbers.is_empty() {
            return Err(LabelError::NotWaiting);
        }
        let found: PackageDetails = self.get(
            PACKAGE_DETAIL,
            &[("package_number_list", &numbers.join(","))],
        )?;
        Ok(found
            .package_list
            .into_iter()
            .filter(|package| numbers.contains(&package.package_number))
            .collect())
    }

    /// Arranges `package` of `order` the way Shopee offers that the app can
    /// fill in: a drop-off that needs nothing, since the owner takes the
    /// parcel at their own time, or else a pickup at the shop's pickup
    /// address in the slot Shopee recommends.
    fn arrange(&self, order: &str, package: &str) -> Result<(), LabelError> {
        let parameter: ShippingParameter = self.get(
            SHIPPING_PARAMETER,
            &[("order_sn", order), ("package_number", package)],
        )?;
        let (way, details) = if parameter
            .info_needed
            .dropoff
            .as_ref()
            .is_some_and(|needs| needs.is_empty())
        {
            ("dropoff", json!({}))
        } else if let Some(details) = parameter
            .info_needed
            .pickup
            .as_ref()
            .and_then(|needs| pickup(needs, &parameter))
        {
            ("pickup", details)
        } else {
            return Err(LabelError::ArrangeOnChannel);
        };
        let body = json!({ "order_sn": order, "package_number": package, way: details });
        Ok(self.post_once(SHIP_ORDER, &body.to_string())?)
    }

    /// The carrier's tracking number of `package`, read until Shopee has it.
    fn tracking_number(&self, order: &str, package: &str) -> Result<String, LabelError> {
        for read in 0..READS {
            if read > 0 {
                self.pause.pause(READ_EVERY);
            }
            let found: TrackingNumber = self.get(
                TRACKING_NUMBER,
                &[("order_sn", order), ("package_number", package)],
            )?;
            if !found.tracking_number.trim().is_empty() {
                return Ok(found.tracking_number);
            }
        }
        Err(LabelError::NotReadyYet)
    }

    /// The kind of document Shopee suggests for the parcels.
    fn document_kind(&self, order: &str, packages: &[String]) -> Result<String, LabelError> {
        let answer: DocumentResults =
            self.post(DOCUMENT_PARAMETER, &documents(order, packages, None))?;
        refusal(&answer.result_list)?;
        Ok(answer
            .result_list
            .into_iter()
            .find_map(|result| result.suggest_shipping_document_type)
            .filter(|kind| !kind.trim().is_empty())
            .unwrap_or_else(|| THERMAL_LABEL.to_owned()))
    }

    /// Reads until Shopee has made the document of every parcel; one still
    /// in the making is read again, not taken as failed.
    fn ready_documents(
        &self,
        order: &str,
        packages: &[String],
        kind: &str,
    ) -> Result<(), LabelError> {
        for read in 0..READS {
            if read > 0 {
                self.pause.pause(READ_EVERY);
            }
            let answer: DocumentResults =
                self.post(DOCUMENT_RESULT, &documents(order, packages, Some(kind)))?;
            refusal(&answer.result_list)?;
            if let Some(failed) = answer
                .result_list
                .iter()
                .find(|result| result.status.as_deref() == Some("FAILED"))
            {
                return Err(PlatformError::Refused(
                    failed
                        .fail_message
                        .clone()
                        .unwrap_or_else(|| "Shopee could not make the label".into()),
                )
                .into());
            }
            if !answer.result_list.is_empty()
                && answer
                    .result_list
                    .iter()
                    .all(|result| result.status.as_deref() == Some("READY"))
            {
                return Ok(());
            }
        }
        Err(LabelError::NotReadyYet)
    }
}

/// A parcel Shopee still waits to see arranged: ready and not arranged,
/// or arranged again after the carrier missed the pickup.
fn awaits_arranging(package: &PackageDetail) -> bool {
    match package.fulfillment_status.as_str() {
        "LOGISTICS_READY" => !package.is_shipment_arranged,
        "LOGISTICS_PICKUP_RETRY" => true,
        _ => false,
    }
}

/// A parcel arranged and not yet with the carrier, whose label prints.
fn is_arranged(package: &PackageDetail) -> bool {
    match package.fulfillment_status.as_str() {
        "LOGISTICS_READY" => package.is_shipment_arranged,
        "LOGISTICS_REQUEST_CREATED" => true,
        _ => false,
    }
}

/// The pickup `parameter` offers, when the app can fill in all it `needs`:
/// the shop's pickup address (or its default one, or the first) and its
/// recommended slot (or the first).
fn pickup(needs: &[String], parameter: &ShippingParameter) -> Option<Value> {
    if needs
        .iter()
        .any(|need| need != "address_id" && need != "pickup_time_id")
    {
        return None;
    }
    let addresses = &parameter.pickup.as_ref()?.address_list;
    let flagged = |flag: &str| {
        addresses
            .iter()
            .find(|address| address.address_flag.iter().any(|found| found == flag))
    };
    let address: &PickupAddress = flagged("pickup_address")
        .or_else(|| flagged("default_address"))
        .or_else(|| addresses.first())?;
    let slot = address
        .time_slot_list
        .iter()
        .find(|slot| slot.flags.iter().any(|flag| flag == "recommended"))
        .or_else(|| address.time_slot_list.first());
    match slot {
        Some(slot) => Some(json!({
            "address_id": address.address_id,
            "pickup_time_id": slot.pickup_time_id,
        })),
        None if needs.iter().any(|need| need == "pickup_time_id") => None,
        // Shopee picks the time itself for some carriers.
        None => Some(json!({ "address_id": address.address_id })),
    }
}

/// The body that names the parcels of `order`, with the document `kind`.
fn documents(order: &str, packages: &[String], kind: Option<&str>) -> String {
    let order_list: Vec<Value> = packages
        .iter()
        .map(|package| {
            let mut entry = json!({ "order_sn": order, "package_number": package });
            if let Some(kind) = kind {
                entry["shipping_document_type"] = kind.into();
            }
            entry
        })
        .collect();
    json!({ "order_list": order_list }).to_string()
}

/// The first parcel Shopee turned down in `results`, as an error.
fn refusal(results: &[DocumentResult]) -> Result<(), LabelError> {
    match results.iter().find_map(|result| {
        result
            .fail_error
            .as_deref()
            .filter(|error| !error.is_empty())
            .map(|error| {
                result
                    .fail_message
                    .clone()
                    .filter(|message| !message.trim().is_empty())
                    .unwrap_or_else(|| error.to_owned())
            })
    }) {
        Some(message) => Err(PlatformError::Refused(message).into()),
        None => Ok(()),
    }
}
