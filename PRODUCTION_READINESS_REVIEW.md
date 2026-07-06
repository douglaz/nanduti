# Production Readiness Review - Nanduti Multi-Federation Wallet

**Review Date:** 2026-01-02
**Reviewer:** Senior Specialist Engineer
**Scope:** Multi-federation Fedimint wallet with Nostr Wallet Connect (NWC) support

---

## Executive Summary

The nanduti codebase demonstrates solid engineering in many areas (ACID transactions, encryption, error handling) but contains **CRITICAL production blockers** that must be addressed before deployment. The most severe issues involve **missing payment authorization checks** that could allow unlimited spending through NWC connections.

**Overall Assessment:** **NOT PRODUCTION READY** - Critical security and authorization issues present.

---

## CRITICAL ISSUES (Must Fix Before Production)

### 1. **MISSING PAYMENT AUTHORIZATION AND RATE LIMITING** ⚠️

**Severity:** CRITICAL
**Risk:** Unlimited spending, DoS attacks, financial loss

**Location:** `nanduti-api/src/nwc_handler.rs`

**Problem:**
The NWC handler stores connection limits (`daily_limit_msats`, `per_payment_limit_msats`, `allowed_federations`, `allowed_methods`) but **NEVER enforces them**. Any NWC request can spend unlimited funds.

```rust
// Lines 85-166: handle_pay_invoice - NO authorization checks
async fn handle_pay_invoice(&self, params: Value) -> Result<NwcResponse> {
    // Parse and execute payment - NO checks for:
    // - daily_limit_msats
    // - per_payment_limit_msats
    // - allowed_federations
    // - allowed_methods
    // - connection lookup by pubkey

    let result = client.pay_invoice(&invoice).await?; // Direct payment!
}

// Lines 304-381: handle_pay_keysend - NO authorization checks
async fn handle_pay_keysend(&self, params: Value) -> Result<NwcResponse> {
    // Same issue - no authorization enforcement
    let result = client.pay_keysend(&params.pubkey, amount, preimage).await?;
}
```

**Impact:**
- Attacker with NWC connection can drain entire wallet balance
- No rate limiting allows DoS via excessive payment attempts
- Daily limits are completely ignored
- Per-payment limits are completely ignored
- Federation restrictions are not enforced

**Required Fix:**
```rust
async fn handle_pay_invoice(&self, params: Value) -> Result<NwcResponse> {
    // 1. Extract sender pubkey from request context
    let sender_pubkey = /* from Nostr event */;

    // 2. Load connection from storage
    let connection = self.storage
        .get_connection(&sender_pubkey)
        .ok_or(NwcErrorCode::Unauthorized)?;

    // 3. Check allowed_methods
    if !connection.allowed_methods.contains(&"pay_invoice".to_string()) {
        return Err(NwcErrorCode::Restricted);
    }

    // 4. Check allowed_federations
    if !connection.allowed_federations.contains(&"*") {
        let federation = self.router.select_federation(amount).await?;
        if !connection.allowed_federations.contains(&federation.id.to_string()) {
            return Err(NwcErrorCode::Restricted);
        }
    }

    // 5. Check per-payment limit
    if let Some(limit) = connection.per_payment_limit_msats {
        if amount.as_msats() > limit {
            return Err(NwcErrorCode::QuotaExceeded);
        }
    }

    // 6. Check daily limit (requires tracking total_spent today)
    if let Some(daily_limit) = connection.daily_limit_msats {
        let today_spent = self.get_daily_spent(&connection, Timestamp::now()).await?;
        if today_spent + amount.as_msats() > daily_limit {
            return Err(NwcErrorCode::QuotaExceeded);
        }
    }

    // 7. Execute payment
    let result = client.pay_invoice(&invoice).await?;

    // 8. Update connection.total_spent_msats and last_used
    self.storage.update_connection_spent(&connection.id, amount)?;

    Ok(NwcResponse::pay_invoice(result))
}
```

**Additional Requirements:**
- Add `get_daily_spent()` method to track spending windows
- Add `update_connection_spent()` to atomically update totals
- Add proper NWC error codes (Restricted, QuotaExceeded, Unauthorized)
- Implement same checks for `handle_pay_keysend()`, `handle_make_invoice()`
- Add tests for all authorization scenarios

---

### 2. **MISSING PUBKEY EXTRACTION FROM NWC EVENTS** ⚠️

**Severity:** CRITICAL
**Risk:** Cannot identify who is making requests, breaks entire authorization model

