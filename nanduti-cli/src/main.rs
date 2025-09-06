//! CLI for multi-federation Fedimint wallet with Nostr Wallet Connect

mod api_client;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use nanduti_api::RoutingStrategy;
use nanduti_core::models::*;
use std::path::PathBuf;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

#[cfg(feature = "mcp")]
mod mcp_server;

#[derive(Parser)]
#[command(name = "nanduti")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "Multi-federation Fedimint wallet with Nostr Wallet Connect")]
#[command(long_about = None)]
struct Cli {
    /// API server URL
    #[arg(
        long,
        global = true,
        env = "FEDIMINT_NWC_API_URL",
        default_value = "http://localhost:3517"
    )]
    api_url: String,

    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start NWC server
    #[command(name = "serve")]
    Serve(ServeArgs),

    // Federation management (fm-*)
    /// Add a federation from invite code
    #[command(name = "fm-add")]
    FmAdd(AddFederationArgs),

    /// Remove a federation
    #[command(name = "fm-remove")]
    FmRemove(RemoveFederationArgs),

    /// List all federations
    #[command(name = "fm-list")]
    FmList(ListFederationsArgs),

    /// Show federation balances
    #[command(name = "fm-balance")]
    FmBalance(BalanceArgs),

    /// List gateways in a federation
    #[command(name = "fm-gateways")]
    FmGateways(GatewaysArgs),

    // NWC connection management (nwc-*)
    /// Generate a new NWC connection string
    #[command(name = "nwc-new")]
    NwcNew(NewConnectionArgs),

    /// List NWC connections
    #[command(name = "nwc-list")]
    NwcList(ListConnectionsArgs),

    // Transaction operations (tx-*)
    /// List transactions
    #[command(name = "tx-list")]
    TxList(ListTransactionsArgs),

    /// Pay an invoice manually
    #[command(name = "tx-pay")]
    TxPay(PayInvoiceArgs),

    /// Create a Lightning invoice
    #[command(name = "tx-invoice")]
    TxInvoice(CreateInvoiceArgs),

    // MCP server (optional)
    #[cfg(feature = "mcp")]
    /// Start MCP server for AI assistants
    #[command(name = "mcp-server")]
    McpServer,
}

#[derive(Parser)]
struct ServeArgs {
    /// Fedimint invite codes (can specify multiple)
    #[arg(
        long = "federation",
        value_name = "INVITE_CODE",
        env = "FEDIMINT_FEDERATIONS"
    )]
    federations: Vec<String>,

    /// Nostr relays to connect to
    #[arg(
        long = "relay",
        value_name = "URL",
        default_value = "wss://relay.damus.io",
        env = "NOSTR_RELAYS"
    )]
    relays: Vec<String>,

    /// API server host
    #[arg(long, default_value = "127.0.0.1", env = "API_HOST")]
    host: String,

    /// API server port
    #[arg(long, default_value = "3517", env = "API_PORT")]
    port: u16,

    /// Data directory (optional, in-memory if not specified)
    #[arg(long, env = "DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// Log level (error, warn, info, debug, trace)
    #[arg(long, default_value = "info", env = "LOG_LEVEL")]
    log_level: String,

    /// Maximum payment amount in sats (per payment)
    #[arg(long, env = "MAX_PAYMENT_SATS")]
    max_payment_sats: Option<u64>,

    /// Daily limit in sats (aggregate)
    #[arg(long, env = "DAILY_LIMIT_SATS")]
    daily_limit_sats: Option<u64>,

    /// Federation selection strategy
    #[arg(
        long,
        default_value = "lowest-fee",
        value_enum,
        env = "ROUTING_STRATEGY"
    )]
    routing_strategy: RoutingStrategyArg,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RoutingStrategyArg {
    LowestFee,
    BestRoute,
    RoundRobin,
    BalanceWeighted,
}

