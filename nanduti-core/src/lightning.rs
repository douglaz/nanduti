//! Lightning payment operations

use anyhow::{Context, Result};
use lightning_invoice::Bolt11Invoice;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::models::{
    Amount, Bolt11String, Description, Expiry, Invoice, PaymentHash, Preimage, PublicKey,
};

/// Result of a lightning payment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentResult {
    pub preimage: Preimage,
    pub fees_paid: Option<Amount>,
    pub payment_hash: PaymentHash,
    pub amount_paid: Amount,
}

/// Lightning operation handler
pub struct LightningOperation;

impl LightningOperation {
    /// Parse a BOLT11 invoice
    pub fn parse_invoice(bolt11: &str) -> Result<Invoice> {
        // Parse using lightning-invoice crate
        let parsed = Bolt11Invoice::from_str(bolt11).context("Failed to parse BOLT11 invoice")?;

        // Extract payment hash
        let payment_hash_bytes: &[u8] = parsed.payment_hash().as_ref();
        let payment_hash = PaymentHash(hex::encode(payment_hash_bytes));

        // Extract amount if present
        let amount = parsed.amount_milli_satoshis().map(Amount::from_msats);

        // Extract description
        let description = match parsed.description() {
            lightning_invoice::Bolt11InvoiceDescriptionRef::Direct(desc) => {
                Some(Description(desc.to_string()))
            }
            lightning_invoice::Bolt11InvoiceDescriptionRef::Hash(_) => {
                Some(Description("Payment with description hash".to_string()))
            }
        };

        // Extract expiry (default is 3600 seconds if not specified)
        let expiry = Some(Expiry(parsed.expiry_time().as_secs()));

        // Extract payee pubkey if present
        let payee_pubkey = parsed
            .payee_pub_key()
            .map(|pk| PublicKey(hex::encode(pk.serialize())));

        Ok(Invoice {
            bolt11: Bolt11String(bolt11.to_string()),
            payment_hash,
            amount,
            description,
            expiry,
            payee_pubkey,
        })
    }

    /// Validate an invoice
    pub fn validate_invoice(bolt11: &str) -> Result<()> {
        // Parse the invoice to get actual timestamps
        let parsed = Bolt11Invoice::from_str(bolt11)
            .context("Failed to parse BOLT11 invoice for validation")?;

        // Check if invoice is expired
        let creation_time = parsed.timestamp();
        let expiry_duration = parsed.expiry_time();

        let now = SystemTime::now();

        // Calculate when the invoice expires
        let expiry_time = creation_time + expiry_duration;

        if now > expiry_time {
            let creation_secs = creation_time
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let expiry_secs = expiry_duration.as_secs();
            anyhow::bail!(
                "Invoice has expired (created at {creation_secs}, expires after {expiry_secs} seconds)"
            );
        }

        Ok(())
    }

    /// Calculate route hints for an invoice
    pub fn get_route_hints(bolt11: &str) -> Result<Vec<RouteHint>> {
        // Parse the invoice to extract route hints
        let parsed = Bolt11Invoice::from_str(bolt11)
            .context("Failed to parse BOLT11 invoice for route hints")?;

        let mut route_hints = Vec::new();

        // Extract private route hints from the invoice
        for route in parsed.private_routes() {
            for hop in route.0.iter() {
                let hint = RouteHint {
                    pubkey: hex::encode(hop.src_node_id.serialize()),
                    short_channel_id: hop.short_channel_id.to_string(),
                    fee_base_msat: hop.fees.base_msat,
                    fee_proportional_millionths: hop.fees.proportional_millionths,
                    cltv_expiry_delta: hop.cltv_expiry_delta,
                };
                route_hints.push(hint);
            }
        }

        Ok(route_hints)
    }
}

/// Route hint for lightning payments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteHint {
    pub pubkey: String,
    pub short_channel_id: String,
    pub fee_base_msat: u32,
    pub fee_proportional_millionths: u32,
    pub cltv_expiry_delta: u16,
}
