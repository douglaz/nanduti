//! MCP server implementation for Nanduti
//! Provides AI assistants with access to NWC (Nostr Wallet Connect) functionality

use anyhow::Result;
use bitcoin::Network;
use lightning_invoice::Bolt11Invoice;
use rmcp::{
    handler::server::ServerHandler,
    model::{
        CallToolRequestParam, CallToolResult, Content, ListToolsResult, PaginatedRequestParam,
    },
    schemars::{self, JsonSchema},
    service::{RequestContext, ServiceExt},
    transport::stdio,
    ErrorData as McpError,
};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

use crate::api_client;
use nanduti_core::models::{Amount, Bolt11String, Description, FederationId};
use std::str::FromStr;

/// Configuration for the MCP server
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    /// Hostname of the Nanduti API server
    pub api_host: String,
    /// Port of the Nanduti API server
    pub api_port: u16,
    /// API key for authentication (reserved for future use).
    /// Set via API_KEY environment variable.
    /// Will be used for API authentication when that feature is implemented.
    #[allow(dead_code)]
    pub api_key: Option<String>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            api_host: std::env::var("API_HOST").unwrap_or_else(|_| "localhost".to_string()),
            api_port: std::env::var("API_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3517),
            api_key: std::env::var("API_KEY").ok(),
        }
    }
}

/// The main MCP server for Nanduti
#[derive(Clone)]
pub struct NandutiMcpServer {
    config: McpServerConfig,
    state: Arc<Mutex<ServerState>>,
}

#[derive(Default)]
struct ServerState {
    // API base URL
    api_url: Option<String>,
}

