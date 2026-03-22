//! NIP-47 protocol request handler

use anyhow::{anyhow, bail, Context, Result};
use nanduti_core::{
    constants::SECONDS_PER_DAY,
    federation::{FederationManager, FederationStatus},
    lightning::LightningOperation,
    models::{
        Bolt11String, Description, PaymentHash, Timestamp, Transaction, TransactionId,
        TransactionState, TransactionType,
    },
    nwc_protocol::{
        ListTransactionsParams, MakeInvoiceParams, NwcErrorCode, NwcMethod, NwcRequestContext,
        NwcResponse, ParsedMethod, PayInvoiceParams, PayKeysendParams,
    },
    storage::Storage,
};
use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, info, warn};
use uuid;

use crate::nostr_client::NostrClient;
use crate::router::FederationRouter;

/// RAII guard that removes a payment hash from the in-flight set on drop.
struct InFlightGuard {
    payment_hash: String,
    in_flight: Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        // Spawn a task to guarantee the in-flight marker is removed even if
        // the mutex is currently held by another task. Using try_lock alone
        // could silently fail, leaving the hash stuck until restart.
        let payment_hash = self.payment_hash.clone();
        let in_flight = Arc::clone(&self.in_flight);
        tokio::spawn(async move {
            in_flight.lock().await.remove(&payment_hash);
        });
    }
}

/// Handles NWC protocol requests
pub struct NwcHandler {
    federation_manager: Arc<FederationManager>,
    router: Arc<FederationRouter>,
    storage: Option<Arc<Storage>>,
    nostr_client: Arc<NostrClient>,
    /// In-flight payment hashes to prevent concurrent duplicate payments.
    /// A payment hash is inserted before the payment call and removed after
    /// settlement or failure. Wrapped in Arc so it can be shared with InFlightGuard.
    in_flight_payments: Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
    /// Serializes the daily-limit check and pending-transaction write for all
    /// payments, preventing two concurrent requests from both passing the quota
    /// check before either's pending transaction is recorded.
    payment_serializer: Arc<tokio::sync::Mutex<()>>,
}

impl NwcHandler {
    /// Create a new NWC handler
    pub fn new(
        federation_manager: Arc<FederationManager>,
        router: Arc<FederationRouter>,
        storage: Option<Arc<Storage>>,
        nostr_client: Arc<NostrClient>,
    ) -> Self {
        Self {
            federation_manager,
            router,
            storage,
            nostr_client,
            in_flight_payments: Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new())),
            payment_serializer: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Validate that the sender has an active connection and is allowed to call the given method.
    ///
    /// Returns the connection if authorized, or an `NwcResponse` error to send back.
    fn authorize_connection(
        &self,
        sender_pubkey: &nanduti_core::models::PublicKey,
        method_name: &str,
    ) -> Result<Result<Option<nanduti_core::storage::NwcConnection>, NwcResponse>> {
        if let Some(storage) = &self.storage {
            let connection = storage
                .get_connection(sender_pubkey)
                .context("Failed to lookup connection")?;

            match connection {
                Some(conn) => {
                    if !conn.allowed_methods.allows(method_name) {
                        warn!(
                            "Connection {} attempted to use restricted method: {method_name}",
                            conn.id
                        );
                        return Ok(Err(NwcResponse::error(
                            method_name.to_string(),
                            NwcErrorCode::Restricted,
                            format!("Method {method_name} is not allowed for this connection"),
                        )));
                    }
                    Ok(Ok(Some(conn)))
                }
                None => {
                    warn!(
                        "Unauthorized {method_name} attempt from unknown pubkey: {sender_pubkey}"
                    );
                    Ok(Err(NwcResponse::error(
                        method_name.to_string(),
                        NwcErrorCode::Unauthorized,
                        "No active connection found for this pubkey".to_string(),
                    )))
                }
            }
        } else {
            Ok(Ok(None))
        }
    }

