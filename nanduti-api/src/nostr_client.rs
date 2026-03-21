//! Nostr relay connection and message handling

use anyhow::{Context, Result};
use nanduti_core::models::{Amount, Bolt11String, PaymentHash, PaymentType, Preimage};
use nostr_sdk::prelude::*;
use std::sync::Arc;
use tracing::{debug, info};

/// Manages connection to Nostr relays
pub struct NostrClient {
    client: Client,
    keys: Keys,
}

impl NostrClient {
    /// Create a new Nostr client
    pub async fn new(relays: Vec<String>, secret_key: Option<String>) -> Result<Self> {
        // Generate or use provided keys
        let keys = if let Some(secret) = secret_key {
            Keys::parse(&secret)?
        } else {
            Keys::generate()
        };

        let client = Client::new(keys.clone());

        // Add relays
        for relay_url in relays {
            client
                .add_relay(&relay_url)
                .await
                .with_context(|| format!("Failed to add relay {relay_url}"))?;
        }

        // Connect to relays
        client.connect().await;

        let relay_count = client.relays().await.len();
        info!("Connected to {relay_count} relays");

        Ok(Self { client, keys })
    }

    /// Get the client's public key
    pub fn public_key(&self) -> String {
        self.keys.public_key().to_hex()
    }

    /// Subscribe to NWC requests
    pub async fn subscribe_nwc_requests(&self) -> Result<()> {
        // Subscribe to kind 23194 events sent to us
        let filter = Filter::new()
            .kind(Kind::from(23194))
            .pubkey(self.keys.public_key());

        self.client.subscribe(filter, None).await?;

        debug!("Subscribed to NWC requests");
        Ok(())
    }

    /// Send NWC response
    pub async fn send_nwc_response(
        &self,
        request_id: String,
        recipient_pubkey: String,
        encrypted_content: String,
    ) -> Result<()> {
        let recipient = PublicKey::from_hex(&recipient_pubkey)?;

        // Create response event (kind 23195)
        let event_builder = EventBuilder::new(Kind::from(23195), encrypted_content).tags(vec![
            Tag::public_key(recipient),
            Tag::event(EventId::from_hex(&request_id)?),
        ]);

        let event = self.client.sign_event_builder(event_builder).await?;

        self.client.send_event(&event).await?;

        debug!("Sent NWC response to {recipient_pubkey}");
        Ok(())
    }

    /// Send info event with default capabilities
    pub async fn publish_info_event(&self) -> Result<()> {
        use nanduti_core::nwc_protocol::{NwcMethod, NwcNotificationType};

        // Note: pay_keysend is not advertised because Fedimint doesn't support it yet
        let capabilities = [
            NwcMethod::PayInvoice.to_string(),
            NwcMethod::MakeInvoice.to_string(),
            NwcMethod::GetBalance.to_string(),
            NwcMethod::ListTransactions.to_string(),
            NwcMethod::GetInfo.to_string(),
            NwcMethod::LookupInvoice.to_string(),
        ];

        let notifications = vec![
            NwcNotificationType::PaymentReceived.to_string(),
            NwcNotificationType::PaymentSent.to_string(),
        ];

        let content = capabilities.join(" ");

        let event_builder = EventBuilder::new(Kind::from(13194), content).tags(vec![
            // Only advertise nip44_v2 — we only implement nip44 decryption,
            // so advertising nip04 would cause clients to send messages we can't read.
            Tag::custom(
                TagKind::Custom("encryption".into()),
                vec!["nip44_v2".to_string()],
            ),
            Tag::custom(TagKind::Custom("notifications".into()), notifications),
        ]);

        let event = self.client.sign_event_builder(event_builder).await?;

        self.client.send_event(&event).await?;

        info!("Published NWC info event");
        Ok(())
    }