// Request structures for Lightning tools
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PayInvoiceRequest {
    #[schemars(description = "BOLT11 invoice to pay")]
    pub invoice: String,
    #[schemars(description = "Optional federation ID to use for payment")]
    pub federation_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateInvoiceRequest {
    #[schemars(description = "Amount in millisatoshis")]
    pub amount_msats: u64,
    #[schemars(description = "Invoice description")]
    pub description: String,
    #[schemars(description = "Optional federation ID to use")]
    pub federation_id: Option<String>,
    #[schemars(description = "Expiry in seconds (default: 3600)")]
    pub expiry: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetBalanceRequest {
    #[schemars(description = "Optional federation ID to get balance for (all if not specified)")]
    pub federation_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListTransactionsRequest {
    #[schemars(description = "Optional federation ID to filter by")]
    pub federation_id: Option<String>,
    #[schemars(description = "Maximum number of transactions to return")]
    pub limit: Option<u32>,
    #[schemars(description = "Offset for pagination")]
    pub offset: Option<u32>,
}

// Request structures for Federation tools
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddFederationRequest {
    #[schemars(description = "Fedimint invite code")]
    pub invite_code: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetFederationInfoRequest {
    #[schemars(description = "Federation ID")]
    pub federation_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RemoveFederationRequest {
    #[schemars(description = "Federation ID to remove")]
    pub federation_id: String,
}

// Request structures for NWC connection tools
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateConnectionRequest {
    #[schemars(description = "Name for the connection")]
    pub name: String,
    #[schemars(description = "Daily spending limit in millisatoshis")]
    pub daily_limit_msats: Option<u64>,
    #[schemars(description = "Per-payment limit in millisatoshis")]
    pub per_payment_limit_msats: Option<u64>,
    #[schemars(description = "Allowed federation IDs")]
    pub allowed_federations: Vec<String>,
    #[schemars(description = "Nostr relay URLs")]
    pub relays: Vec<String>,
    #[schemars(description = "Lightning address (optional)")]
    pub lud16: Option<String>,
}

// Request structures for decoding tools
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DecodeInvoiceRequest {
    #[schemars(description = "BOLT11 invoice string to decode")]
    pub invoice: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DecodeLnurlRequest {
    #[schemars(description = "LNURL string to decode")]
    pub lnurl: String,
}

impl NandutiMcpServer {
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(ServerState::default())),
        }
    }

    /// Start the MCP server
    pub async fn run(self) -> Result<()> {
        // Initialize tracing
        let log_level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
        if log_level != "error" {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::from_default_env()
                        .add_directive(tracing::Level::INFO.into()),
                )
                .init();

            info!("Starting Nanduti MCP server");
            info!(
                "Connecting to API at {}:{}",
                self.config.api_host, self.config.api_port
            );
        }

        // Initialize the API URL
        {
            let mut state = self.state.lock().await;
            state.api_url = Some(format!(
                "http://{}:{}",
                self.config.api_host, self.config.api_port
            ));
        }

        // Start the stdio transport server
        let service = self.serve(stdio()).await?;
        service.waiting().await?;

        Ok(())
    }

    async fn get_client(&self) -> Result<api_client::ApiClient> {
        let state = self.state.lock().await;
        let api_url = state
            .api_url
            .clone()
            .ok_or_else(|| anyhow::anyhow!("API URL not initialized"))?;
        api_client::ApiClient::new(api_url)
    }
}

// Implement the ServerHandler trait for NandutiMcpServer
impl ServerHandler for NandutiMcpServer {
    async fn initialize(
        &self,
        _params: rmcp::model::InitializeRequestParam,
        _req: RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::InitializeResult, McpError> {
        Ok(rmcp::model::InitializeResult {
            protocol_version: rmcp::model::ProtocolVersion::V_2024_11_05,
            capabilities: rmcp::model::ServerCapabilities {
                tools: Some(rmcp::model::ToolsCapability { list_changed: None }),
                ..Default::default()
            },
            server_info: rmcp::model::Implementation {
                name: "nanduti".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                icons: None,
                title: None,
                website_url: None,
            },
            instructions: None,
        })
    }

    async fn list_tools(
        &self,
        _params: Option<PaginatedRequestParam>,
        _req: RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        // Helper to convert schema to Arc<Map<String, Value>>
        fn schema_to_arc_map<T: JsonSchema>(
        ) -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
            let schema = schemars::schema_for!(T);
            if let Ok(serde_json::Value::Object(map)) = serde_json::to_value(schema) {
                return std::sync::Arc::new(map);
            }
            std::sync::Arc::new(serde_json::Map::new())
        }

        // Empty schema for tools with no parameters
        let empty_schema = std::sync::Arc::new(serde_json::Map::new());

        // Collect all tools manually
        let tools = vec![
            rmcp::model::Tool {
                name: "pay_invoice".into(),
                description: Some("Pay a Lightning invoice".into()),
                input_schema: schema_to_arc_map::<PayInvoiceRequest>(),
                output_schema: None,
                annotations: None,
                icons: None,
                title: None,
            },
            rmcp::model::Tool {
                name: "create_invoice".into(),
                description: Some("Create a Lightning invoice".into()),
                input_schema: schema_to_arc_map::<CreateInvoiceRequest>(),
                output_schema: None,
                annotations: None,
                icons: None,
                title: None,
            },
            rmcp::model::Tool {
                name: "get_balance".into(),
                description: Some("Get wallet balance".into()),
                input_schema: schema_to_arc_map::<GetBalanceRequest>(),
                output_schema: None,
                annotations: None,
                icons: None,
                title: None,
            },
            rmcp::model::Tool {
                name: "list_transactions".into(),
                description: Some("List recent transactions".into()),
                input_schema: schema_to_arc_map::<ListTransactionsRequest>(),
                output_schema: None,
                annotations: None,
                icons: None,
                title: None,
            },
            rmcp::model::Tool {
                name: "list_federations".into(),
                description: Some("List all connected federations".into()),
                input_schema: empty_schema.clone(),
                output_schema: None,
                annotations: None,
                icons: None,
                title: None,
            },
            rmcp::model::Tool {
                name: "add_federation".into(),
                description: Some("Add a new federation from invite code".into()),
                input_schema: schema_to_arc_map::<AddFederationRequest>(),
                output_schema: None,
                annotations: None,
                icons: None,
                title: None,
            },
            rmcp::model::Tool {
                name: "get_federation_info".into(),
                description: Some("Get information about a specific federation".into()),
                input_schema: schema_to_arc_map::<GetFederationInfoRequest>(),
                output_schema: None,
                annotations: None,
                icons: None,
                title: None,
            },
            rmcp::model::Tool {
                name: "remove_federation".into(),
                description: Some("Remove a federation".into()),
                input_schema: schema_to_arc_map::<RemoveFederationRequest>(),
                output_schema: None,
                annotations: None,
                icons: None,
                title: None,
            },
            rmcp::model::Tool {
                name: "list_connections".into(),
                description: Some("List all NWC connections".into()),
                input_schema: empty_schema.clone(),
                output_schema: None,
                annotations: None,
                icons: None,
                title: None,
            },
            rmcp::model::Tool {
                name: "create_connection".into(),
                description: Some("Create a new NWC connection".into()),
                input_schema: schema_to_arc_map::<CreateConnectionRequest>(),
                output_schema: None,
                annotations: None,
                icons: None,
                title: None,
            },
            rmcp::model::Tool {
                name: "decode_invoice".into(),
                description: Some("Decode and parse a BOLT11 Lightning invoice".into()),
                input_schema: schema_to_arc_map::<DecodeInvoiceRequest>(),
                output_schema: None,
                annotations: None,
                icons: None,
                title: None,
            },
            rmcp::model::Tool {
                name: "decode_lnurl".into(),
                description: Some("Decode an LNURL".into()),
                input_schema: schema_to_arc_map::<DecodeLnurlRequest>(),
                output_schema: None,
                annotations: None,
                icons: None,
                title: None,
            },
            rmcp::model::Tool {
                name: "get_info".into(),
                description: Some("Get general wallet information".into()),
                input_schema: empty_schema,
                output_schema: None,
                annotations: None,
                icons: None,
                title: None,
            },
        ];

        Ok(ListToolsResult {
            tools,
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        params: CallToolRequestParam,
        _req: RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        // Dispatch to the appropriate tool handler
        let result = match params.name.as_ref() {
            "pay_invoice" => {
                let request: PayInvoiceRequest = serde_json::from_value(serde_json::Value::Object(
                    params.arguments.unwrap_or_default(),
                ))
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                self.pay_invoice(request).await
            }
            "create_invoice" => {
                let request: CreateInvoiceRequest = serde_json::from_value(
                    serde_json::Value::Object(params.arguments.unwrap_or_default()),
                )
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                self.create_invoice(request).await
            }
            "get_balance" => {
                let request: GetBalanceRequest = serde_json::from_value(serde_json::Value::Object(
                    params.arguments.unwrap_or_default(),
                ))
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                self.get_balance(request).await
            }
            "list_transactions" => {
                let request: ListTransactionsRequest = serde_json::from_value(
                    serde_json::Value::Object(params.arguments.unwrap_or_default()),
                )
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                self.list_transactions(request).await
            }
            "list_federations" => self.list_federations().await,
            "add_federation" => {
                let request: AddFederationRequest = serde_json::from_value(
                    serde_json::Value::Object(params.arguments.unwrap_or_default()),
                )
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                self.add_federation(request).await
            }
            "get_federation_info" => {
                let request: GetFederationInfoRequest = serde_json::from_value(
                    serde_json::Value::Object(params.arguments.unwrap_or_default()),
                )
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                self.get_federation_info(request).await
            }
            "remove_federation" => {
                let request: RemoveFederationRequest = serde_json::from_value(
                    serde_json::Value::Object(params.arguments.unwrap_or_default()),
                )
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                self.remove_federation(request).await
            }
            "list_connections" => self.list_connections().await,
            "create_connection" => {
                let request: CreateConnectionRequest = serde_json::from_value(
                    serde_json::Value::Object(params.arguments.unwrap_or_default()),
                )
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                self.create_connection(request).await
            }
            "decode_invoice" => {
                let request: DecodeInvoiceRequest = serde_json::from_value(
                    serde_json::Value::Object(params.arguments.unwrap_or_default()),
                )
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                self.decode_invoice(request).await
            }
            "decode_lnurl" => {
                let request: DecodeLnurlRequest = serde_json::from_value(
                    serde_json::Value::Object(params.arguments.unwrap_or_default()),
                )
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
                self.decode_lnurl(request).await
            }
            "get_info" => self.get_info().await,
            _ => CallToolResult::error(vec![Content::text(format!(
                "Unknown tool: {}",
                params.name
            ))]),
        };

        Ok(result)
    }
}

// Implement the Lightning tools
impl NandutiMcpServer {
    async fn pay_invoice(&self, request: PayInvoiceRequest) -> CallToolResult {
        let client = match self.get_client().await {
            Ok(c) => c,
            Err(error) => {
                return CallToolResult::error(vec![Content::text(format!("Error: {error}"))])
            }
        };

        let pay_request = api_client::PayInvoiceRequest {
            invoice: Bolt11String::new(request.invoice),
            federation_id: request.federation_id.map(FederationId::new),
        };

        match client.pay_invoice(pay_request).await {
            Ok(response) => CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&response).unwrap_or_else(|e| e.to_string()),
            )]),
            Err(error) => CallToolResult::error(vec![Content::text(format!("Error: {error}"))]),
        }
    }

    async fn create_invoice(&self, request: CreateInvoiceRequest) -> CallToolResult {
        let client = match self.get_client().await {
            Ok(c) => c,
            Err(error) => {
                return CallToolResult::error(vec![Content::text(format!("Error: {error}"))])
            }
        };

        let create_request = api_client::CreateInvoiceRequest {
            federation_id: request.federation_id.map(FederationId::new),
            amount: Amount::from_msats(request.amount_msats),
            description: Description::new(request.description),
            expiry: request.expiry.map(nanduti_core::models::Expiry::from_secs),
        };

        match client.create_invoice(create_request).await {
            Ok(response) => CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&response).unwrap_or_else(|e| e.to_string()),
            )]),
            Err(error) => CallToolResult::error(vec![Content::text(format!("Error: {error}"))]),
        }
    }

    async fn get_balance(&self, request: GetBalanceRequest) -> CallToolResult {
        let client = match self.get_client().await {
            Ok(c) => c,
            Err(error) => {
                return CallToolResult::error(vec![Content::text(format!("Error: {error}"))])
            }
        };

        if let Some(federation_id) = request.federation_id {
            match client
                .get_federation_balance(&FederationId::new(federation_id))
                .await
            {
                Ok(balance) => CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&balance).unwrap_or_else(|e| e.to_string()),
                )]),
                Err(error) => CallToolResult::error(vec![Content::text(format!("Error: {error}"))]),
            }
        } else {
            // Get total balance across all federations
            match client.list_federations().await {
                Ok(federations) => {
                    let mut total_msats = 0u64;
                    let mut balances = vec![];

                    for fed in federations {
                        total_msats += fed.balance.as_msats();
                        balances.push(serde_json::json!({
                            "federation_id": &fed.id,
                            "balance_msats": fed.balance.as_msats()
                        }));
                    }

                    let response = serde_json::json!({
                        "total_balance_msats": total_msats,
                        "federations": balances
                    });

                    CallToolResult::success(vec![Content::text(
                        serde_json::to_string_pretty(&response).unwrap_or_else(|e| e.to_string()),
                    )])
                }
                Err(error) => CallToolResult::error(vec![Content::text(format!("Error: {error}"))]),
            }
        }
    }

    async fn list_transactions(&self, request: ListTransactionsRequest) -> CallToolResult {
        let client = match self.get_client().await {
            Ok(c) => c,
            Err(error) => {
                return CallToolResult::error(vec![Content::text(format!("Error: {error}"))])
            }
        };

        let federation_id = request.federation_id.map(FederationId::new);
        let limit = request.limit.map(|l| l as usize);
        let offset = request.offset.map(|o| o as usize);

        match client.list_transactions(federation_id, limit, offset).await {
            Ok(transactions) => CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&transactions).unwrap_or_else(|e| e.to_string()),
            )]),
            Err(error) => CallToolResult::error(vec![Content::text(format!("Error: {error}"))]),
        }
    }
}