    /// Handle a NWC request with sender context
    pub async fn handle_request(&self, context: NwcRequestContext) -> Result<NwcResponse> {
        let sender_pubkey = &context.sender_pubkey;
        let method = &context.request.method;
        debug!(
            "Handling NWC request: {} from {sender_pubkey}",
            method.as_str()
        );

        // Enforce connection auth for all known methods before dispatching.
        // Unknown/unimplemented methods are rejected below without needing auth.
        if let ParsedMethod::Known(known_method) = method {
            if !matches!(
                known_method,
                NwcMethod::MultiPayInvoice | NwcMethod::MultiPayKeysend
            ) {
                let method_name = method.as_str();
                match self.authorize_connection(sender_pubkey, method_name)? {
                    Ok(_) => {} // authorized — continue to handler
                    Err(err_response) => return Ok(err_response),
                }
            }
        }

        match method {
            ParsedMethod::Known(NwcMethod::PayInvoice) => {
                self.handle_pay_invoice(context.request.params, sender_pubkey)
                    .await
            }
            ParsedMethod::Known(NwcMethod::MakeInvoice) => {
                self.handle_make_invoice(context.request.params, sender_pubkey)
                    .await
            }
            ParsedMethod::Known(NwcMethod::GetBalance) => {
                self.handle_get_balance(sender_pubkey).await
            }
            ParsedMethod::Known(NwcMethod::ListTransactions) => {
                self.handle_list_transactions(context.request.params, sender_pubkey)
                    .await
            }
            ParsedMethod::Known(NwcMethod::GetInfo) => self.handle_get_info().await,
            ParsedMethod::Known(NwcMethod::PayKeysend) => {
                self.handle_pay_keysend(context.request.params, sender_pubkey)
                    .await
            }
            ParsedMethod::Known(NwcMethod::LookupInvoice) => {
                self.handle_lookup_invoice(context.request.params, sender_pubkey)
                    .await
            }
            ParsedMethod::Known(NwcMethod::MultiPayInvoice | NwcMethod::MultiPayKeysend) => {
                let method_str = method.as_str();
                warn!("Unimplemented method: {method_str}");
                Ok(NwcResponse::error(
                    method_str.to_string(),
                    NwcErrorCode::NotImplemented,
                    format!("Method {method_str} is not yet implemented"),
                ))
            }
            ParsedMethod::Unknown(method_str) => {
                warn!("Unknown method: {method_str}");
                Ok(NwcResponse::error(
                    method_str.clone(),
                    NwcErrorCode::NotImplemented,
                    format!("Unknown method: {method_str}"),
                ))
            }
        }
    }

