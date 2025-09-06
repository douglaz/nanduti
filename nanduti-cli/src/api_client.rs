//! REST API client for the nanduti server

use anyhow::{anyhow, Result};
use nanduti_core::models::*;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// REST API client for communicating with the nanduti server
pub struct ApiClient {
    client: Client,
    base_url: String,
}

impl ApiClient {
    /// Create a new API client
    pub fn new(base_url: String) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self { client, base_url })
    }

    /// Check server health
    #[allow(dead_code)]
    pub async fn health(&self) -> Result<()> {
        let response = self
            .client
            .get(format!("{base_url}/health", base_url = self.base_url))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("Server health check failed"));
        }

        Ok(())
    }

    // Federation management

    /// Add a new federation
    pub async fn add_federation(
        &self,
        invite_code: fedimint_core::invite_code::InviteCode,
    ) -> Result<AddFederationResponse> {
        let request = AddFederationRequest { invite_code };

        let response = self
            .client
            .post(format!(
                "{base_url}/api/v1/federations",
                base_url = self.base_url
            ))
            .json(&request)
            .send()
            .await?;

        handle_response(response).await
    }

    /// List all federations
    pub async fn list_federations(&self) -> Result<Vec<FederationInfo>> {
        let response = self
            .client
            .get(format!(
                "{base_url}/api/v1/federations",
                base_url = self.base_url
            ))
            .send()
            .await?;

        handle_response(response).await
    }

    /// Get federation details
    pub async fn get_federation(&self, id: &FederationId) -> Result<FederationInfo> {
        let response = self
            .client
            .get(format!(
                "{base_url}/api/v1/federations/{id}",
                base_url = self.base_url,
                id = id.as_str()
            ))
            .send()
            .await?;

        handle_response(response).await
    }

    /// Remove a federation
    pub async fn remove_federation(&self, id: &FederationId) -> Result<()> {
        let response = self
            .client
            .delete(format!(
                "{base_url}/api/v1/federations/{id}",
                base_url = self.base_url,
                id = id.as_str()
            ))
            .send()
            .await?;

        if !response.status().is_success() {
            let error = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow!("Failed to remove federation: {error}"));
        }

        Ok(())
    }

    /// Get federation balance
    #[allow(dead_code)]
    pub async fn get_federation_balance(&self, id: &FederationId) -> Result<Value> {
        let response = self
            .client
            .get(format!(
                "{base_url}/api/v1/federations/{id}/balance",
                base_url = self.base_url,
                id = id.as_str()
            ))
            .send()
            .await?;

        handle_response(response).await
    }

    /// List federation gateways
    pub async fn list_federation_gateways(&self, id: &FederationId) -> Result<Vec<GatewayInfo>> {
        let response = self
            .client
            .get(format!(
                "{base_url}/api/v1/federations/{id}/gateways",
                base_url = self.base_url,
                id = id.as_str()
            ))
            .send()
            .await?;

        handle_response(response).await
    }

    // Invoice management

    /// Create a new invoice
    pub async fn create_invoice(
        &self,
        request: CreateInvoiceRequest,
    ) -> Result<CreateInvoiceResponse> {
        let response = self
            .client
            .post(format!(
                "{base_url}/api/v1/invoices",
                base_url = self.base_url
            ))
            .json(&request)
            .send()
            .await?;

        handle_response(response).await
    }

    // Payment management

    /// Pay an invoice
    pub async fn pay_invoice(&self, request: PayInvoiceRequest) -> Result<PayInvoiceResponse> {
        let response = self
            .client
            .post(format!(
                "{base_url}/api/v1/payments",
                base_url = self.base_url
            ))
            .json(&request)
            .send()
            .await?;

        handle_response(response).await
    }

    // Transaction management

    /// List transactions
    pub async fn list_transactions(
        &self,
        federation_id: Option<FederationId>,
        limit: Option<usize>,
    ) -> Result<Vec<TransactionInfo>> {
        let base_url = &self.base_url;
        let mut url = format!("{base_url}/api/v1/transactions");
        let mut params = Vec::new();

        if let Some(id) = federation_id {
            let id = id.as_str();
            params.push(format!("federation_id={id}"));
        }
        if let Some(limit) = limit {
            params.push(format!("limit={limit}"));
        }

        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let response = self.client.get(url).send().await?;

        handle_response(response).await
    }

    // NWC connection management

    /// Create a new NWC connection
    pub async fn create_nwc_connection(
        &self,
        request: CreateConnectionRequest,
    ) -> Result<CreateConnectionResponse> {
        let response = self
            .client
            .post(format!(
                "{base_url}/api/v1/nwc/connections",
                base_url = self.base_url
            ))
            .json(&request)
            .send()
            .await?;

        handle_response(response).await
    }

    /// List NWC connections
    pub async fn list_nwc_connections(&self) -> Result<Vec<ConnectionInfo>> {
        let response = self
            .client
            .get(format!(
                "{base_url}/api/v1/nwc/connections",
                base_url = self.base_url
            ))
            .send()
            .await?;

        handle_response(response).await
    }
}