impl From<RoutingStrategyArg> for RoutingStrategy {
    fn from(arg: RoutingStrategyArg) -> Self {
        match arg {
            RoutingStrategyArg::LowestFee => RoutingStrategy::LowestFee,
            RoutingStrategyArg::BestRoute => RoutingStrategy::BestRoute,
            RoutingStrategyArg::RoundRobin => RoutingStrategy::RoundRobin,
            RoutingStrategyArg::BalanceWeighted => RoutingStrategy::BalanceWeighted,
        }
    }
}

#[derive(Parser)]
struct AddFederationArgs {
    /// Fedimint invite code
    invite_code: String,
}

#[derive(Parser)]
struct RemoveFederationArgs {
    /// Federation ID or name
    federation: String,
}

#[derive(Parser)]
struct ListFederationsArgs {
    /// Output format (json, table)
    #[arg(long, default_value = "table")]
    format: OutputFormat,
}

#[derive(Parser)]
struct BalanceArgs {
    /// Show detailed per-federation balances
    #[arg(long)]
    detailed: bool,

    /// Output format (json, table)
    #[arg(long, default_value = "table")]
    format: OutputFormat,
}

#[derive(Parser)]
struct GatewaysArgs {
    /// Federation ID (optional, all if not specified)
    #[arg(long)]
    federation: Option<String>,

    /// Output format (json, table)
    #[arg(long, default_value = "table")]
    format: OutputFormat,
}

#[derive(Parser)]
struct NewConnectionArgs {
    /// Connection name
    #[arg(long)]
    name: String,

    /// Daily limit in sats
    #[arg(long)]
    daily_limit_sats: Option<u64>,

    /// Per-payment limit in sats
    #[arg(long)]
    per_payment_limit_sats: Option<u64>,

    /// Allowed federations (comma-separated IDs, or "*" for all)
    #[arg(long, default_value = "*")]
    federations: String,

    /// Nostr relays to use for this connection
    #[arg(
        long = "relay",
        value_name = "URL",
        default_value = "wss://relay.damus.io"
    )]
    relays: Vec<String>,

    /// Lightning address (LUD16) for this connection
    #[arg(long)]
    lud16: Option<String>,
}

#[derive(Parser)]
struct ListConnectionsArgs {
    /// Output format (json, table)
    #[arg(long, default_value = "table")]
    format: OutputFormat,
}

#[derive(Parser)]
struct ListTransactionsArgs {
    /// Limit number of results
    #[arg(long, default_value = "10")]
    limit: usize,

    /// Federation ID (optional, all if not specified)
    #[arg(long)]
    federation: Option<String>,

    /// Output format (json, table)
    #[arg(long, default_value = "table")]
    format: OutputFormat,
}

#[derive(Parser)]
struct PayInvoiceArgs {
    /// Lightning invoice
    invoice: String,

    /// Federation ID (optional, auto-select if not specified)
    #[arg(long)]
    federation: Option<String>,

    /// Output format
    #[arg(long, default_value = "table")]
    format: OutputFormat,
}

#[derive(Parser)]
struct CreateInvoiceArgs {
    /// Amount (e.g., "100sats", "0.001btc", "1000msats")
    amount: String,

    /// Description for the invoice
    #[arg(long, default_value = "Payment")]
    description: String,

    /// Expiry time in seconds (default: 3600 = 1 hour)
    #[arg(long, default_value = "3600")]
    expiry: u64,

    /// Federation ID (optional, auto-select if not specified)
    #[arg(long)]
    federation: Option<String>,

    /// Output format
    #[arg(long, default_value = "table")]
    format: OutputFormat,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutputFormat {
    Json,
    Table,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve(args) => serve(args).await,
        Commands::FmAdd(args) => add_federation(args, &cli.api_url).await,
        Commands::FmRemove(args) => remove_federation(args, &cli.api_url).await,
        Commands::FmList(args) => list_federations(args, &cli.api_url).await,
        Commands::FmBalance(args) => show_balance(args, &cli.api_url).await,
        Commands::FmGateways(args) => list_gateways(args, &cli.api_url).await,
        Commands::NwcNew(args) => new_connection(args, &cli.api_url).await,
        Commands::NwcList(args) => list_connections(args, &cli.api_url).await,
        Commands::TxList(args) => list_transactions(args, &cli.api_url).await,
        Commands::TxPay(args) => pay_invoice(args, &cli.api_url).await,
        Commands::TxInvoice(args) => create_invoice(args, &cli.api_url).await,
        #[cfg(feature = "mcp")]
        Commands::McpServer => mcp_server::run_mcp_server().await,
    }
}