    /// Handle pay_invoice request
    async fn handle_pay_invoice(
        &self,
        params: Value,
        sender_pubkey: &nanduti_core::models::PublicKey,
    ) -> Result<NwcResponse> {
        let params: PayInvoiceParams =
            serde_json::from_value(params).context("Invalid pay_invoice parameters")?;

        // Parse invoice
        let invoice = LightningOperation::parse_invoice(params.invoice.as_str())?;

        // Validate invoice
        LightningOperation::validate_invoice(&invoice)?;

        // Determine the actual payment amount.
        // For fixed-amount invoices, the BOLT11 amount is authoritative — we must
        // use it for quota checks because the backend pays that amount regardless
        // of any client-supplied override. Accepting a smaller override would let
        // a client bypass per-payment and daily limits.
        // For amountless invoices, the client MUST supply an amount override.
        let amount = if let Some(invoice_amount) = invoice.amount {
            // Fixed-amount invoice: warn if client sent a conflicting override
            if let Some(override_amount) = params.amount {
                if override_amount != invoice_amount {
                    warn!(
                        "Ignoring amount override ({} msats) for fixed-amount invoice ({} msats)",
                        override_amount.as_msats(),
                        invoice_amount.as_msats()
                    );
                }
            }
            invoice_amount
        } else if let Some(override_amount) = params.amount {
            override_amount
        } else {
            bail!("Invoice amount not specified and no amount override provided");
        };

        // Serialize the quota-check → pending-write window so two concurrent
        // payments for different invoices cannot both pass the daily limit.
        let _payment_guard = self.payment_serializer.lock().await;

        // AUTHORIZATION CHECKS
        if let Some(storage) = &self.storage {
            // 1. Look up connection by sender pubkey
            let connection = storage
                .get_connection(sender_pubkey)
                .context("Failed to lookup connection")?;

            let connection = match connection {
                Some(conn) => conn,
                None => {
                    warn!(
                        "Unauthorized payment attempt from unknown pubkey: {}",
                        sender_pubkey
                    );
                    return Ok(NwcResponse::error(
                        "pay_invoice".to_string(),
                        NwcErrorCode::Unauthorized,
                        "No active connection found for this pubkey".to_string(),
                    ));
                }
            };

            // 2. Check if pay_invoice method is allowed
            if !connection.allowed_methods.allows("pay_invoice") {
                warn!(
                    "Connection {} attempted to use restricted method: pay_invoice",
                    connection.id
                );
                return Ok(NwcResponse::error(
                    "pay_invoice".to_string(),
                    NwcErrorCode::Restricted,
                    "Method pay_invoice is not allowed for this connection".to_string(),
                ));
            }

            // 3. Check per-payment limit
            if let Some(per_payment_limit) = connection.per_payment_limit_msats {
                if amount.as_msats() > per_payment_limit {
                    warn!(
                        "Payment of {} msats exceeds per-payment limit of {} msats for connection {}",
                        amount.as_msats(),
                        per_payment_limit,
                        connection.id
                    );
                    return Ok(NwcResponse::error(
                        "pay_invoice".to_string(),
                        NwcErrorCode::QuotaExceeded,
                        format!(
                            "Payment amount {} msats exceeds per-payment limit of {} msats",
                            amount.as_msats(),
                            per_payment_limit
                        ),
                    ));
                }
            }

            // 4. Check daily spending limit
            if let Some(daily_limit) = connection.daily_limit_msats {
                // Get current day timestamp (00:00:00 UTC)
                // SECURITY: We must fail if system clock is broken, not silently use epoch
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .context("System clock error: time is before UNIX epoch")?
                    .as_secs();
                let day_start = (now / SECONDS_PER_DAY) * SECONDS_PER_DAY; // Round down to start of day

                let daily_spent = storage
                    .get_daily_spent(&connection.id, day_start)
                    .context("Failed to get daily spending")?;

                // Reserve a fee margin so routing fees don't push spending
                // over the limit after the payment succeeds.
                // Use max(2%, 10000 msat) to account for both proportional and base fee components.
                let fee_margin = std::cmp::max(amount.as_msats() / 50, 10_000);
                let amount_with_fee_margin = amount.as_msats().saturating_add(fee_margin);
                let total_after_payment = daily_spent.saturating_add(amount_with_fee_margin);

                if total_after_payment > daily_limit {
                    warn!(
                        "Payment would exceed daily limit: spent {} msats, limit {} msats, payment {} msats for connection {}",
                        daily_spent, daily_limit, amount.as_msats(), connection.id
                    );
                    return Ok(NwcResponse::error(
                        "pay_invoice".to_string(),
                        NwcErrorCode::QuotaExceeded,
                        format!(
                            "Payment would exceed daily limit: spent {} msats of {} msats limit",
                            daily_spent, daily_limit
                        ),
                    ));
                }
            }

            // 5. Atomic duplicate-payment detection: check both storage and
            // the in-flight set under a single lock to prevent two concurrent
            // requests for the same BOLT11 from both passing.
            {
                let mut in_flight = self.in_flight_payments.lock().await;
                let ph = invoice.payment_hash.to_string();

                // Check in-flight first (concurrent request already processing)
                if in_flight.contains(&ph) {
                    return Ok(NwcResponse::error(
                        "pay_invoice".to_string(),
                        NwcErrorCode::PaymentInProgress,
                        "Payment already in progress (concurrent request)".to_string(),
                    ));
                }

                // Check storage for settled/pending
                let existing_txs = storage
                    .get_transactions_by_payment_hash(&invoice.payment_hash)
                    .context("Failed to check for duplicate payments")?;

                for tx in existing_txs {
                    if tx.state == TransactionState::Settled {
                        warn!(
                            "Duplicate payment attempt detected for payment_hash {} by connection {}",
                            invoice.payment_hash, connection.id
                        );
                        return Ok(NwcResponse::error(
                            "pay_invoice".to_string(),
                            NwcErrorCode::AlreadyPaid,
                            format!("Invoice already paid (transaction {})", tx.id.as_str()),
                        ));
                    } else if tx.state == TransactionState::Pending {
                        warn!(
                            "Payment already in progress for payment_hash {} (transaction {})",
                            invoice.payment_hash,
                            tx.id.as_str()
                        );
                        return Ok(NwcResponse::error(
                            "pay_invoice".to_string(),
                            NwcErrorCode::PaymentInProgress,
                            format!(
                                "Payment already in progress (transaction {})",
                                tx.id.as_str()
                            ),
                        ));
                    }
                }

                // Mark as in-flight while still holding the lock
                in_flight.insert(ph);
            }
        }

        // Create RAII guard so the in-flight marker is automatically removed on
        // any exit path (success, failure, or early `?` error).
        let _in_flight_guard = InFlightGuard {
            payment_hash: invoice.payment_hash.to_string(),
            in_flight: Arc::clone(&self.in_flight_payments),
        };

        // Load connection once for federation filtering and metadata
        let connection = if let Some(storage) = &self.storage {
            storage
                .get_connection(sender_pubkey)
                .context("Failed to lookup connection")?
        } else {
            None
        };

        // Select federation, filtering to the connection's allowed set so that
        // restricted connections don't get spurious rejections when the router
        // picks a cheaper but unauthorized federation.
        let allowed_filter = connection.as_ref().map(|c| &c.allowed_federations);
        let federation = self
            .router
            .select_federation_filtered(amount, allowed_filter)
            .await?;

        info!(
            "Paying invoice via federation {} for {} msats",
            federation.id,
            amount.as_msats()
        );

        // Store initial transaction before payment
        let uuid = uuid::Uuid::new_v4();
        let transaction_id = TransactionId::new(format!("tx_{uuid}"));
        let created_at = Timestamp::now();

        // Create metadata with connection_id for tracking
        let metadata = connection.as_ref().map(|conn| {
            serde_json::json!({
                "connection_id": conn.id,
                "sender_pubkey": sender_pubkey.as_str()
            })
        });

        if let Some(storage) = &self.storage {
            let transaction = Transaction {
                id: transaction_id.clone(),
                federation_id: federation.id.clone(),
                transaction_type: TransactionType::Outgoing,
                state: TransactionState::Pending,
                invoice: Some(params.invoice.clone()),
                description: invoice.description.clone(),
                preimage: None,
                payment_hash: invoice.payment_hash.clone(),
                amount,
                fees_paid: None,
                created_at,
                settled_at: None,
                metadata: metadata.clone(),
            };
            storage.store_transaction(&transaction)?;
        }

        // Release the serializer now that the pending transaction is written.
        // Other payments can proceed with their quota checks while this one
        // waits for the actual Lightning payment to complete.
        drop(_payment_guard);

        // Execute payment
        let client = federation
            .client
            .as_ref()
            .ok_or_else(|| anyhow!("Federation client not initialized"))?;

        let result = match client.pay_invoice(&invoice, Some(amount)).await {
            Ok(payment_result) => payment_result,
            Err(error) => {
                // Payment failed - update transaction state to Failed
                if let Some(storage) = &self.storage {
                    let failed_transaction = Transaction {
                        id: transaction_id.clone(),
                        federation_id: federation.id.clone(),
                        transaction_type: TransactionType::Outgoing,
                        state: TransactionState::Failed,
                        invoice: Some(params.invoice.clone()),
                        description: invoice.description.clone(),
                        preimage: None,
                        payment_hash: invoice.payment_hash.clone(),
                        amount,
                        fees_paid: None,
                        created_at,
                        settled_at: None,
                        metadata: metadata.clone(),
                    };
                    storage
                        .store_transaction(&failed_transaction)
                        .context("Failed to store failed transaction")?;
                }

                // In-flight marker is cleaned up by _in_flight_guard on return

                // Return payment failed error
                return Ok(NwcResponse::error(
                    "pay_invoice".to_string(),
                    NwcErrorCode::PaymentFailed,
                    format!("Payment failed: {error}"),
                ));
            }
        };

        // Update transaction with settlement details
        if let Some(storage) = &self.storage {
            let transaction = Transaction {
                id: transaction_id,
                federation_id: federation.id.clone(),
                transaction_type: TransactionType::Outgoing,
                state: TransactionState::Settled,
                invoice: Some(params.invoice.clone()),
                description: invoice.description.clone(),
                preimage: Some(result.preimage.clone()),
                payment_hash: result.payment_hash.clone(),
                amount,
                fees_paid: result.fees_paid,
                created_at,
                settled_at: Some(Timestamp::now()),
                metadata: metadata.clone(),
            };
            storage.store_transaction(&transaction)?;

            // Increment connection's spent amount after successful payment
            if let Some(conn) = &connection {
                let total_amount =
                    amount.as_msats() + result.fees_paid.map(|f| f.as_msats()).unwrap_or(0);
                storage
                    .increment_connection_spent(&conn.id, total_amount)
                    .context("Failed to increment connection spent")?;
            }
        }

        // Refresh the federation's cached balance so subsequent routing and
        // get_balance calls reflect the spend immediately.
        if let Err(e) = self.federation_manager.update_balance(&federation.id).await {
            warn!(
                "Failed to refresh balance for federation {} after payment: {e}",
                federation.id
            );
        }

        // In-flight marker is cleaned up by _in_flight_guard when it drops

        Ok(NwcResponse::pay_invoice(result))
    }

