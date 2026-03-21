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
        let payment_hash = PaymentHash::new(hex::encode(payment_hash_bytes));

        // Extract amount if present
        let amount = parsed.amount_milli_satoshis().map(Amount::from_msats);

        // Extract description
        let description = match parsed.description() {
            lightning_invoice::Bolt11InvoiceDescriptionRef::Direct(desc) => {
                Some(Description::new(desc.to_string()))
            }
            lightning_invoice::Bolt11InvoiceDescriptionRef::Hash(_) => Some(Description::new(
                "Payment with description hash".to_string(),
            )),
        };

        // Extract expiry (default is 3600 seconds if not specified)
        let expiry = Some(Expiry::from_secs(parsed.expiry_time().as_secs()));

        // Extract payee pubkey if present
        let payee_pubkey = parsed
            .payee_pub_key()
            .map(|pk| PublicKey::new(hex::encode(pk.serialize())));

        // Extract creation timestamp
        let created_at = Some(parsed.timestamp());

        Ok(Invoice {
            bolt11: Bolt11String::new(bolt11.to_string()),
            payment_hash,
            amount,
            description,
            expiry,
            payee_pubkey,
            created_at,
            operation_id: None,
        })
    }

    /// Validate an invoice
    pub fn validate_invoice(invoice: &Invoice) -> Result<()> {
        // Get creation time and expiry duration
        let created_at = invoice
            .created_at
            .context("Invoice missing creation timestamp")?;
        let expiry_secs = invoice
            .expiry
            .map(|e| e.as_secs())
            .unwrap_or(crate::constants::DEFAULT_INVOICE_EXPIRY_SECS);

        let now = SystemTime::now();

        // Calculate when the invoice expires
        let expiry_duration = std::time::Duration::from_secs(expiry_secs);
        let expiry_time = created_at + expiry_duration;

        if now > expiry_time {
            let creation_secs = created_at
                .duration_since(UNIX_EPOCH)
                .context("Invalid invoice creation timestamp")?
                .as_secs();
            anyhow::bail!(
                "Invoice has expired (created at {creation_secs}, expires after {expiry_secs} seconds)"
            );
        }

        Ok(())
    }
}