**Location:** `nanduti-api/src/nwc_handler.rs:50-83`

**Problem:**
The `handle_request()` method receives an `NwcRequest` struct with no pubkey field. The actual sender's pubkey is in the parent Nostr event, but it's not passed down to the handler.

```rust
// Current broken flow:
pub async fn handle_request(&self, request: NwcRequest) -> Result<NwcResponse> {
    // request has no pubkey field!
    // Cannot lookup connection
    // Cannot enforce authorization
}
```

**Required Fix:**
```rust
// Option 1: Add pubkey to request context
pub struct NwcRequestContext {
    pub request: NwcRequest,
    pub sender_pubkey: PublicKey,
    pub event_id: String,
}

pub async fn handle_request(&self, context: NwcRequestContext) -> Result<NwcResponse> {
    let sender_pubkey = &context.sender_pubkey;
    // Now can lookup connection and enforce limits
}

// Option 2: Extract from nostr_client event handling
// In nostr_client.rs:305-342, pass pubkey to handler
let response = handler.handle_request(request, event.pubkey).await?;
```

---

### 3. **UNPROTECTED STORAGE OPERATIONS** ⚠️

**Severity:** HIGH
**Risk:** Concurrent transactions could corrupt connection state

**Location:** `nanduti-core/src/storage.rs:272-279`

**Problem:**
Connection storage operations don't use ACID transactions like federation storage does:

```rust
pub fn store_connection(&self, connection: &NwcConnection) -> Result<()> {
    if let Some(tree) = &self.connections {
        let data = serde_json::to_vec(connection)?;
        tree.insert(connection.id.as_bytes(), data)?; // NOT in transaction!
    }
    Ok(())
}
```

**Impact:**
- Race condition: Two payments update `total_spent_msats` concurrently → lost updates
- Race condition: `last_used` timestamp updates could be lost
- No atomicity for read-modify-write operations on connection state

**Required Fix:**
```rust
pub fn store_connection(&self, connection: &NwcConnection) -> Result<()> {
    if let Some(tree) = &self.connections {
        let connection_clone = connection.clone();

        // Use sled's transactional API
        tree.transaction(|tx_tree| {
            let data = serde_json::to_vec(&connection_clone)
                .map_err(|_| sled::transaction::ConflictableTransactionError::Abort(()))?;

            tx_tree.insert(connection_clone.id.as_bytes(), data.as_slice())?;
            Ok::<(), sled::transaction::ConflictableTransactionError<()>>(())
        })
        .map_err(|e| anyhow::anyhow!("Connection store failed: {e:?}"))?;
    }
    Ok(())
}

// Add atomic increment for spent tracking
pub fn increment_connection_spent(&self, connection_id: &str, amount: Amount) -> Result<()> {
    if let Some(tree) = &self.connections {
        tree.transaction(|tx_tree| {
            // Read current connection
            let data = tx_tree.get(connection_id.as_bytes())?
                .ok_or(sled::transaction::ConflictableTransactionError::Abort(()))?;

            let mut connection: NwcConnection = serde_json::from_slice(&data)
                .map_err(|_| sled::transaction::ConflictableTransactionError::Abort(()))?;

            // Update spent amount atomically
            connection.total_spent_msats += amount.as_msats();
            connection.last_used = Some(Timestamp::now().as_secs());

            // Write back
            let updated_data = serde_json::to_vec(&connection)
                .map_err(|_| sled::transaction::ConflictableTransactionError::Abort(()))?;

            tx_tree.insert(connection_id.as_bytes(), updated_data.as_slice())?;
            Ok::<(), sled::transaction::ConflictableTransactionError<()>>(())
        })
        .map_err(|e| anyhow::anyhow!("Connection update failed: {e:?}"))?;
    }
    Ok(())
}
```

---

### 4. **UNBOUNDED TRANSACTION QUERIES** ⚠️

**Severity:** HIGH
**Risk:** Memory exhaustion, DoS via resource consumption

**Location:** `nanduti-core/src/storage.rs:203-232`

**Problem:**
Transaction queries iterate entire database without pagination controls:

```rust
pub fn get_federation_transactions(
    &self,
    federation_id: &FederationId,
    limit: Option<usize>, // Limit applied AFTER loading all into memory!
) -> Result<Vec<Transaction>> {
    let mut transactions = Vec::new();

    if let Some(tree) = &self.transactions {
        for item in tree.iter() { // Iterates ENTIRE database
            let transaction: Transaction = serde_json::from_slice(&value)?;

            if transaction.federation_id == *federation_id {
                transactions.push(transaction); // Loads all matching into memory

                if let Some(limit) = limit {
                    if transactions.len() >= limit {
                        break; // Only helps with memory, not I/O
                    }
                }
            }
        }
    }

    // Sorts ENTIRE result set in memory
    transactions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(transactions)
}
```