    /// Send notification event
    pub async fn send_notification(
        &self,
        recipient_pubkey: &PublicKey,
        notification_type: nanduti_core::nwc_protocol::NwcNotificationType,
        notification_data: nanduti_core::nwc_protocol::NotificationData,
    ) -> Result<()> {
        use crate::encryption;

        // Create notification payload
        let notification = nanduti_core::nwc_protocol::NostrNotification {
            notification_type,
            notification: notification_data,
        };

        let content = serde_json::to_string(&notification)?;

        // Encrypt with NIP-44 (modern encryption)
        // Send NIP-44 version (kind 23197)
        let encrypted_nip44 = encryption::encrypt_nip44(&content, recipient_pubkey, &self.keys)?;
        let event_builder_nip44 = EventBuilder::new(Kind::from(23197), encrypted_nip44)
            .tag(Tag::public_key(*recipient_pubkey));
        let event_nip44 = self.client.sign_event_builder(event_builder_nip44).await?;

        self.client.send_event(&event_nip44).await?;

        debug!(
            "Sent {} notification to {}",
            notification_type,
            recipient_pubkey.to_hex()
        );
        Ok(())
    }

    /// Send payment received notification
    pub async fn notify_payment_received(
        &self,
        recipient_pubkey: &PublicKey,
        invoice: &Bolt11String,
        payment_hash: &PaymentHash,
        amount: Amount,
        preimage: &Preimage,
    ) -> Result<()> {
        use nanduti_core::models::{Timestamp, TransactionState};
        use nanduti_core::nwc_protocol::{
            NotificationData, NwcNotificationType, PaymentReceivedNotification,
        };

        let notification_data = NotificationData::PaymentReceived(PaymentReceivedNotification {
            payment_type: PaymentType::Incoming,
            state: TransactionState::Settled,
            invoice: invoice.clone(),
            payment_hash: payment_hash.clone(),
            preimage: preimage.clone(),
            amount,
            settled_at: Timestamp::now(),
        });

        self.send_notification(
            recipient_pubkey,
            NwcNotificationType::PaymentReceived,
            notification_data,
        )
        .await
    }

    /// Send payment sent notification
    pub async fn notify_payment_sent(
        &self,
        recipient_pubkey: &PublicKey,
        invoice: &Bolt11String,
        payment_hash: &PaymentHash,
        amount: Amount,
        fees_paid: Option<Amount>,
        preimage: &Preimage,
    ) -> Result<()> {
        use nanduti_core::models::{Timestamp, TransactionState};
        use nanduti_core::nwc_protocol::{
            NotificationData, NwcNotificationType, PaymentSentNotification,
        };

        let notification_data = NotificationData::PaymentSent(PaymentSentNotification {
            payment_type: PaymentType::Outgoing,
            state: TransactionState::Settled,
            invoice: invoice.clone(),
            payment_hash: payment_hash.clone(),
            preimage: preimage.clone(),
            amount,
            fees_paid,
            settled_at: Timestamp::now(),
        });

        self.send_notification(
            recipient_pubkey,
            NwcNotificationType::PaymentSent,
            notification_data,
        )
        .await
    }