async fn serve(args: ServeArgs) -> Result<()> {
    // Setup logging
    setup_logging(&args.log_level)?;

    tracing::info!("Starting Nanduti server");

    // Create server config
    let config = nanduti_api::ServerConfig {
        host: args.host,
        port: args.port,
        relays: args.relays,
        data_dir: args.data_dir,
        routing_strategy: args.routing_strategy.into(),
        max_payment_sats: args.max_payment_sats,
        daily_limit_sats: args.daily_limit_sats,
    };

    // Start server
    nanduti_api::start_server(config).await?;

    Ok(())
}

async fn add_federation(args: AddFederationArgs, api_url: &str) -> Result<()> {
    use std::str::FromStr;
    let client = api_client::ApiClient::new(api_url.to_string())?;
    let invite_code = fedimint_core::invite_code::InviteCode::from_str(&args.invite_code)
        .context("Invalid invite code")?;
    let response = client.add_federation(invite_code).await?;
    let federation_id = &response.federation_id;
    let name = &response.name;
    println!("Successfully added federation: {federation_id} ({name})");
    Ok(())
}

async fn remove_federation(args: RemoveFederationArgs, api_url: &str) -> Result<()> {
    let client = api_client::ApiClient::new(api_url.to_string())?;
    let federation_id = FederationId::new(args.federation.clone());
    client.remove_federation(&federation_id).await?;
    let federation = &args.federation;
    println!("Successfully removed federation: {federation}");
    Ok(())
}

async fn list_federations(args: ListFederationsArgs, api_url: &str) -> Result<()> {
    let client = api_client::ApiClient::new(api_url.to_string())?;
    let federations = client.list_federations().await?;

    match args.format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&federations)?;
            println!("{json}");
        }
        OutputFormat::Table => {
            println!("Federations:");
            println!(
                "{id:<20} {name:<20} {balance:<15} {status:<10}",
                id = "ID",
                name = "Name",
                balance = "Balance (sats)",
                status = "Status"
            );
            let separator = "-".repeat(70);
            println!("{separator}");
            for federation in federations {
                let id = &federation.id;
                let name = &federation.name;
                let balance = federation.balance.as_sats();
                let status = &federation.status;
                println!("{id:<20} {name:<20} {balance:<15} {status:<10}");
            }
        }
    }

    Ok(())
}

async fn show_balance(args: BalanceArgs, api_url: &str) -> Result<()> {
    let client = api_client::ApiClient::new(api_url.to_string())?;
    let federations = client.list_federations().await?;

    if args.detailed {
        match args.format {
            OutputFormat::Json => {
                let balances: Vec<_> = federations
                    .iter()
                    .map(|f| {
                        serde_json::json!({
                            "federation_id": f.id,
                            "federation_name": f.name,
                            "balance_sats": f.balance.as_sats(),
                        })
                    })
                    .collect();
                let json = serde_json::to_string_pretty(&balances)?;
                println!("{json}");
            }
            OutputFormat::Table => {
                println!("Federation Balances:");
                println!("{:<20} {:<20} {:<15}", "ID", "Name", "Balance (sats)");
                let separator = "-".repeat(60);
                println!("{separator}");
                for federation in federations {
                    println!(
                        "{:<20} {:<20} {:<15}",
                        federation.id,
                        federation.name,
                        federation.balance.as_sats()
                    );
                }
            }
        }
    } else {
        let total_balance: u64 = federations.iter().map(|f| f.balance.as_sats()).sum();

        match args.format {
            OutputFormat::Json => {
                let json_output = serde_json::json!({
                    "total_balance_sats": total_balance,
                });
                println!("{json_output}");
            }
            OutputFormat::Table => {
                println!("Total balance: {total_balance} sats");
            }
        }
    }

    Ok(())
}