**Impact:**
- Wallet with 100,000 transactions: loads all into memory to return 10
- No index on `federation_id` → full table scan every time
- `list_transactions` in `nwc_handler.rs:231-256` calls this for EVERY federation → amplified problem
- Attacker can trigger memory exhaustion via `list_transactions` NWC request

**Required Fix:**

Option 1: Use proper indexing with sled's prefix scan:
```rust
// Store transactions with composite key: federation_id + timestamp + tx_id
// Key format: "{federation_id}:{reverse_timestamp}:{tx_id}"

pub fn get_federation_transactions(
    &self,
    federation_id: &FederationId,
    limit: Option<usize>,
) -> Result<Vec<Transaction>> {
    let mut transactions = Vec::new();

    if let Some(tree) = &self.transactions {
        let prefix = format!("{}:", federation_id.as_str());
        let limit = limit.unwrap_or(100).min(1000); // Cap at 1000

        for item in tree.scan_prefix(prefix.as_bytes()).take(limit) {
            let (_, value) = item?;
            let transaction: Transaction = serde_json::from_slice(&value)?;
            transactions.push(transaction);
        }
    }

    Ok(transactions) // Already sorted by key
}
```

Option 2: Add hard limit and warning:
```rust
pub fn get_federation_transactions(
    &self,
    federation_id: &FederationId,
    limit: Option<usize>,
) -> Result<Vec<Transaction>> {
    const MAX_LIMIT: usize = 1000;
    let limit = limit.unwrap_or(100).min(MAX_LIMIT);
    let mut transactions = Vec::new();
    let mut scanned = 0;
    const MAX_SCAN: usize = 10_000; // Safety limit

    if let Some(tree) = &self.transactions {
        for item in tree.iter() {
            scanned += 1;
            if scanned > MAX_SCAN {
                warn!("Transaction scan exceeded {MAX_SCAN} items, aborting");
                break;
            }

            let (_, value) = item?;
            let transaction: Transaction = serde_json::from_slice(&value)?;

            if transaction.federation_id == *federation_id {
                transactions.push(transaction);
                if transactions.len() >= limit {
                    break;
                }
            }
        }
    }

    transactions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(transactions)
}
```

---

### 5. **PAYMENT HASH COLLISION VULNERABILITY** ⚠️

**Severity:** MEDIUM
**Risk:** Transaction lookup failures, duplicate payment detection issues

**Location:** `nanduti-core/src/storage.rs:235-251`

**Problem:**
Transaction ID uses UUID but payment_hash from invoice is stored. Multiple transactions can have the same payment_hash (retries, duplicate invoices):

```rust
pub fn get_transaction_by_payment_hash(&self, payment_hash: &str) -> Result<Option<Transaction>> {
    if let Some(tree) = &self.transactions {
        for item in tree.iter() { // Full table scan!
            let transaction: Transaction = serde_json::from_slice(&value)?;

            if transaction.payment_hash.as_str() == payment_hash {
                return Ok(Some(transaction)); // Returns FIRST match only!
            }
        }
    }
    Ok(None)
}
```

**Impact:**
- Multiple payments with same invoice → only finds first
- Full table scan for every lookup (no index)
- `lookup_invoice` in `nwc_handler.rs:383-445` relies on this → broken results

**Required Fix:**
```rust
// Option 1: Index by payment_hash using separate tree
pub fn get_transactions_by_payment_hash(&self, payment_hash: &str) -> Result<Vec<Transaction>> {
    // Store in payment_hash_index tree: hash -> Vec<transaction_id>
    // Then fetch each transaction by ID
}

// Option 2: Use composite keys and return all matches
pub fn get_transactions_by_payment_hash(&self, payment_hash: &str) -> Result<Vec<Transaction>> {
    let mut transactions = Vec::new();

    if let Some(tree) = &self.transactions {
        for item in tree.iter() {
            let (_, value) = item?;
            let transaction: Transaction = serde_json::from_slice(&value)?;

            if transaction.payment_hash.as_str() == payment_hash {
                transactions.push(transaction); // Collect ALL matches
            }
        }
    }

    // Return most recent if multiple found
    transactions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(transactions)
}
```

---