    /// Handle incoming NWC events
    pub async fn handle_nwc_events(&self, handler: Arc<crate::NwcHandler>) -> Result<()> {
        use lru::LruCache;
        use std::num::NonZeroUsize;

        // Track processed events to avoid duplicates using LRU cache
        // Capacity of 10,000 provides good memory bounds while preventing duplicate processing
        const EVENT_CACHE_CAPACITY: NonZeroUsize = NonZeroUsize::new(10000).unwrap();
        let mut processed_events = LruCache::new(EVENT_CACHE_CAPACITY);

        // Subscribe to NWC request events (kind 23194) sent to us.
        // Use a `since` filter set to the current time so we only process events
        // that arrive after the server starts, preventing replay of historical
        // requests (which could re-trigger payments after a restart).
        let now = Timestamp::now();
        let base_filter = Filter::new()
            .kind(Kind::from(23194))
            .pubkey(self.keys.public_key());
        let filter = base_filter.clone().since(now);

        info!(
            "Listening for NWC requests on {relay_count} relays (since {now})",
            relay_count = self.client.relays().await.len()
        );

        // Use proper event streaming (subscribe and poll)
        self.client.subscribe(filter.clone(), None).await?;

        // Track the latest event timestamp we've seen so we can advance the
        // `since` window and avoid re-scanning already-processed events.
        let mut latest_seen = now;

        // Circuit breaker for error handling
        let mut consecutive_errors = 0;
        const MAX_CONSECUTIVE_ERRORS: usize = 10;
        const BASE_BACKOFF_MS: u64 = 500;
        const MAX_BACKOFF_MS: u64 = 30_000; // Cap at 30 seconds for payment systems

        // Poll for events continuously
        loop {
            // Query only for events newer than the last one we processed
            let poll_filter = base_filter.clone().since(latest_seen);
            match self.client.database().query(poll_filter).await {
                Ok(events) => {
                    consecutive_errors = 0; // Reset error counter on success

                    for event in events {
                        // Skip if we've already processed this event
                        if processed_events.contains(&event.id) {
                            continue;
                        }

                        // Advance the since window so future queries skip older events
                        if event.created_at > latest_seen {
                            latest_seen = event.created_at;
                        }

                        // Mark as processed (LRU cache automatically evicts oldest when full)
                        processed_events.put(event.id, ());

                        // Handle the event
                        if let Err(e) = self.handle_single_event(event, handler.clone()).await {
                            tracing::error!("Error handling event: {e}");
                        }
                    }
                }
                Err(e) => {
                    consecutive_errors += 1;
                    tracing::error!(
                        "Error querying events (attempt {consecutive_errors}/{MAX_CONSECUTIVE_ERRORS}): {e}"
                    );

                    if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                        anyhow::bail!(
                            "Too many consecutive errors in event handler ({MAX_CONSECUTIVE_ERRORS}), shutting down"
                        );
                    }

                    // Exponential backoff with proper cap
                    // Formula: BASE * 2^(attempts - 1), capped at MAX
                    // Error 1: 500ms, Error 2: 1s, Error 3: 2s, Error 4: 4s, Error 5: 8s, Error 6: 16s, Error 7+: 30s
                    // Note: consecutive_errors is guaranteed to be >= 1 here (incremented on line 273)
                    let backoff_ms = (BASE_BACKOFF_MS * 2_u64.pow((consecutive_errors - 1) as u32))
                        .min(MAX_BACKOFF_MS);

                    tracing::warn!(
                        "Backing off for {backoff_ms}ms before retry (consecutive errors: {consecutive_errors})"
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms)).await;
                    continue;
                }
            }

            // Small delay before next poll
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
    }

    /// Handle a single NWC event
    async fn handle_single_event(
        &self,
        event: Event,
        handler: Arc<crate::NwcHandler>,
    ) -> Result<()> {
        use crate::encryption;
        use nanduti_core::models::PublicKey;
        use nanduti_core::nwc_protocol::{NwcRequest, NwcRequestContext};

        let pubkey = &event.pubkey;
        debug!("Processing event from {pubkey}");

        // Decrypt the request (NIP-44)
        let decrypted_content =
            encryption::decrypt_nip44(&event.content, &event.pubkey, &self.keys)?;

        // Parse the request
        let request: NwcRequest =
            serde_json::from_str(&decrypted_content).context("Failed to parse NWC request")?;

        let method = &request.method;
        info!("Received NWC request: {method}");

        // Create request context with sender pubkey for authorization
        let context = NwcRequestContext {
            request,
            sender_pubkey: PublicKey::new(event.pubkey.to_hex()),
            event_id: event.id.to_hex(),
        };

        // Handle the request
        let response = handler.handle_request(context).await?;

        // Serialize response
        let response_content = serde_json::to_string(&response)?;

        // Encrypt the response (NIP-44)
        let encrypted_response =
            encryption::encrypt_nip44(&response_content, &event.pubkey, &self.keys)?;

        // Send response
        self.send_nwc_response(event.id.to_hex(), event.pubkey.to_hex(), encrypted_response)
            .await?;

        Ok(())
    }
}