    /// Handle make_invoice request
    async fn handle_make_invoice(
        &self,
        params: Value,
        sender_pubkey: &nanduti_core::models::PublicKey,
    ) -> Result<NwcResponse> {
        let params: MakeInvoiceParams =
            serde_json::from_value(params).context("Invalid make_invoice parameters")?;

        // Reject description_hash — Fedimint's create_bolt11_invoice only supports
        // direct descriptions. Silently dropping it would commit to different data
        // than the client requested, breaking hashed-description flows.
        if params.description_hash.is_some() {
            return Ok(NwcResponse::error(
                "make_invoice".to_string(),
                NwcErrorCode::NotImplemented,
                "description_hash is not supported; use description instead".to_string(),
            ));
        }

        let amount = params.amount;
        let description = params
            .description
            .as_ref()
            .map(|d| d.to_string())
            .unwrap_or_else(|| "Payment".to_string());

        // Load connection for federation filtering and metadata
        let connection = if let Some(storage) = &self.storage {
            storage
                .get_connection(sender_pubkey)
                .context("Failed to lookup connection")?
        } else {
            None
        };
        let allowed_filter = connection.as_ref().map(|c| &c.allowed_federations);
        let federation = self
            .router
            .select_federation_for_receive_filtered(allowed_filter)
            .await?;

        let federation_id = &federation.id;
        let amount_msats = amount.as_msats();
        info!("Creating invoice via federation {federation_id} for {amount_msats} msats");

        let client = federation
            .client
            .as_ref()
            .ok_or_else(|| anyhow!("Federation client not initialized"))?;

        let invoice = client
            .make_invoice(amount, description, params.expiry.map(|e| e.as_secs()))
            .await?;

        // Create transaction record with connection metadata and operation_id
        // so the invoice appears in this connection's list_transactions response
        // and the operation_id survives process restarts for re-subscription.
        let metadata = {
            let mut meta = serde_json::json!({});
            if let Some(conn) = connection.as_ref() {
                meta["connection_id"] = serde_json::json!(conn.id);
                meta["sender_pubkey"] = serde_json::json!(sender_pubkey.as_str());
            }
            if let Some(op_id) = &invoice.operation_id {
                meta["operation_id"] = serde_json::json!(op_id);
            }
            Some(meta)
        };

        let transaction = Transaction {
            id: {
                let uuid = uuid::Uuid::new_v4();
                TransactionId::new(format!("tx_{uuid}"))
            },
            federation_id: federation.id.clone(),
            transaction_type: TransactionType::Incoming,
            state: TransactionState::Pending,
            invoice: Some(invoice.bolt11.clone()),
            description: invoice.description.clone(),
            preimage: None,
            payment_hash: invoice.payment_hash.clone(),
            amount,
            fees_paid: None,
            created_at: Timestamp::now(),
            settled_at: None,
            metadata,
        };

        // Store transaction
        if let Some(storage) = &self.storage {
            storage.store_transaction(&transaction)?;
        }

        // Spawn a background task to watch for invoice settlement so the
        // transaction transitions from Pending to Settled when payment arrives.
        if let (Some(op_id), Some(client)) = (&invoice.operation_id, &federation.client) {
            let op_id = op_id.clone();
            let client = client.clone();
            let tx_id = transaction.id.clone();
            let fed_id = federation.id.clone();
            let payment_hash = transaction.payment_hash.clone();
            let storage = self.storage.clone();
            tokio::spawn(async move {
                match client.await_invoice_settlement(&op_id).await {
                    Ok(true) => {
                        info!("Invoice {tx_id} settled on federation {fed_id}");
                        if let Some(storage) = &storage {
                            if let Ok(Some(mut tx)) =
                                storage.get_transaction_by_payment_hash(&payment_hash)
                            {
                                tx.state = TransactionState::Settled;
                                tx.settled_at = Some(Timestamp::now());
                                let _ = storage.store_transaction(&tx);
                            }
                        }
                    }
                    Ok(false) => {
                        warn!("Invoice {tx_id} cancelled on federation {fed_id}");
                        // Mark as Failed so it doesn't stay Pending forever
                        if let Some(storage) = &storage {
                            if let Ok(Some(mut tx)) =
                                storage.get_transaction_by_payment_hash(&payment_hash)
                            {
                                tx.state = TransactionState::Failed;
                                let _ = storage.store_transaction(&tx);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to watch invoice {tx_id} settlement: {e}");
                    }
                }
            });
        }

        Ok(NwcResponse::make_invoice(invoice, transaction))
    }

    /// Handle get_balance request
    async fn handle_get_balance(
        &self,
        sender_pubkey: &nanduti_core::models::PublicKey,
    ) -> Result<NwcResponse> {
        // Load connection to filter balance by allowed federations
        let allowed_filter = if let Some(storage) = &self.storage {
            storage
                .get_connection(sender_pubkey)
                .context("Failed to lookup connection")?
                .map(|c| c.allowed_federations)
        } else {
            None
        };

        // Sum balance only from federations this connection is allowed to use
        let mut total_msats = 0u64;
        for federation in self.federation_manager.list_federations().await {
            let allowed = allowed_filter
                .as_ref()
                .map(|f| f.allows(&federation.id))
                .unwrap_or(true);
            if allowed {
                total_msats = total_msats.saturating_add(federation.balance.as_msats());
            }
        }

        Ok(NwcResponse::get_balance(total_msats))
    }

    /// Handle list_transactions request
    async fn handle_list_transactions(
        &self,
        params: Value,
        sender_pubkey: &nanduti_core::models::PublicKey,
    ) -> Result<NwcResponse> {
        let params: ListTransactionsParams =
            serde_json::from_value(params).context("Invalid list_transactions parameters")?;

        // AUTHORIZATION: Require a valid connection before serving transaction history
        let connection_id = if let Some(storage) = &self.storage {
            let connection = storage
                .get_connection(sender_pubkey)
                .context("Failed to lookup connection")?;

            match connection {
                Some(conn) => {
                    // Check if list_transactions method is allowed
                    if !conn.allowed_methods.allows("list_transactions") {
                        warn!(
                            "Connection {} attempted to use restricted method: list_transactions",
                            conn.id
                        );
                        return Ok(NwcResponse::error(
                            "list_transactions".to_string(),
                            NwcErrorCode::Restricted,
                            "Method list_transactions is not allowed for this connection"
                                .to_string(),
                        ));
                    }
                    Some(conn.id)
                }
                None => {
                    warn!(
                        "Unauthorized list_transactions attempt from unknown pubkey: {}",
                        sender_pubkey
                    );
                    return Ok(NwcResponse::error(
                        "list_transactions".to_string(),
                        NwcErrorCode::Unauthorized,
                        "No active connection found for this pubkey".to_string(),
                    ));
                }
            }
        } else {
            None
        };

        let mut all_transactions = Vec::new();

        // Get all transactions (including from removed federations)
        if let Some(storage) = &self.storage {
            all_transactions = storage.get_all_transactions()?;
        }

        // Filter to only show transactions belonging to this connection
        if let Some(conn_id) = &connection_id {
            all_transactions.retain(|tx| {
                tx.metadata
                    .as_ref()
                    .and_then(|m| m.get("connection_id"))
                    .and_then(|v| v.as_str())
                    == Some(conn_id)
            });
        }

        // Filter by timestamp range (from/until)
        if let Some(from_ts) = &params.from {
            all_transactions.retain(|tx| tx.created_at >= *from_ts);
        }
        if let Some(until_ts) = &params.until {
            all_transactions.retain(|tx| tx.created_at <= *until_ts);
        }

        // Filter by transaction type (incoming/outgoing)
        if let Some(tx_type) = &params.transaction_type {
            all_transactions.retain(|tx| tx.transaction_type == *tx_type);
        }

        // Filter by unpaid status (pending transactions)
        if let Some(true) = params.unpaid {
            all_transactions.retain(|tx| tx.state == TransactionState::Pending);
        }

        // Sort by created_at descending
        all_transactions.sort_by_key(|tx| std::cmp::Reverse(tx.created_at));

        // Apply offset (skip first N transactions)
        if let Some(offset) = params.offset {
            if offset < all_transactions.len() {
                all_transactions = all_transactions.split_off(offset);
            } else {
                all_transactions.clear();
            }
        }

        // Apply limit
        if let Some(limit) = params.limit {
            all_transactions.truncate(limit);
        }

        Ok(NwcResponse::list_transactions(all_transactions))
    }

    /// Handle get_info request
    async fn handle_get_info(&self) -> Result<NwcResponse> {
        use nanduti_core::nwc_protocol::NwcNetwork;

        // Get first online federation for network info
        let federations = self.federation_manager.list_federations().await;
        let online_federation = federations
            .iter()
            .find(|f| f.status == FederationStatus::Online);

        let (network, block_height) = if let Some(federation) = online_federation {
            if let Some(client) = &federation.client {
                let info = client.get_info().await?;
                (NwcNetwork::from_str_loose(&info.network), info.block_height)
            } else {
                (NwcNetwork::Mainnet, 0)
            }
        } else {
            (NwcNetwork::Mainnet, 0)
        };

        // Note: pay_keysend is not advertised because Fedimint doesn't support it yet.
        // See: https://github.com/fedimint/fedimint/issues/XXXX
        let methods = vec![
            NwcMethod::PayInvoice,
            NwcMethod::MakeInvoice,
            NwcMethod::GetBalance,
            NwcMethod::ListTransactions,
            NwcMethod::GetInfo,
            NwcMethod::LookupInvoice,
        ];

        // Don't advertise notifications until we actually emit them.
        // TODO: Enable once invoice settlement watcher sends notifications.
        let notifications = vec![];

        // Use the actual wallet's Nostr public key
        let pubkey = self.nostr_client.public_key();

        Ok(NwcResponse::get_info(
            pubkey,
            network,
            block_height,
            None, // TODO: Fetch actual block hash from federation wallet module
            methods,
            notifications,
        ))
    }

    /// Handle pay_keysend request
    async fn handle_pay_keysend(
        &self,
        params: Value,
        sender_pubkey: &nanduti_core::models::PublicKey,
    ) -> Result<NwcResponse> {
        let params: PayKeysendParams =
            serde_json::from_value(params).context("Invalid pay_keysend parameters")?;

        let amount = params.amount;

        // Serialize the quota-check → pending-write window so two concurrent
        // payments for different invoices cannot both pass the daily limit.
        let _payment_guard = self.payment_serializer.lock().await;

        // AUTHORIZATION CHECKS
        if let Some(storage) = &self.storage {
            // 1. Look up connection by sender pubkey
            let connection = storage
                .get_connection(sender_pubkey)
                .context("Failed to lookup connection")?;

            let connection = match connection {
                Some(conn) => conn,
                None => {
                    warn!(
                        "Unauthorized keysend attempt from unknown pubkey: {}",
                        sender_pubkey
                    );
                    return Ok(NwcResponse::error(
                        "pay_keysend".to_string(),
                        NwcErrorCode::Unauthorized,
                        "No active connection found for this pubkey".to_string(),
                    ));
                }
            };

            // 2. Check if pay_keysend method is allowed
            if !connection.allowed_methods.allows("pay_keysend") {
                warn!(
                    "Connection {} attempted to use restricted method: pay_keysend",
                    connection.id
                );
                return Ok(NwcResponse::error(
                    "pay_keysend".to_string(),
                    NwcErrorCode::Restricted,
                    "Method pay_keysend is not allowed for this connection".to_string(),
                ));
            }

            // 3. Check per-payment limit
            if let Some(per_payment_limit) = connection.per_payment_limit_msats {
                if amount.as_msats() > per_payment_limit {
                    warn!(
                        "Keysend payment of {} msats exceeds per-payment limit of {} msats for connection {}",
                        amount.as_msats(),
                        per_payment_limit,
                        connection.id
                    );
                    return Ok(NwcResponse::error(
                        "pay_keysend".to_string(),
                        NwcErrorCode::QuotaExceeded,
                        format!(
                            "Payment amount {} msats exceeds per-payment limit of {} msats",
                            amount.as_msats(),
                            per_payment_limit
                        ),
                    ));
                }
            }

            // 4. Check daily spending limit
            if let Some(daily_limit) = connection.daily_limit_msats {
                // Get current day timestamp (00:00:00 UTC)
                // SECURITY: We must fail if system clock is broken, not silently use epoch
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .context("System clock error: time is before UNIX epoch")?
                    .as_secs();
                let day_start = (now / SECONDS_PER_DAY) * SECONDS_PER_DAY; // Round down to start of day

                let daily_spent = storage
                    .get_daily_spent(&connection.id, day_start)
                    .context("Failed to get daily spending")?;

                // Reserve a fee margin so routing fees don't push spending
                // over the limit after the payment succeeds.
                // Use max(2%, 10000 msat) to account for both proportional and base fee components.
                let fee_margin = std::cmp::max(amount.as_msats() / 50, 10_000);
                let amount_with_fee_margin = amount.as_msats().saturating_add(fee_margin);
                let total_after_payment = daily_spent.saturating_add(amount_with_fee_margin);

                if total_after_payment > daily_limit {
                    warn!(
                        "Keysend payment would exceed daily limit: spent {} msats, limit {} msats, payment {} msats for connection {}",
                        daily_spent, daily_limit, amount.as_msats(), connection.id
                    );
                    return Ok(NwcResponse::error(
                        "pay_keysend".to_string(),
                        NwcErrorCode::QuotaExceeded,
                        format!(
                            "Payment would exceed daily limit: spent {} msats of {} msats limit",
                            daily_spent, daily_limit
                        ),
                    ));
                }
            }
        }

        // Load connection once for federation filtering and metadata
        let connection = if let Some(storage) = &self.storage {
            storage
                .get_connection(sender_pubkey)
                .context("Failed to lookup connection")?
        } else {
            None
        };

        // Select federation, filtering to the connection's allowed set
        let allowed_filter = connection.as_ref().map(|c| &c.allowed_federations);
        let federation = self
            .router
            .select_federation_filtered(amount, allowed_filter)
            .await?;

        let federation_id = &federation.id;
        let amount_msats = amount.as_msats();
        let pubkey = params.pubkey.as_str();
        info!(
            "Sending keysend via federation {federation_id} for {amount_msats} msats to {pubkey}"
        );

        // Store initial transaction before payment
        let uuid = uuid::Uuid::new_v4();
        let transaction_id = TransactionId::new(format!("tx_{uuid}"));
        let created_at = Timestamp::now();
        let pubkey = params.pubkey.as_str();
        let description = Some(Description::new(format!("Keysend to {pubkey}")));

        // Create metadata with connection_id for tracking
        let metadata = connection.as_ref().map(|conn| {
            serde_json::json!({
                "connection_id": conn.id,
                "sender_pubkey": sender_pubkey.as_str()
            })
        });

        if let Some(storage) = &self.storage {
            let transaction = Transaction {
                id: transaction_id.clone(),
                federation_id: federation.id.clone(),
                transaction_type: TransactionType::Outgoing,
                state: TransactionState::Pending,
                invoice: None,
                description: description.clone(),
                preimage: None,
                payment_hash: PaymentHash::new(String::new()), // Will be updated after payment
                amount,
                fees_paid: None,
                created_at,
                settled_at: None,
                metadata: metadata.clone(),
            };
            storage.store_transaction(&transaction)?;
        }

        // Release the serializer now that the pending transaction is written.
        // Other payments can proceed with their quota checks while this one
        // waits for the actual Lightning payment to complete.
        drop(_payment_guard);

        let client = federation
            .client
            .as_ref()
            .ok_or_else(|| anyhow!("Federation client not initialized"))?;

        let preimage = params
            .preimage
            .map(|p| hex::decode(p.as_str()))
            .transpose()
            .context("Invalid preimage hex")?;

        let result = match client.pay_keysend(&params.pubkey, amount, preimage).await {
            Ok(payment_result) => payment_result,
            Err(error) => {
                // Payment failed - update transaction state to Failed
                if let Some(storage) = &self.storage {
                    let failed_transaction = Transaction {
                        id: transaction_id.clone(),
                        federation_id: federation.id.clone(),
                        transaction_type: TransactionType::Outgoing,
                        state: TransactionState::Failed,
                        invoice: None,
                        description: description.clone(),
                        preimage: None,
                        payment_hash: PaymentHash::new(String::new()),
                        amount,
                        fees_paid: None,
                        created_at,
                        settled_at: None,
                        metadata: metadata.clone(),
                    };
                    storage
                        .store_transaction(&failed_transaction)
                        .context("Failed to store failed transaction")?;
                }

                // Return payment failed error
                return Ok(NwcResponse::error(
                    "pay_keysend".to_string(),
                    NwcErrorCode::PaymentFailed,
                    format!("Keysend payment failed: {error}"),
                ));
            }
        };

        // Update transaction with settlement details
        if let Some(storage) = &self.storage {
            let transaction = Transaction {
                id: transaction_id,
                federation_id: federation.id.clone(),
                transaction_type: TransactionType::Outgoing,
                state: TransactionState::Settled,
                invoice: None,
                description,
                preimage: Some(result.preimage.clone()),
                payment_hash: result.payment_hash.clone(),
                amount,
                fees_paid: result.fees_paid,
                created_at,
                settled_at: Some(Timestamp::now()),
                metadata: metadata.clone(),
            };
            storage.store_transaction(&transaction)?;

            // Increment connection's spent amount after successful payment
            if let Some(conn) = &connection {
                let total_amount =
                    amount.as_msats() + result.fees_paid.map(|f| f.as_msats()).unwrap_or(0);
                storage
                    .increment_connection_spent(&conn.id, total_amount)
                    .context("Failed to increment connection spent")?;
            }
        }

        // Refresh the federation's cached balance after keysend payment
        if let Err(e) = self.federation_manager.update_balance(&federation.id).await {
            warn!(
                "Failed to refresh balance for federation {} after keysend: {e}",
                federation.id
            );
        }

        Ok(NwcResponse::pay_invoice(result))
    }

    /// Handle lookup_invoice request
    async fn handle_lookup_invoice(
        &self,
        params: Value,
        sender_pubkey: &nanduti_core::models::PublicKey,
    ) -> Result<NwcResponse> {
        // Parse payment hash or invoice from params
        let payment_hash = params
            .get("payment_hash")
            .and_then(|v| v.as_str())
            .map(String::from);

        let invoice = params
            .get("invoice")
            .and_then(|v| v.as_str())
            .map(String::from);

        // Load the requesting connection's ID for scoping
        let connection_id = if let Some(storage) = &self.storage {
            storage
                .get_connection(sender_pubkey)
                .context("Failed to lookup connection")?
                .map(|c| c.id)
        } else {
            None
        };

        // Look up transactions by payment hash or invoice, then filter to this
        // connection BEFORE selecting a single result. This prevents returning
        // NOT_FOUND when the newest match belongs to a different connection but
        // older matches belong to the caller.
        let candidates: Vec<Transaction> = if let Some(hash) = payment_hash {
            if let Some(storage) = &self.storage {
                storage
                    .get_transactions_by_payment_hash(&PaymentHash::new(hash))
                    .map_err(|error| anyhow::anyhow!("Failed to lookup transaction: {error}"))?
            } else {
                vec![]
            }
        } else if let Some(inv) = invoice {
            if let Some(storage) = &self.storage {
                storage
                    .get_transactions_by_invoice(&Bolt11String::new(inv))
                    .map_err(|error| anyhow::anyhow!("Failed to lookup transaction: {error}"))?
            } else {
                vec![]
            }
        } else {
            return Ok(NwcResponse::error(
                "lookup_invoice".to_string(),
                NwcErrorCode::BadRequest,
                "Missing payment_hash or invoice parameter".to_string(),
            ));
        };

        // Scope to the requesting connection before picking the best match.
        // When a connection_id is known, only return transactions that explicitly
        // belong to this connection. Transactions with no metadata (e.g. REST-created)
        // are excluded to prevent cross-channel data leakage.
        let transaction = candidates
            .into_iter()
            .find(|tx| match (&connection_id, &tx.metadata) {
                (Some(conn_id), Some(meta)) => {
                    meta.get("connection_id").and_then(|v| v.as_str()) == Some(conn_id)
                }
                // Connection is known but tx has no metadata — deny (REST-created tx)
                (Some(_), None) => false,
                // No connection tracking (no storage) — allow
                (None, _) => true,
            });

        // Check if transaction was found
        if let Some(tx) = transaction {
            // Build response based on transaction state
            let settled = matches!(tx.state, TransactionState::Settled);
            // NIP-47 uses millisatoshis for all amount fields
            let response = serde_json::json!({
                "invoice": tx.invoice.as_ref().map(|i| i.to_string()),
                "amount": tx.amount.as_msats(),
                "payment_hash": tx.payment_hash.to_string(),
                "preimage": tx.preimage.as_ref().map(|p| p.to_string()),
                "settled_at": tx.settled_at.map(|t| t.as_secs()),
                "created_at": tx.created_at.as_secs(),
                "description": tx.description.as_ref().map(|d| d.to_string()),
                "fees_paid": tx.fees_paid.map(|f| f.as_msats()),
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
