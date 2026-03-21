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
        ListTransactionsParams, MakeInvoiceParams, NwcErrorCode, NwcMethod, NwcNotificationType,
        NwcRequestContext, NwcResponse, ParsedMethod, PayInvoiceParams, PayKeysendParams,
    },
    storage::Storage,
};
use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, info, warn};
use uuid;

use crate::nostr_client::NostrClient;
use crate::router::FederationRouter;

/// Handles NWC protocol requests
pub struct NwcHandler {
    federation_manager: Arc<FederationManager>,
    router: Arc<FederationRouter>,
    storage: Option<Arc<Storage>>,
    nostr_client: Arc<NostrClient>,
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
            ParsedMethod::Known(NwcMethod::GetBalance) => self.handle_get_balance().await,
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
                self.handle_lookup_invoice(context.request.params).await
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

        // Determine amount
        let amount = if let Some(override_amount) = params.amount {
            override_amount
        } else if let Some(invoice_amount) = invoice.amount {
            invoice_amount
        } else {
            bail!("Invoice amount not specified");
        };

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

                let total_after_payment = daily_spent.saturating_add(amount.as_msats());

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

            // 5. Check for duplicate payment (same payment hash already settled)
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
        }

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

        // Execute payment
        let client = federation
            .client
            .as_ref()
            .ok_or_else(|| anyhow!("Federation client not initialized"))?;

        let result = match client.pay_invoice(&invoice).await {
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

        let amount = params.amount;
        let description = params
            .description
            .as_ref()
            .map(|d| d.to_string())
            .unwrap_or_else(|| "Payment".to_string());

        // Select a federation, filtering to the connection's allowed set
        let allowed_filter = if let Some(storage) = &self.storage {
            storage
                .get_connection(sender_pubkey)
                .context("Failed to lookup connection")?
                .map(|c| c.allowed_federations)
        } else {
            None
        };
        let federation = self
            .router
            .select_federation_for_receive_filtered(allowed_filter.as_ref())
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

        // Create transaction record
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

        // Get transactions from all federations (get more than limit to allow for filtering)
        if let Some(storage) = &self.storage {
            for federation in self.federation_manager.list_federations().await {
                let transactions = storage.get_federation_transactions(&federation.id, None)?;
                all_transactions.extend(transactions);
            }
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

        let notifications = vec![
            NwcNotificationType::PaymentReceived,
            NwcNotificationType::PaymentSent,
        ];

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

                let total_after_payment = daily_spent.saturating_add(amount.as_msats());

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
                    .get_transaction_by_payment_hash(&PaymentHash::new(hash))
                    .map_err(|error| anyhow::anyhow!("Failed to lookup transaction: {error}"))?
            } else {
                None
            }
        } else if let Some(inv) = invoice {
            if let Some(storage) = &self.storage {
                storage
                    .get_transaction_by_invoice(&Bolt11String::new(inv))
                    .map_err(|error| anyhow::anyhow!("Failed to lookup transaction: {error}"))?
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
                "invoice": tx.invoice.as_ref().map(|i| i.to_string()),
                "amount": tx.amount.as_msats() / 1000, // Convert to sats for NWC
                "payment_hash": tx.payment_hash.to_string(),
                "preimage": tx.preimage.as_ref().map(|p| p.to_string()),
                "settled_at": tx.settled_at.map(|t| t.as_secs()),
                "created_at": tx.created_at.as_secs(),
                "description": tx.description.as_ref().map(|d| d.to_string()),
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