## HIGH PRIORITY ISSUES

### 6. **NO TIMEOUT ON NOSTR EVENT POLLING**

**Severity:** HIGH
**Risk:** Infinite blocking, resource leaks

**Location:** `nanduti-api/src/nostr_client.rs:250-301`

**Problem:**
```rust
loop {
    match self.client.database().query(filter.clone()).await {
        Ok(events) => { /* process */ }
        Err(e) => { /* backoff */ }
    }

    tokio::time::sleep(Duration::from_millis(500)).await; // No overall timeout!
}
```

**Impact:**
- Event loop runs forever with no cancellation mechanism
- Cannot gracefully shutdown
- No timeout on `query()` call itself

**Required Fix:**
```rust
pub async fn handle_nwc_events(&self, handler: Arc<crate::NwcHandler>) -> Result<()> {
    // Add shutdown signal
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel(1);

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("Shutting down NWC event handler");
                break;
            }
            result = tokio::time::timeout(
                Duration::from_secs(30),
                self.client.database().query(filter.clone())
            ) => {
                match result {
                    Ok(Ok(events)) => { /* process */ }
                    Ok(Err(e)) => { /* handle error */ }
                    Err(_) => {
                        warn!("Query timeout after 30s");
                        continue;
                    }
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    Ok(())
}
```

---

### 7. **UNWRAP IN TEST CODE COULD MASK ISSUES**

**Severity:** MEDIUM
**Risk:** Tests pass when they should fail, false confidence

**Location:** Multiple test files

**Problem:**
```rust
// nanduti-core/src/mnemonic_store.rs:274
assert_eq!(loaded_mnemonic.unwrap().to_string(), ...); // unwrap hides errors

// nanduti-api/src/handlers/test_*.rs - multiple files
let storage = Arc::new(Storage::new(None).unwrap());
let nostr_client = Arc::new(NostrClient::new(vec![], None).await.unwrap());
```

**Required Fix:**
```rust
// Use ? operator in tests (tests return Result<()>)
assert_eq!(loaded_mnemonic?.to_string(), ...);

let storage = Arc::new(Storage::new(None)?);
let nostr_client = Arc::new(NostrClient::new(vec![], None).await?);
```

---

### 8. **HARDCODED EVENT CACHE SIZE**

**Severity:** MEDIUM
**Risk:** Memory exhaustion with high event volume

**Location:** `nanduti-api/src/nostr_client.rs:228`

**Problem:**
```rust
const EVENT_CACHE_CAPACITY: NonZeroUsize = NonZeroUsize::new(10000).unwrap();
let mut processed_events = LruCache::new(EVENT_CACHE_CAPACITY);
```

**Impact:**
- 10,000 events × ~500 bytes = 5MB minimum
- No consideration for high-traffic scenarios
- Could be too small (duplicate processing) or too large (memory waste)

**Required Fix:**
```rust
// Make configurable
const DEFAULT_EVENT_CACHE_SIZE: usize = 10_000;
let cache_size = std::env::var("NANDUTI_EVENT_CACHE_SIZE")
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(DEFAULT_EVENT_CACHE_SIZE);

let mut processed_events = LruCache::new(
    NonZeroUsize::new(cache_size).unwrap_or(NonZeroUsize::new(1000).unwrap())
);
```

---

### 9. **SYSTEMTIME FALLBACK TO EPOCH 0**

**Severity:** MEDIUM
**Risk:** Incorrect timestamps in edge cases

**Location:** Multiple files

**Problem:**
```rust
// nanduti-core/src/models.rs:418
Timestamp::now() -> Self {
    Self(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(0) // If clock is before 1970, use 0!
    )
}

// nanduti-api/src/handlers/nwc.rs:76-79
created_at: SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_secs())
    .unwrap_or(0), // Same issue
```

**Impact:**
- If system clock is incorrect, creates timestamp 0 (Jan 1, 1970)
- Transactions appear to be from 1970
- Sorting by timestamp breaks completely
- Daily limits calculated incorrectly

**Required Fix:**
```rust
pub fn now() -> Result<Self> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock is before UNIX epoch (1970-01-01). Please check system time.")?
        .as_secs();

    // Sanity check: timestamp should be after 2020
    const MIN_VALID_TIMESTAMP: u64 = 1577836800; // 2020-01-01
    if secs < MIN_VALID_TIMESTAMP {
        bail!("System clock appears to be incorrect (timestamp {secs} is before 2020)");
    }

    Ok(Self(secs))
}
```

---