// Implement the Federation management tools
impl NandutiMcpServer {
    async fn list_federations(&self) -> CallToolResult {
        let client = match self.get_client().await {
            Ok(c) => c,
            Err(error) => {
                return CallToolResult::error(vec![Content::text(format!("Error: {error}"))])
            }
        };

        match client.list_federations().await {
            Ok(federations) => CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&federations).unwrap_or_else(|e| e.to_string()),
            )]),
            Err(error) => CallToolResult::error(vec![Content::text(format!("Error: {error}"))]),
        }
    }

    async fn add_federation(&self, request: AddFederationRequest) -> CallToolResult {
        let client = match self.get_client().await {
            Ok(c) => c,
            Err(error) => {
                return CallToolResult::error(vec![Content::text(format!("Error: {error}"))])
            }
        };

        // Parse the invite code string into InviteCode type
        let invite_code =
            match fedimint_core::invite_code::InviteCode::from_str(&request.invite_code) {
                Ok(code) => code,
                Err(error) => {
                    return CallToolResult::error(vec![Content::text(format!(
                        "Invalid invite code: {error}"
                    ))])
                }
            };

        match client.add_federation(invite_code).await {
            Ok(federation) => CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&federation).unwrap_or_else(|e| e.to_string()),
            )]),
            Err(error) => CallToolResult::error(vec![Content::text(format!("Error: {error}"))]),
        }
    }

    async fn get_federation_info(&self, request: GetFederationInfoRequest) -> CallToolResult {
        let client = match self.get_client().await {
            Ok(c) => c,
            Err(error) => {
                return CallToolResult::error(vec![Content::text(format!("Error: {error}"))])
            }
        };

        match client
            .get_federation(&FederationId::new(request.federation_id))
            .await
        {
            Ok(federation) => CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&federation).unwrap_or_else(|e| e.to_string()),
            )]),
            Err(error) => CallToolResult::error(vec![Content::text(format!("Error: {error}"))]),
        }
    }

    async fn remove_federation(&self, request: RemoveFederationRequest) -> CallToolResult {
        let client = match self.get_client().await {
            Ok(c) => c,
            Err(error) => {
                return CallToolResult::error(vec![Content::text(format!("Error: {error}"))])
            }
        };

        match client
            .remove_federation(&FederationId::new(request.federation_id))
            .await
        {
            Ok(_) => CallToolResult::success(vec![Content::text(
                "Federation removed successfully".to_string(),
            )]),
            Err(error) => CallToolResult::error(vec![Content::text(format!("Error: {error}"))]),
        }
    }
}