async fn list_gateways(args: GatewaysArgs, api_url: &str) -> Result<()> {
    let client = api_client::ApiClient::new(api_url.to_string())?;

    let federations = if let Some(fed_id) = args.federation {
        let federation_id = FederationId::new(fed_id);
        vec![client.get_federation(&federation_id).await?]
    } else {
        client.list_federations().await?
    };

    for federation in federations {
        let name = &federation.name;
        let id = &federation.id;
        println!("\nFederation: {name} ({id})");

        match client.list_federation_gateways(&federation.id).await {
            Ok(gateways) => match args.format {
                OutputFormat::Json => {
                    let json = serde_json::to_string_pretty(&gateways)?;
                    println!("{json}");
                }
                OutputFormat::Table => {
                    if gateways.is_empty() {
                        println!("  No gateways available");
                    } else {
                        println!("  Gateways:");
                        println!(
                            "  {:<44} {:<20} {:<15} {:<15}",
                            "Gateway ID", "API", "Base Fee", "Prop Fee"
                        );
                        let separator = "-".repeat(100);
                        println!("  {separator}");
                        for gateway in gateways {
                            let gateway_id = &gateway.gateway_id;
                            let api = if gateway.api.as_str().len() > 20 {
                                let prefix = &gateway.api.as_str()[..17];
                                format!("{prefix}...")
                            } else {
                                gateway.api.to_string()
                            };
                            let base_fee = gateway.base_fee_msat;
                            let base_fee_str = format!("{base_fee} msat");
                            let prop_fee = gateway.proportional_fee_ppm;
                            let prop_fee_str = format!("{prop_fee}/M");
                            println!(
                                "  {gateway_id:<44} {api:<20} {base_fee_str:<15} {prop_fee_str:<15}"
                            );
                        }
                    }
                }
            },
            Err(_) => {
                println!("  Federation offline or no gateways available");
            }
        }
    }

    Ok(())
}

async fn new_connection(args: NewConnectionArgs, api_url: &str) -> Result<()> {
    let client = api_client::ApiClient::new(api_url.to_string())?;

    let relays = if args.relays.is_empty() {
        vec!["wss://relay.damus.io".to_string()]
    } else {
        args.relays.clone()
    };

    let allowed_federations = if args.federations == "*" {
        vec![FederationId::new("*".to_string())]
    } else {
        args.federations
            .split(',')
            .map(|s| FederationId::new(s.to_string()))
            .collect()
    };

    let request = api_client::CreateConnectionRequest {
        name: ConnectionName::new(args.name),
        daily_limit_sats: args.daily_limit_sats,
        per_payment_limit_sats: args.per_payment_limit_sats,
        allowed_federations,
        relays: relays.into_iter().map(RelayUrl::new).collect(),
        lud16: args.lud16.map(LightningAddress::new),
    };

    let response = client.create_nwc_connection(request).await?;

    println!("NWC Connection created!");
    let name = &response.name;
    println!("Name: {name}");
    let pubkey = &response.pubkey;
    println!("Wallet Public Key: {pubkey}");
    println!("Connection URI:");
    let uri = &response.connection_uri;
    println!("{uri}");

    Ok(())
}

async fn list_connections(args: ListConnectionsArgs, api_url: &str) -> Result<()> {
    let client = api_client::ApiClient::new(api_url.to_string())?;
    let connections = client.list_nwc_connections().await?;

    match args.format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&connections)?;
            println!("{json}");
        }
        OutputFormat::Table => {
            println!("NWC Connections:");
            println!(
                "{name:<30} {created:<20} {spent:<15}",
                name = "Name",
                created = "Created",
                spent = "Spent (sats)"
            );
            let separator = "-".repeat(70);
            println!("{separator}");
            for connection in connections {
                let created = chrono::DateTime::from_timestamp(connection.created_at.as_i64(), 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "Unknown".to_string());

                println!(
                    "{:<30} {:<20} {:<15}",
                    connection.name,
                    created,
                    connection.total_spent_msats / 1000
                );
            }
        }
    }

    Ok(())
}