## MEDIUM PRIORITY ISSUES

### 10. **MISSING TRANSACTION STATE TRANSITIONS**

**Severity:** MEDIUM
**Risk:** Incomplete transaction tracking

**Location:** `nanduti-api/src/nwc_handler.rs`

**Problem:**
Payment failures don't update transaction state:

```rust
// Lines 118-136: Store Pending transaction
storage.store_transaction(&transaction)?;

// Lines 144: Execute payment (might fail!)
let result = client.pay_invoice(&invoice).await?; // If fails, transaction stays Pending forever!
```

**Required Fix:**
```rust
// Store pending transaction
storage.store_transaction(&transaction)?;

// Execute payment with proper state handling
let result = match client.pay_invoice(&invoice).await {
    Ok(result) => result,
    Err(e) => {
        // Update transaction to Failed state
        let failed_transaction = Transaction {
            state: TransactionState::Failed,
            metadata: Some(json!({"error": e.to_string()})),
            ..transaction
        };
        storage.store_transaction(&failed_transaction)?;
        return Err(e);
    }
};

// Update to Settled
storage.store_transaction(&settled_transaction)?;
```

---

### 11. **NO DUPLICATE PAYMENT PROTECTION**

**Severity:** MEDIUM
**Risk:** Double payments, idempotency violations

**Location:** `nanduti-api/src/nwc_handler.rs:85-166`

**Problem:**
Same invoice can be paid multiple times:

```rust
async fn handle_pay_invoice(&self, params: Value) -> Result<NwcResponse> {
    let invoice = LightningOperation::parse_invoice(params.invoice.as_str())?;

    // No check for existing payment!
    // Someone could submit same NWC request multiple times

    let result = client.pay_invoice(&invoice).await?;
}
```

**Required Fix:**
```rust
async fn handle_pay_invoice(&self, params: Value) -> Result<NwcResponse> {
    let invoice = LightningOperation::parse_invoice(params.invoice.as_str())?;

    // Check for existing payment
    if let Some(storage) = &self.storage {
        if let Some(existing) = storage.get_transaction_by_payment_hash(
            invoice.payment_hash.as_str()
        )? {
            match existing.state {
                TransactionState::Settled => {
                    // Already paid!
                    return Ok(NwcResponse::error(
                        "pay_invoice",
                        NwcErrorCode::AlreadyPaid,
                        "Invoice already paid"
                    ));
                }
                TransactionState::Pending => {
                    // Payment in progress
                    return Ok(NwcResponse::error(
                        "pay_invoice",
                        NwcErrorCode::PaymentInProgress,
                        "Payment already in progress"
                    ));
                }
                _ => {}
            }
        }
    }

    let result = client.pay_invoice(&invoice).await?;
    // ... rest of implementation
}
```

---

### 12. **UNBOUNDED PARALLEL FEDERATION UPDATES**

**Severity:** MEDIUM
**Risk:** Resource exhaustion during bulk operations

**Location:** `nanduti-api/src/nwc_handler.rs:231-256`

**Problem:**
```rust
async fn handle_list_transactions(&self, params: Value) -> Result<NwcResponse> {
    // Get transactions from ALL federations
    for federation in self.federation_manager.list_federations().await {
        let transactions = storage.get_federation_transactions(&federation.id, limit)?;
        all_transactions.extend(transactions);
    }
}
```

**Impact:**
- Wallet with 100 federations: launches 100 concurrent database scans
- No concurrency control
- Memory spike from loading all results

**Required Fix:**
```rust
use futures::stream::{self, StreamExt};

async fn handle_list_transactions(&self, params: Value) -> Result<NwcResponse> {
    let mut all_transactions = Vec::new();
    let federations = self.federation_manager.list_federations().await;

    // Process in batches of 10
    const BATCH_SIZE: usize = 10;

    for chunk in federations.chunks(BATCH_SIZE) {
        let chunk_results: Vec<_> = stream::iter(chunk)
            .then(|federation| async move {
                self.storage
                    .as_ref()
                    .and_then(|s| s.get_federation_transactions(&federation.id, params.limit).ok())
                    .unwrap_or_default()
            })
            .collect()
            .await;

        for txs in chunk_results {
            all_transactions.extend(txs);
        }
    }

    // ... rest of implementation
}
```

---

## LOW PRIORITY ISSUES

### 13. **PARTIAL_CMP UNWRAP IN ROUTING**

**Severity:** LOW
**Risk:** Panic on NaN in metrics

**Location:** `nanduti-api/src/router.rs:130-135`