// Implement NWC connection management tools
impl NandutiMcpServer {
    async fn list_connections(&self) -> CallToolResult {
        let client = match self.get_client().await {
            Ok(c) => c,
            Err(error) => {
                return CallToolResult::error(vec![Content::text(format!("Error: {error}"))])
            }
        };

        match client.list_nwc_connections().await {
            Ok(connections) => CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&connections).unwrap_or_else(|e| e.to_string()),
            )]),
            Err(error) => CallToolResult::error(vec![Content::text(format!("Error: {error}"))]),
        }
    }

    async fn create_connection(&self, request: CreateConnectionRequest) -> CallToolResult {
        let client = match self.get_client().await {
            Ok(c) => c,
            Err(error) => {
                return CallToolResult::error(vec![Content::text(format!("Error: {error}"))])
            }
        };

        let create_request = api_client::CreateConnectionRequest {
            name: nanduti_core::models::ConnectionName::new(request.name),
            daily_limit: request.daily_limit_msats.map(Amount::from_msats),
            per_payment_limit: request.per_payment_limit_msats.map(Amount::from_msats),
            allowed_federations: request
                .allowed_federations
                .into_iter()
                .map(FederationId::new)
                .collect(),
            relays: request
                .relays
                .into_iter()
                .map(nanduti_core::models::RelayUrl::new)
                .collect(),
            lud16: request
                .lud16
                .map(nanduti_core::models::LightningAddress::new),
        };

        match client.create_nwc_connection(create_request).await {
            Ok(connection) => CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&connection).unwrap_or_else(|e| e.to_string()),
            )]),
            Err(error) => CallToolResult::error(vec![Content::text(format!("Error: {error}"))]),
        }
    }
}

