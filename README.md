# Nanduti

**Multi-backend NWC (Nostr Wallet Connect) implementation**

Nanduti ("spider web" in Guaraní) weaves connections between your Lightning wallets and Nostr apps. A headless wallet service that implements the NIP-47 protocol, currently supporting Fedimint federations with plans for additional backend integrations.

## Features

- 🕸️ **Multi-Backend Ready**: Extensible architecture for multiple wallet backends
- 🏛️ **Fedimint Support**: Connect to multiple Fedimint federations simultaneously
- ⚡ **NIP-47 Protocol**: Full implementation of Nostr Wallet Connect for Lightning payments
- 🔄 **Smart Routing**: Automatic backend selection based on fees, balance, or custom strategies
- 🔒 **Secure**: Isolated keys, per-connection permissions
- 📦 **Static Binaries**: Musl-based builds for easy deployment
- 🎯 **CLI-First**: No config files required, everything via command-line arguments
- 🤖 **MCP Support**: Optional Model Context Protocol server for AI assistants

## Installation

### Using Nix (Recommended)

```bash
# Run directly from GitHub
nix run github:user/nanduti -- --help

# Install to your system
nix profile install github:user/nanduti

# Build from source
git clone https://github.com/user/nanduti
cd nanduti
nix build
./result/bin/nanduti --help
```

### Build from Source

```bash
# Clone repository
git clone https://github.com/user/nanduti
cd nanduti

# Enter development environment
nix develop

# Build
cargo build --release

# Run
./target/release/nanduti --help
```

## Quick Start

### 1. Start Server with Single Federation

```bash
nanduti serve --federation "fed11qgqzc4u..."
```

### 2. Start with Multiple Federations

```bash
nanduti serve \
  --federation "fed11qgqzc4u..." \
  --federation "fed11qgqrg9u..." \
  --relay "wss://relay.damus.io" \
  --relay "wss://nos.lol" \
  --data-dir ~/.nanduti \
  --max-payment-sats 1000000 \
  --routing-strategy best-route
```

### 3. Generate NWC Connection String

```bash
# Create connection with access to all federations
nanduti nwc-new \
  --name "My Lightning App" \
  --daily-limit-sats 100000
```

This outputs a connection string like:
```
nostr+walletconnect://02abc...?relay=wss://relay.damus.io&secret=def...
```

### 4. Use with NWC-Compatible Apps

Copy the connection string to any app that supports NWC:
- Zeus Wallet
- Alby Extension
- Mutiny Wallet
- And many more...

## Usage

### Server Commands

```bash
# Start server (all config via CLI args, no config files)
nanduti serve \
  --federation "invite_code_1" \
  --federation "invite_code_2" \
  --relay "wss://relay.damus.io" \
  --port 3517 \
  --data-dir ~/.nanduti \
  --routing-strategy lowest-fee

# Using environment variables
export FEDIMINT_FEDERATIONS="fed11...,fed12..."
export NOSTR_RELAYS="wss://relay.damus.io,wss://nos.lol"
export API_PORT=3517
nanduti serve
```

### Federation Management

```bash
# Add federation
nanduti fm-add "fed11qgqzc4u..."

# Remove federation
nanduti fm-remove "fed_12345678"

# List federations
nanduti fm-list

# Show balances
nanduti fm-balance          # Aggregate balance
nanduti fm-balance --detailed  # Per-federation
```

### NWC Connection Management

```bash
# Create new connection
nanduti nwc-new \
  --name "Zeus Wallet" \
  --daily-limit-sats 50000 \
  --per-payment-limit-sats 10000

# List connections
nanduti nwc-list
```

### Transaction Management

```bash
# List recent transactions
nanduti tx-list --limit 20

# List transactions for specific federation
nanduti tx-list --federation "fed_12345678"

# Manual invoice payment
nanduti tx-pay "lnbc..." --federation "fed_12345678"
```

## Routing Strategies

The wallet supports multiple strategies for selecting which federation to use for payments:

- **`lowest-fee`**: Choose federation with lowest estimated fees
- **`best-route`**: Select based on success rate and uptime metrics
- **`round-robin`**: Distribute payments evenly across federations
- **`balance-weighted`**: Probabilistic selection based on balance distribution

## Architecture

### Workspace Structure

```
nanduti/
├── nanduti-core/   # Core library with federation management
├── nanduti-api/    # API server with NWC protocol handler
└── nanduti/        # CLI application
```

### Key Components

- **Federation Manager**: Handles multiple federation connections
- **NWC Handler**: Implements NIP-47 protocol methods
- **Router**: Selects optimal federation for each payment
- **Storage**: Optional persistence using embedded database

## NWC Protocol Support

### Implemented Methods

- ✅ `pay_invoice` - Pay Lightning invoices
- ✅ `make_invoice` - Generate invoices
- ✅ `get_balance` - Query wallet balance
- ✅ `list_transactions` - Transaction history
- ✅ `get_info` - Wallet information
- ✅ `pay_keysend` - Keysend payments
- 🚧 `lookup_invoice` - Query invoice status
- 🚧 `multi_pay_invoice` - Batch payments

### Notifications

- `payment_received` - Incoming payment notifications
- `payment_sent` - Outgoing payment confirmations

## Security

- **Key Isolation**: Each federation and NWC connection uses separate keys
- **Permission System**: Per-connection limits and method restrictions
- **Stateless Mode**: Can run entirely in-memory without persistence
- **Encryption**: Supports both NIP-44 (recommended) and NIP-04 (legacy)

## Development

### Prerequisites

- Nix (for development environment)
- Rust 1.75+ (if not using Nix)

### Development Workflow

```bash
# Enter development shell
nix develop

# Run tests
cargo test

# Check formatting
cargo fmt --check

# Run linter
cargo clippy

# Build for production
cargo build --release --target x86_64-unknown-linux-musl
```

### Project Structure

- Uses workspace with three crates
- Follows cyberkrill conventions for error handling and code style
- No config files - everything via CLI arguments
- Static musl builds for portability

## Contributing

Contributions are welcome! Please:

1. Follow the conventions in `CONVENTIONS.md`
2. Ensure all tests pass
3. Run formatters and linters
4. Update documentation as needed

## License

MIT OR Apache-2.0

## Acknowledgments

- Built on [Fedimint](https://fedimint.org/)
- Implements [NIP-47](https://github.com/nostr-protocol/nips/blob/master/47.md)
- Inspired by [cyberkrill](https://github.com/douglaz/cyberkrill) patterns