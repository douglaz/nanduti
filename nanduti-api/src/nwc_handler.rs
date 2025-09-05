//! NIP-47 protocol request handler

use anyhow::{anyhow, bail, Context, Result};
use nanduti_core::{
    federation::{FederationManager, FederationStatus},
    lightning::LightningOperation,
    models::{
        Description, Timestamp, Transaction, TransactionId, TransactionState, TransactionType,
    },
    nwc_protocol::{
        ListTransactionsParams, MakeInvoiceParams, NwcErrorCode, NwcMethod, NwcRequest,
        NwcResponse, PayInvoiceParams, PayKeysendParams,
    },
    storage::Storage,
};
use serde_json::Value;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::router::FederationRouter;

/// Handles NWC protocol requests
pub struct NwcHandler {
    federation_manager: Arc<FederationManager>,
    router: Arc<FederationRouter>,
    storage: Option<Arc<Storage>>,
}

impl NwcHandler {
    /// Create a new NWC handler
    pub fn new(
        federation_manager: Arc<FederationManager>,
        router: Arc<FederationRouter>,
        storage: Option<Arc<Storage>>,
    ) -> Self {
        Self {
            federation_manager,
            router,
            storage,
        }
    }

    /// Handle a NWC request
    pub async fn handle_request(&self, request: NwcRequest) -> Result<NwcResponse> {
        let method_str = &request.method;
        debug!("Handling NWC request: {method_str}");

        // Parse the method string into enum using FromStr trait
        let method = NwcMethod::from_str(method_str);

        match method {
            Ok(NwcMethod::PayInvoice) => self.handle_pay_invoice(request.params).await,
            Ok(NwcMethod::MakeInvoice) => self.handle_make_invoice(request.params).await,
            Ok(NwcMethod::GetBalance) => self.handle_get_balance().await,
            Ok(NwcMethod::ListTransactions) => self.handle_list_transactions(request.params).await,
            Ok(NwcMethod::GetInfo) => self.handle_get_info().await,
            Ok(NwcMethod::PayKeysend) => self.handle_pay_keysend(request.params).await,
            Ok(NwcMethod::LookupInvoice) => self.handle_lookup_invoice(request.params).await,
            Ok(NwcMethod::MultiPayInvoice) | Ok(NwcMethod::MultiPayKeysend) => {
                warn!("Unimplemented method: {method_str}");
                Ok(NwcResponse::error(
                    method_str.to_string(),
                    NwcErrorCode::NotImplemented,
                    format!("Method {method_str} is not yet implemented"),
                ))
            }
            Err(_) => {
                warn!("Unknown method: {method_str}");
                Ok(NwcResponse::error(
                    method_str.to_string(),
                    NwcErrorCode::NotImplemented,
                    format!("Unknown method: {method_str}"),
                ))
            }
        }
    }

    /// Handle pay_invoice request
    async fn handle_pay_invoice(&self, params: Value) -> Result<NwcResponse> {
        let params: PayInvoiceParams =
            serde_json::from_value(params).context("Invalid pay_invoice parameters")?;

        // Parse invoice
        let invoice = LightningOperation::parse_invoice(&params.invoice.0)?;

        // Validate invoice
        LightningOperation::validate_invoice(&params.invoice.0)?;

        // Determine amount
        let amount = if let Some(override_amount) = params.amount {
            override_amount
        } else if let Some(invoice_amount) = invoice.amount {
            invoice_amount
        } else {
            bail!("Invoice amount not specified");
        };

        // Select federation
        let federation = self.router.select_federation(amount).await?;

        info!(
            "Paying invoice via federation {} for {} msats",
            federation.id,
            amount.as_msats()
        );

        // Execute payment
        let client = federation
            .client
            .as_ref()
            .ok_or_else(|| anyhow!("Federation client not initialized"))?;

        let result = client.pay_invoice(&invoice).await?;

        // Store transaction
        if let Some(storage) = &self.storage {
            let transaction = Transaction {
                id: TransactionId(format!("tx_{}", uuid::Uuid::new_v4())),
                federation_id: federation.id.clone(),
                transaction_type: TransactionType::Outgoing,
                state: TransactionState::Settled,
                invoice: Some(params.invoice.clone()),
                description: invoice.description.clone(),
                preimage: Some(result.preimage.clone()),
                payment_hash: result.payment_hash.clone(),
                amount,
                fees_paid: result.fees_paid,
                created_at: Timestamp(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_secs(),
                ),
                settled_at: Some(Timestamp(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_secs(),
                )),
                metadata: None,
            };
            storage.store_transaction(&transaction)?;
        }

        Ok(NwcResponse::pay_invoice(result))
    }

