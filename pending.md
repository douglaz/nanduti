# Nanduti - Technical Debt Resolution Complete

All 14 technical debt items from nopus.md have been resolved.

## Completed Tasks

### Phase 1 - Security & Performance (Critical) - DONE
- **1.1** Transaction Data Encryption - AES-256-GCM encryption for data at rest
- **1.2** Storage Indexing - Secondary indexes for O(1) lookups

### Phase 2 - Incomplete Features (High) - DONE
- **2.1** Keysend - Removed from advertised NWC methods
- **2.2** Multi-Pay - Verified not advertised
- **2.3** MCP Invoice/LNURL Decoding - Proper BOLT11/LNURL parsing

### Phase 3 - Configuration (Medium/Low) - DONE
- **3.1** Dynamic Fee Estimation - Queries gateway for actual fees
- **3.2** Port Alignment - MCP default changed to 3517
- **3.3** Relay Configuration - DEFAULT_RELAY_URL constant, CLI documented
- **3.4** Block Hash - Accepts optional block_hash parameter

### Phase 4 - Cleanup (Low) - DONE
- **4.1** Dead Code Cleanup - Implemented offset param, added db_path getter
- **4.2** LNv2 Tracking - Added tracking comment with GitHub issue
- **4.3** ListTransactions Filtering - All filter params implemented (from, until, offset, unpaid, type)
- **4.4** Clock Warning - Already handled gracefully
- **4.5** Constants Extraction - Created constants.rs module

## Summary of Changes

### New Files
- `nanduti-core/src/constants.rs` - Centralized constants

### Modified Files
- `nanduti-core/src/storage.rs` - Encryption, indexing
- `nanduti-core/src/fedimint_client.rs` - Dynamic fee estimation, db_path getter
- `nanduti-core/src/lightning.rs` - Use constants
- `nanduti-core/src/nwc_protocol.rs` - Updated get_info signature
- `nanduti-core/src/lib.rs` - Export constants module
- `nanduti-api/src/nwc_handler.rs` - ListTransactions filtering, constants
- `nanduti-api/src/nostr_client.rs` - Removed keysend
- `nanduti-api/src/handlers/transactions.rs` - Full filtering support
- `nanduti-cli/src/main.rs` - Relay config docs, use constants
- `nanduti-cli/src/api_client.rs` - Added offset param
- `nanduti-cli/src/mcp_server.rs` - Removed dead_code annotations

## Test Results
All 49 tests pass.