// Implement decoding tools
impl NandutiMcpServer {
    async fn decode_invoice(&self, request: DecodeInvoiceRequest) -> CallToolResult {
        // Parse the BOLT11 invoice using lightning-invoice crate
        let invoice = match request.invoice.parse::<Bolt11Invoice>() {
            Ok(inv) => inv,
            Err(e) => {
                return CallToolResult::error(vec![Content::text(format!(
                    "Invalid BOLT11 invoice: {e}"
                ))]);
            }
        };

        // Extract payment hash
        let payment_hash = hex::encode::<&[u8]>(invoice.payment_hash().as_ref());

        // Extract amount (if specified)
        let amount_msats = invoice.amount_milli_satoshis();

        // Extract description using the ref-based API
        let description = match invoice.description() {
            lightning_invoice::Bolt11InvoiceDescriptionRef::Direct(desc) => Some(desc.to_string()),
            lightning_invoice::Bolt11InvoiceDescriptionRef::Hash(hash) => Some(format!(
                "description_hash:{}",
                hex::encode::<&[u8]>(hash.0.as_ref())
            )),
        };

        // Extract expiry
        let expiry_secs = invoice.expiry_time().as_secs();

        // Extract timestamp (Duration since UNIX epoch)
        let created_at = invoice.duration_since_epoch().as_secs();

        // Extract payee pubkey - get_payee_pub_key returns the node pubkey
        let payee_pubkey = invoice.get_payee_pub_key().to_string();

        // Determine network
        let network = match invoice.network() {
            Network::Bitcoin => "mainnet",
            Network::Testnet => "testnet",
            Network::Signet => "signet",
            Network::Regtest => "regtest",
            _ => "unknown",
        };

        // Extract routing hints (if any)
        let route_hints: Vec<_> = invoice
            .route_hints()
            .iter()
            .map(|hint| {
                hint.0
                    .iter()
                    .map(|hop| {
                        serde_json::json!({
                            "pubkey": hop.src_node_id.to_string(),
                            "short_channel_id": hop.short_channel_id,
                            "base_fee_msat": hop.fees.base_msat,
                            "proportional_fee_ppm": hop.fees.proportional_millionths,
                            "cltv_expiry_delta": hop.cltv_expiry_delta,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .collect();

        // Build response
        let response = serde_json::json!({
            "invoice": request.invoice,
            "network": network,
            "payment_hash": payment_hash,
            "amount_msats": amount_msats,
            "description": description,
            "expiry_secs": expiry_secs,
            "created_at": created_at,
            "payee_pubkey": payee_pubkey,
            "route_hints": route_hints,
            "is_expired": invoice.is_expired(),
        });

        CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&response).unwrap_or_else(|e| e.to_string()),
        )])
    }

    async fn decode_lnurl(&self, request: DecodeLnurlRequest) -> CallToolResult {
        // Decode LNURL using bech32
        let lnurl = request.lnurl.to_lowercase();

        // Check if it's a valid LNURL format (bech32 encoded)
        if !lnurl.starts_with("lnurl") {
            return CallToolResult::error(vec![Content::text(
                "Invalid LNURL: must start with 'lnurl'",
            )]);
        }

        // Decode the bech32 to get the URL
        // bech32 0.11 uses a different API - decode returns (Hrp, Vec<u8>)
        match bech32::decode(&lnurl) {
            Ok((hrp, data)) => {
                if hrp.as_str() != "lnurl" {
                    return CallToolResult::error(vec![Content::text(
                        "Invalid LNURL: human-readable part must be 'lnurl'",
                    )]);
                }

                // data is already the decoded bytes in bech32 0.11
                // Convert to UTF-8 string (the URL)
                let url = match String::from_utf8(data) {
                    Ok(u) => u,
                    Err(e) => {
                        return CallToolResult::error(vec![Content::text(format!(
                            "Invalid LNURL: URL is not valid UTF-8: {e}"
                        ))]);
                    }
                };

                // Determine LNURL type based on URL pattern
                let lnurl_type = if url.contains("/lnurlp/") || url.contains("/pay") {
                    "pay"
                } else if url.contains("/lnurlw/") || url.contains("/withdraw") {
                    "withdraw"
                } else if url.contains("/lnurl-auth") || url.contains("/auth") {
                    "auth"
                } else if url.contains("/lnurlc/") || url.contains("/channel") {
                    "channel"
                } else {
                    "unknown"
                };

                let response = serde_json::json!({
                    "lnurl": request.lnurl,
                    "decoded_url": url,
                    "type": lnurl_type,
                    "note": "Fetch the URL to get full LNURL details"
                });

                CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&response).unwrap_or_else(|e| e.to_string()),
                )])
            }
            Err(e) => CallToolResult::error(vec![Content::text(format!(
                "Failed to decode LNURL bech32: {e}"
            ))]),
        }
    }

    async fn get_info(&self) -> CallToolResult {
        let client = match self.get_client().await {
            Ok(c) => c,
            Err(error) => {
                return CallToolResult::error(vec![Content::text(format!("Error: {error}"))])
            }
        };

        // Gather comprehensive info
        let federations = client.list_federations().await.unwrap_or_default();
        let connections = client.list_nwc_connections().await.unwrap_or_default();

        let mut total_balance_msats = 0u64;
        for fed in &federations {
            total_balance_msats += fed.balance.as_msats();
        }

        let info = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "api_host": self.config.api_host,
            "api_port": self.config.api_port,
            "federations_count": federations.len(),
            "connections_count": connections.len(),
            "total_balance_msats": total_balance_msats,
        });

        CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&info).unwrap_or_else(|e| e.to_string()),
        )])
    }
}

/// Run the MCP server
#[cfg(feature = "mcp")]
pub async fn run_mcp_server() -> anyhow::Result<()> {
    let config = McpServerConfig::default();
    let server = NandutiMcpServer::new(config);
    server.run().await
}