    /// Handle make_invoice request
    async fn handle_make_invoice(&self, params: Value) -> Result<NwcResponse> {
        let params: MakeInvoiceParams =
            serde_json::from_value(params).context("Invalid make_invoice parameters")?;

        let amount = params.amount;
        let description = params
            .description
            .as_ref()
            .map(|d| d.0.clone())
            .unwrap_or_else(|| "Payment".to_string());

        // Select a federation (round-robin or least loaded)
        let federation = self.router.select_federation_for_receive().await?;

        info!(
            "Creating invoice via federation {} for {} msats",
            federation.id,
            amount.as_msats()
        );

        let client = federation
            .client
            .as_ref()
            .ok_or_else(|| anyhow!("Federation client not initialized"))?;

        let invoice = client
            .make_invoice(amount, description, params.expiry.map(|e| e.0))
            .await?;

        // Create transaction record
        let transaction = Transaction {
            id: TransactionId(format!("tx_{}", uuid::Uuid::new_v4())),
            federation_id: federation.id.clone(),
            transaction_type: TransactionType::Incoming,
            state: TransactionState::Pending,
            invoice: Some(invoice.bolt11.clone()),
            description: invoice.description.clone(),
            preimage: None,
            payment_hash: invoice.payment_hash.clone(),
            amount,
            fees_paid: None,
            created_at: Timestamp(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs(),
            ),
            settled_at: None,
            metadata: None,
        };

        // Store transaction
        if let Some(storage) = &self.storage {
            storage.store_transaction(&transaction)?;
        }

        Ok(NwcResponse::make_invoice(invoice, transaction))
    }

    /// Handle get_balance request
    async fn handle_get_balance(&self) -> Result<NwcResponse> {
        let balance = self.federation_manager.get_total_balance().await;
        Ok(NwcResponse::get_balance(balance.as_msats()))
    }

    /// Handle list_transactions request
    async fn handle_list_transactions(&self, params: Value) -> Result<NwcResponse> {
        let params: ListTransactionsParams =
            serde_json::from_value(params).context("Invalid list_transactions parameters")?;

        let mut all_transactions = Vec::new();

        // Get transactions from all federations
        if let Some(storage) = &self.storage {
            for federation in self.federation_manager.list_federations().await {
                let transactions =
                    storage.get_federation_transactions(&federation.id, params.limit)?;
                all_transactions.extend(transactions);
            }
        }

        // Sort by created_at descending
        all_transactions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        // Apply limit
        if let Some(limit) = params.limit {
            all_transactions.truncate(limit);
        }

        Ok(NwcResponse::list_transactions(all_transactions))
    }

    /// Handle get_info request
    async fn handle_get_info(&self) -> Result<NwcResponse> {
        // Get first online federation for network info
        let federations = self.federation_manager.list_federations().await;
        let online_federation = federations
            .iter()
            .find(|f| f.status == FederationStatus::Online);

        let (network, block_height) = if let Some(federation) = online_federation {
            if let Some(client) = &federation.client {
                let info = client.get_info().await?;
                (info.network, info.block_height)
            } else {
                ("bitcoin".to_string(), 0)
            }
        } else {
            ("bitcoin".to_string(), 0)
        };

        let methods = vec![
            "pay_invoice".to_string(),
            "make_invoice".to_string(),
            "get_balance".to_string(),
            "list_transactions".to_string(),
            "get_info".to_string(),
            "pay_keysend".to_string(),
            "lookup_invoice".to_string(),
        ];

        let notifications = vec!["payment_received".to_string(), "payment_sent".to_string()];

        // Generate a deterministic pubkey for this instance
        let pubkey =
            "02fedimint0000000000000000000000000000000000000000000000000000000".to_string();

        Ok(NwcResponse::get_info(
            pubkey,
            network,
            block_height,
            methods,
            notifications,
        ))
    }