async fn list_transactions(args: ListTransactionsArgs, api_url: &str) -> Result<()> {
    let client = api_client::ApiClient::new(api_url.to_string())?;
    let federation_id = args.federation.map(FederationId::new);
    let transactions = client
        .list_transactions(federation_id, Some(args.limit))
        .await?;

    match args.format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&transactions)?;
            println!("{json}");
        }
        OutputFormat::Table => {
            println!("Transactions:");
            println!(
                "{:<10} {:<20} {:<12} {:<15} {:<10}",
                "Type", "Created", "Amount (sats)", "Federation", "State"
            );
            let separator = "-".repeat(80);
            println!("{separator}");
            for tx in transactions {
                let created = chrono::DateTime::from_timestamp(tx.created_at.as_i64(), 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "Unknown".to_string());

                println!(
                    "{:<10} {:<20} {:<12} {:<15} {:<10}",
                    tx.transaction_type,
                    created,
                    tx.amount_sats,
                    &tx.federation_id.as_str()[..15.min(tx.federation_id.as_str().len())],
                    tx.state
                );
            }
        }
    }

    Ok(())
}

async fn pay_invoice(args: PayInvoiceArgs, api_url: &str) -> Result<()> {
    let client = api_client::ApiClient::new(api_url.to_string())?;
    let request = api_client::PayInvoiceRequest {
        federation_id: args.federation.map(FederationId::new),
        invoice: Bolt11String::new(args.invoice),
    };
    let response = client.pay_invoice(request).await?;

    match args.format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&response)?;
            println!("{json}");
        }
        OutputFormat::Table => {
            println!("Payment successful!");
            let payment_hash = &response.payment_hash;
            println!("Payment hash: {payment_hash}");
            let preimage = &response.preimage;
            println!("Preimage: {preimage}");
            if let Some(fees) = response.fees_paid_msats {
                println!("Fees paid: {fees} msats");
            }
            let amount = response.amount_paid_msats;
            println!("Amount paid: {amount} msats");
            let federation = &response.federation_id;
            println!("Federation: {federation}");
        }
    }

    Ok(())
}

async fn create_invoice(args: CreateInvoiceArgs, api_url: &str) -> Result<()> {
    use std::str::FromStr;
    let client = api_client::ApiClient::new(api_url.to_string())?;
    let amount = Amount::from_str(&args.amount)
        .context("Invalid amount format. Use formats like '100sat', '0.001btc', or '1000msat'")?;
    let request = api_client::CreateInvoiceRequest {
        federation_id: args.federation.map(FederationId::new),
        amount,
        description: Description::new(args.description.clone()),
        expiry: Some(Expiry(args.expiry)),
    };
    let response = client.create_invoice(request).await?;

    match args.format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&response)?;
            println!("{json}");
        }
        OutputFormat::Table => {
            println!("Invoice created successfully!");
            let invoice = &response.invoice;
            println!("Invoice: {invoice}");
            let payment_hash = &response.payment_hash;
            println!("Payment hash: {payment_hash}");
            let amount = response.amount_sats;
            println!("Amount: {amount} sats");
            let federation = &response.federation_id;
            println!("Federation: {federation}");
        }
    }

    Ok(())
}

fn setup_logging(level_str: &str) -> Result<()> {
    let level = match level_str.to_lowercase().as_str() {
        "error" => Level::ERROR,
        "warn" => Level::WARN,
        "info" => Level::INFO,
        "debug" => Level::DEBUG,
        "trace" => Level::TRACE,
        _ => Level::INFO,
    };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(level)
        .with_target(false)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .context("Failed to set global tracing subscriber")?;

    Ok(())
}

// Add chrono for timestamp formatting