**Problem:**
```rust
.max_by(|a, b| {
    let a_score = a.metrics.success_rate * a.metrics.uptime_percent;
    let b_score = b.metrics.success_rate * b.metrics.uptime_percent;
    a_score
        .partial_cmp(&b_score)
        .unwrap_or(std::cmp::Ordering::Equal) // Could panic if NaN
})
```

**Required Fix:**
```rust
.max_by(|a, b| {
    let a_score = a.metrics.success_rate * a.metrics.uptime_percent;
    let b_score = b.metrics.success_rate * b.metrics.uptime_percent;

    // Handle NaN explicitly
    match (a_score.is_nan(), b_score.is_nan()) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (false, false) => a_score.partial_cmp(&b_score).unwrap_or(std::cmp::Ordering::Equal),
    }
})
```

---

### 14. **MISSING TESTS FOR CRITICAL PATHS**

**Severity:** LOW
**Risk:** Regression in production features

**Missing Test Coverage:**
- ✗ Payment authorization (daily limit enforcement)
- ✗ Payment authorization (per-payment limit enforcement)
- ✗ Payment authorization (federation restrictions)
- ✗ Connection state updates (total_spent tracking)
- ✗ Duplicate payment prevention
- ✗ Transaction state transitions (Pending → Failed)
- ✗ Concurrent transaction queries
- ✗ Event cache overflow behavior
- ✗ Nostr event decryption failures
- ✗ Invalid timestamp handling

**Required:**
Add integration tests for all authorization scenarios.

---

## POSITIVE OBSERVATIONS

**What's Working Well:**

1. **ACID Guarantees:** Federation and transaction storage correctly uses sled transactions (lines storage.rs:89-98, 180-189)

2. **Encryption Security:** Mnemonic storage uses proper AES-256-GCM with Argon2id KDF (mnemonic_store.rs:10-246)

3. **Circuit Breaker:** Nostr client has proper exponential backoff (nostr_client.rs:244-295)

4. **Concurrency Safety:** Federation updates use atomic entry API to prevent races (federation.rs:281-288)

5. **Error Context:** Good use of `.context()` for debugging

6. **Type Safety:** Strong newtype wrappers prevent domain errors (models.rs)

---

## RECOMMENDATIONS

### Immediate Actions (Before Production)

1. **Implement Payment Authorization** (Issue #1, #2)
   - Add connection lookup by pubkey
   - Enforce all limits (daily, per-payment, federation, methods)
   - Add atomic spent tracking

2. **Fix Storage Race Conditions** (Issue #3)
   - Use transactions for all connection updates
   - Add atomic increment operations

3. **Add Query Limits** (Issue #4)
   - Implement pagination or hard caps
   - Add monitoring for query performance

4. **Add Comprehensive Tests**
   - Authorization edge cases
   - Concurrent payment scenarios
   - Limit enforcement

### Short-term Improvements

5. **Add Monitoring**
   - Payment success/failure rates
   - Database query latencies
   - Connection spent totals
   - Failed authorization attempts

6. **Add Alerting**
   - High error rates
   - Approaching daily limits
   - Unusual spending patterns

7. **Add Admin Tools**
   - Force disconnect connections
   - Reset spent totals
   - Emergency payment blocking

### Long-term Enhancements

8. **Performance Optimization**
   - Add database indexes (federation_id, payment_hash, timestamp)
   - Consider switching from sled to SQLite for better query support
   - Add caching layer for frequently accessed data

9. **Feature Additions**
   - Webhook notifications for payments
   - Multi-sig authorization for large payments
   - Spending analytics dashboard

---

## CONCLUSION

**Current State:** The nanduti codebase has solid foundations (ACID, encryption, error handling) but contains **critical security holes** that make it unsafe for production use.

**Blockers:**
- Missing payment authorization (CRITICAL)
- Missing pubkey extraction (CRITICAL)
- Unprotected connection updates (HIGH)
- Unbounded queries (HIGH)

**Timeline Estimate:**
- Fix critical issues: 3-5 days
- Add comprehensive tests: 2-3 days
- Security audit: 1-2 days
- **Total:** 1-2 weeks before production-ready

**Risk Assessment:** **HIGH** - Do not deploy without fixing critical authorization issues.

---

**Next Steps:**
1. Fix Issues #1 and #2 immediately
2. Add authorization tests
3. Conduct security review
4. Perform load testing
5. Deploy to staging with monitoring
6. Production deployment after 1 week of stable staging operation