    /// Handle pay_keysend request
    async fn handle_pay_keysend(&self, params: Value) -> Result<NwcResponse> {
        let params: PayKeysendParams =
            serde_json::from_value(params).context("Invalid pay_keysend parameters")?;

        let amount = params.amount;

        // Select federation
        let federation = self.router.select_federation(amount).await?;

        info!(
            "Sending keysend via federation {} for {} msats to {}",
            federation.id,
            amount.as_msats(),
            params.pubkey.0
        );

        let client = federation
            .client
            .as_ref()
            .ok_or_else(|| anyhow!("Federation client not initialized"))?;

        let preimage = params
            .preimage
            .map(|p| hex::decode(p.0))
            .transpose()
            .context("Invalid preimage hex")?;

        let result = client.pay_keysend(&params.pubkey, amount, preimage).await?;

        // Store transaction
        if let Some(storage) = &self.storage {
            let transaction = Transaction {
                id: TransactionId(format!("tx_{}", uuid::Uuid::new_v4())),
                federation_id: federation.id.clone(),
                transaction_type: TransactionType::Outgoing,
                state: TransactionState::Settled,
                invoice: None,
                description: Some(Description(format!("Keysend to {}", params.pubkey.0))),
                preimage: Some(result.preimage.clone()),
                payment_hash: result.payment_hash.clone(),
                amount,
                fees_paid: result.fees_paid,
                created_at: Timestamp(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_secs(),
                ),
                settled_at: Some(Timestamp(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_secs(),
                )),
                metadata: None,
            };
            storage.store_transaction(&transaction)?;
        }

        Ok(NwcResponse::pay_invoice(result))
    }

    /// Handle lookup_invoice request
    async fn handle_lookup_invoice(&self, params: Value) -> Result<NwcResponse> {
        // Parse payment hash or invoice from params
        let payment_hash = params
            .get("payment_hash")
            .and_then(|v| v.as_str())
            .map(String::from);

        let invoice = params
            .get("invoice")
            .and_then(|v| v.as_str())
            .map(String::from);

        // Look up transaction by payment hash or invoice
        let transaction = if let Some(hash) = payment_hash {
            if let Some(storage) = &self.storage {
                storage
                    .get_transaction_by_payment_hash(&hash)
                    .map_err(|e| anyhow::anyhow!("Failed to lookup transaction: {}", e))?
            } else {
                None
            }
        } else if let Some(inv) = invoice {
            if let Some(storage) = &self.storage {
                storage
                    .get_transaction_by_invoice(&inv)
                    .map_err(|e| anyhow::anyhow!("Failed to lookup transaction: {}", e))?
            } else {
                None
            }
        } else {
            return Ok(NwcResponse::error(
                "lookup_invoice".to_string(),
                NwcErrorCode::BadRequest,
                "Missing payment_hash or invoice parameter".to_string(),
            ));
        };

        // Check if transaction was found
        if let Some(tx) = transaction {
            // Build response based on transaction state
            let settled = matches!(tx.state, TransactionState::Settled);
            let response = serde_json::json!({
                "invoice": tx.invoice.as_ref().map(|i| i.0.clone()),
                "amount": tx.amount.as_msats() / 1000, // Convert to sats for NWC
                "payment_hash": tx.payment_hash.0,
                "preimage": tx.preimage.as_ref().map(|p| p.0.clone()),
                "settled_at": tx.settled_at.map(|t| t.0),
                "created_at": tx.created_at.0,
                "description": tx.description.as_ref().map(|d| d.0.clone()),
                "fees_paid": tx.fees_paid.map(|f| f.as_msats() / 1000),
                "settled": settled,
            });

            Ok(NwcResponse::lookup_invoice(response))
        } else {
            Ok(NwcResponse::error(
                "lookup_invoice".to_string(),
                NwcErrorCode::NotFound,
                "Invoice not found".to_string(),
            ))
        }
    }
}

// Add uuid dependency for transaction IDs
use uuid;