/// Handle API response and error extraction
async fn handle_response<T: for<'de> Deserialize<'de>>(response: reqwest::Response) -> Result<T> {
    let status = response.status();

    if status.is_success() {
        let data = response.json::<T>().await?;
        Ok(data)
    } else {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        Err(anyhow!("API error ({status}): {error_text}"))
    }
}

// Request/Response types matching the server handlers

#[derive(Debug, Serialize)]
pub struct AddFederationRequest {
    pub invite_code: fedimint_core::invite_code::InviteCode,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AddFederationResponse {
    pub federation_id: FederationId,
    pub name: FederationName,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FederationInfo {
    pub id: FederationId,
    pub name: FederationName,
    pub balance: Amount,
    pub status: nanduti_core::federation::FederationStatus,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GatewayInfo {
    pub gateway_id: GatewayId,
    pub api: GatewayApiUrl,
    pub base_fee_msat: Amount,
    pub proportional_fee_ppm: u32,
}

#[derive(Debug, Serialize)]
pub struct CreateInvoiceRequest {
    pub federation_id: Option<FederationId>,
    pub amount: Amount,
    pub description: Description,
    pub expiry: Option<Expiry>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateInvoiceResponse {
    pub invoice: Bolt11String,
    pub payment_hash: PaymentHash,
    pub amount: Amount,
    pub amount_msats: u64,
    pub federation_id: FederationId,
}

#[derive(Debug, Serialize)]
pub struct PayInvoiceRequest {
    pub federation_id: Option<FederationId>,
    pub invoice: Bolt11String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PayInvoiceResponse {
    pub payment_hash: PaymentHash,
    pub preimage: Preimage,
    pub amount_paid: Amount,
    pub fees_paid: Option<Amount>,
    pub federation_id: FederationId,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TransactionInfo {
    pub id: TransactionId,
    pub federation_id: FederationId,
    pub transaction_type: TransactionType,
    pub state: TransactionState,
    pub amount: Amount,
    pub description: Option<Description>,
    pub payment_hash: PaymentHash,
    pub created_at: Timestamp,
    pub settled_at: Option<Timestamp>,
}

#[derive(Debug, Serialize)]
pub struct CreateConnectionRequest {
    pub name: ConnectionName,
    pub daily_limit: Option<Amount>,
    pub per_payment_limit: Option<Amount>,
    pub allowed_federations: Vec<FederationId>,
    pub relays: Vec<RelayUrl>,
    pub lud16: Option<LightningAddress>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateConnectionResponse {
    pub connection_id: ConnectionId,
    pub name: ConnectionName,
    pub pubkey: PublicKey,
    pub connection_uri: ConnectionUri,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ConnectionInfo {
    pub id: ConnectionId,
    pub name: ConnectionName,
    pub pubkey: PublicKey,
    pub created_at: Timestamp,
    pub last_used: Option<Timestamp>,
    pub total_spent_msats: u64,
}
