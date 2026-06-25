//! Named constants used throughout the nanduti codebase.
//!
//! This module centralizes magic numbers to improve readability and maintainability.
//! Each constant is documented with its purpose and usage context.

// ============================================================================
// Time Constants
// ============================================================================

/// Number of seconds in one day (24 hours).
/// Used for daily spending limit calculations.
pub const SECONDS_PER_DAY: u64 = 86400;

/// Default expiry time for Lightning invoices in seconds (1 hour).
/// This is the BOLT11 standard default expiry.
pub const DEFAULT_INVOICE_EXPIRY_SECS: u64 = 3600;

// ============================================================================
// Fee Constants (Hardcoded fallbacks)
// ============================================================================

/// Default base fee in millisatoshis (1 sat).
/// Used when gateway fee schedule is unavailable.
/// TODO: Replace with dynamic fee estimation from gateway (see nopus.md 3.1)
pub const DEFAULT_BASE_FEE_MSATS: u64 = 1000;

/// Default proportional fee in parts per million (0.25%).
/// Used when gateway fee schedule is unavailable.
/// TODO: Replace with dynamic fee estimation from gateway (see nopus.md 3.1)
pub const DEFAULT_PROPORTIONAL_FEE_PPM: u64 = 2500;

// ============================================================================
// Nostr Relay Constants
// ============================================================================

/// Default Nostr relay URL for NWC connections.
/// This is a well-known public relay with good availability.
/// Users can override this via CLI flags or environment variables.
pub const DEFAULT_RELAY_URL: &str = "wss://relay.damus.io";

// ============================================================================
// Wallet Identity Constants
// ============================================================================

/// Wallet alias returned in NWC get_info responses.
pub const WALLET_ALIAS: &str = "Nanduti";

/// Wallet color (hex) returned in NWC get_info responses.
/// Orange color representing the Nanduti brand.
pub const WALLET_COLOR: &str = "#FF6B00";

/// Placeholder block hash used when actual block hash is unavailable.
/// This is a zero hash (64 hex characters representing 32 zero bytes).
pub const ZERO_BLOCK_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";
