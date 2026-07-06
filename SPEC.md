# Nanduti Product and Architecture Spec

Status: Draft source-of-truth spec  
Last updated: 2026-07-06  
Branch context: `initial-release`

## Purpose

Nanduti is a headless wallet service for exposing one or more Fedimint wallets
through Nostr Wallet Connect (NWC). It lets an operator join Fedimint
federations, create scoped NWC connections for apps, and route Lightning
payments or invoices through the available federation set.

The product should be understood as:

- A local/server-side wallet daemon.
- An NWC provider that speaks NIP-47 over Nostr.
- A Fedimint-backed Lightning wallet aggregator.
- A CLI and REST control plane for operators.
- An optional MCP control surface for trusted automation.

The product should not yet be described as a complete generic multi-backend
wallet platform. Fedimint is the only implemented backend. Other backends are a
future extension point, not current product scope.

## One-Sentence Vision

Run one wallet service, connect it to multiple Fedimint federations, and hand
NWC-compatible apps scoped connection strings that can spend or receive only
within operator-defined limits.

## Product Principles

1. Payment safety beats convenience.
   A failed, duplicated, unauthorized, or over-limit payment path is more severe
   than a rejected request.

2. NWC is the data plane.
   Nostr events from external apps carry money-moving requests and must enforce
   connection identity, permissions, limits, deduplication, and transaction
   recording before payment execution.

3. REST and CLI are the operator control plane.
   REST/CLI are for federation management, connection issuance, manual
   operations, and inspection. If exposed off-loopback, REST must be protected by
   a bearer secret or a stronger deployment-layer control.

4. Persistence is part of correctness.
   Connection state, transaction state, event deduplication, pending invoice
   watchers, and wallet secrets must survive restarts in production mode.

5. Capability advertising must be truthful.
   The Nostr info event and `get_info` response must only advertise methods and
   notifications that are usable through the active backend set.

6. Backend support must be explicit.
   Until a backend trait and second real backend exist, docs and CLI should say
   "Fedimint-backed" rather than imply broad backend support.

## Personas

### Wallet Operator

Runs the daemon, joins federations, configures relays, manages API secrets,
creates NWC connections, monitors balances, and handles recovery.

Needs:

- Reliable startup and restart behavior.
- Clear errors when secrets or persistent data are missing.
- Safe defaults for local vs exposed deployments.
- A way to inspect federations, gateways, transactions, balances, and
  connection spending.

### NWC App User

Pastes a generated NWC URI into apps such as Zeus or Alby.

Needs:

- A valid NWC URI that points to relays the daemon is subscribed to.
- Predictable method support.
- Correct NWC errors for restricted, over-limit, duplicate, or failed requests.

### Automation Client

Uses the CLI, REST API, or MCP server to operate a trusted local Nanduti
instance.

Needs:

- Stable command/API contracts.
- Machine-readable output.
- Authentication when operating over HTTP.
- No hidden requirement to inspect implementation details.

## Current System Shape

The repository is a Rust workspace with three crates:

- `nanduti-core`: domain types, storage, key generation, Fedimint client wrapper,
  Lightning parsing, and NWC protocol models.
- `nanduti-api`: HTTP server, Nostr client, NWC request handler, routing, app
  state, and REST handlers.
- `nanduti-cli`: command-line interface, REST client, and optional MCP server.

Current major runtime components:

- `AppState`: owns storage, federation manager, router, Nostr client, NWC
  handler, relays, and server-level limits.
- `FederationManager`: loads, adds, removes, tracks, and refreshes Fedimint
  federations.
- `FederationRouter`: selects a federation for outgoing or incoming operations.
- `NostrClient`: publishes NWC info, listens for NWC request events, decrypts
  requests, sends encrypted responses, and maintains processed-event dedupe.
- `NwcHandler`: authorizes NWC requests and performs money-moving or query
  operations.
- `Storage`: sled-backed persistence for federations, NWC connections,
  transactions, secondary indexes, and processed Nostr events.
- `FedimintClientWrapper`: wraps Fedimint client operations such as balance,
  invoice payment, invoice creation, gateway selection, and invoice settlement
  subscription.

## Supported Deployment Modes

### Persistent Production-Like Mode

Configured with `--data-dir`.

Required:

- `NANDUTI_MNEMONIC_PASSWORD` must be set and non-empty.
  - Current gap: bare `serve` startup only checks that the variable is present.
    Non-emptiness is enforced later, when a Fedimint client is created, so an
    empty password is currently accepted for storage-key and Nostr-key
    derivation at startup. Rejecting an empty password at startup is on the
    hardening checklist.
- The password encrypts stored mnemonics.
- The service-level mnemonic controls the storage encryption key and wallet
  Nostr key.
- Each persistent Fedimint client also has federation-local wallet state under
  its federation data directory.
- Existing storage without the original `.mnemonic` file must fail startup
  rather than silently generating a new key.

Recommended:

- `API_SECRET` when binding to anything other than loopback.
- Persistent volume for the data directory.
- Explicit relay list.
- Regular backups of the whole data directory, including the root `.mnemonic`
  and any federation-local `.mnemonic` files.

### Ephemeral Development Mode

Configured without `--data-dir`.

Behavior:

- Uses temporary sled storage for the process lifetime.
- Uses ephemeral Fedimint client state and Nostr keys.
- Does not require `NANDUTI_MNEMONIC_PASSWORD`.
- NWC connection URIs and wallet identity do not survive restart.

This mode is for local testing only.

## Product Scope

### MVP Scope

The MVP should be Fedimint-backed NWC with:

- Join/list/show/remove Fedimint federations.
- Track per-federation balance and status.
- Discover available Lightning gateways.
- Create NWC connections with scoped limits and federation filters.
- Publish truthful NWC info events.
- Handle NWC `pay_invoice`, `make_invoice`, `get_balance`, `list_transactions`,
  `lookup_invoice`, and `get_info`.
- Reject unsupported NWC methods cleanly.
- Route outgoing payments by strategy and invoice network.
- Route incoming invoices to an allowed online federation.
- Persist and recover critical wallet state.
- Provide CLI and REST control-plane access.

### Out of Scope for MVP

- Generic backend support beyond Fedimint.
- Keysend as an advertised capability.
- Multi-pay methods.
- Push notifications until the server actually emits them in all advertised
  cases.
- Mainnet production claims without completing the hardening checklist.
- Hosted multi-tenant service mode.
- Browser UI.
- Automatic backup management.

### Future Scope

- A real backend abstraction and second backend implementation.
- Keysend support if a backend can perform it safely.
- Multi-pay methods with batch-level idempotency and partial-failure semantics.
- Full notification support for incoming and outgoing settlements.
- Better transaction indexing for large stores.
- Gateway health scoring and smarter routing.
- Web UI for operators.
- Structured audit logs.

## NWC Capability Contract

Nanduti must distinguish three categories:

1. Parsed methods:
   Methods the protocol model can deserialize.

2. Handled methods:
   Methods the handler has code paths for.

3. Advertised methods:
   Methods the server tells clients are supported.

Only advertised methods are part of the public NWC contract.

Current advertised methods:

| Method | Status | Notes |
| --- | --- | --- |
| `pay_invoice` | Supported | Requires fixed-amount BOLT11 invoice. Amountless invoices are rejected. |
| `make_invoice` | Supported | Uses Fedimint invoice creation and settlement watcher. |
| `get_balance` | Supported | Sums online federations visible to the connection. |
| `list_transactions` | Supported | Scoped to the connection. |
| `lookup_invoice` | Supported | Scoped to the connection by transaction metadata. |
| `get_info` | Supported | Must report truthful methods and network info. |

Currently parsed but not advertised:

| Method | Status | Notes |
| --- | --- | --- |
| `pay_keysend` | Not supported by Fedimint backend | Handler exists, but backend returns unsupported. Do not advertise until backend can perform it. |
| `multi_pay_invoice` | Not implemented | Return `NOT_IMPLEMENTED`. |
| `multi_pay_keysend` | Not implemented | Return `NOT_IMPLEMENTED`. |

Notifications:

- Do not advertise notification support until notifications are emitted
  consistently.
- Settlement watchers update local transaction state today; that is not the same
  as client notification support.

## Control-Plane Contract

### CLI Commands

The CLI is the primary operator interface.

Expected command groups:

- `serve`: run the daemon.
- `health`: check HTTP health.
- `fm-*`: manage federations.
- `nwc-*`: create and list NWC connections.
- `tx-*`: list transactions, pay invoices manually, and create invoices.
- `mcp-server`: optional MCP server when compiled with `mcp`.

CLI output should support:

- Human-readable table output for operators.
- JSON output for scripts where currently exposed.
- Clear error messages with context.

### REST API

REST endpoints live under `/api/v1/*` and should be treated as trusted
operator/admin endpoints.

Current endpoint groups:

- `/health`: unauthenticated health check.
- `/api/v1/federations`: list/add federations.
- `/api/v1/federations/{id}`: show/remove federation.
- `/api/v1/federations/{id}/balance`: get federation balance.
- `/api/v1/federations/{id}/gateways`: list gateways.
- `/api/v1/invoices`: create invoice.
- `/api/v1/payments`: pay invoice manually.
- `/api/v1/transactions`: list transactions.
- `/api/v1/nwc/connections`: create/list NWC connections.

Authentication:

- If `API_SECRET` is configured, all `/api/v1/*` routes require
  `Authorization: Bearer <secret>`.
- If binding to non-loopback and `API_SECRET` is missing, startup must warn.
- Future production deployments should fail closed unless an explicit
  `--allow-unauthenticated-api` escape hatch is added.

### MCP

MCP is a trusted local automation surface over stdio. It wraps the REST API.

MCP tools should not bypass REST authentication or safety rules. When the REST
API requires `API_SECRET`, MCP clients must use the same configured secret path
through the API client.

## NWC Request Flow

1. The daemon derives or generates its wallet Nostr key.
2. The daemon connects to configured relays.
3. The daemon publishes a kind `13194` NWC info event.
4. The daemon subscribes to kind `23194` requests addressed to its wallet pubkey.
5. The event loop maintains an in-memory LRU and persistent processed-event set.
6. Each request event is decrypted with NIP-44.
7. The raw request is wrapped with sender pubkey and event id.
8. The handler authorizes the sender against stored NWC connections.
9. The handler validates method permissions, limits, federation scope, and
   duplicate/idempotency state.
10. For money-moving operations, the handler writes a Pending transaction before
   calling the backend.
11. The backend operation executes.
12. The transaction is updated to Settled or Failed where possible.
13. The response is encrypted with NIP-44 and sent as kind `23195`.
14. The event is marked processed even if response publishing fails after the
   handler completed, to avoid replaying side effects.

## Payment Safety Invariants

These are hard requirements. A change that weakens one must be treated as a
security regression.

### Authorization

- Every advertised NWC method must require a valid connection for the event
  sender pubkey.
- Each connection has an allowed-method filter.
- Each connection has an allowed-federation filter.
- NWC transaction queries must only return transactions for the requesting
  connection.
- REST manual payments are operator actions and are protected by REST auth, not
  NWC connection limits.

### Limits

- Per-payment limits apply before payment execution.
- Daily limits apply before payment execution.
- The "day" is a fixed UTC calendar day (`00:00:00` UTC to the next
  `00:00:00` UTC), and a transaction is assigned to a day by its `created_at`
  timestamp, not `settled_at`. This keeps a reservation counted against the day
  it was made even if it settles after midnight. It is a calendar-day window,
  not a rolling 24-hour window.
- Daily limit calculations must count Pending and Settled outgoing payments.
- Failed/expired outgoing payments must not keep consuming daily quota.
- Fee margin should be reserved before payment so fees do not bypass limits.
  The reserved margin is `max(2% of amount, 10_000 msat)`.
- Server-level limits are hard caps on connection-level limits.
  - Current semantics: this cap is applied when a connection is created (each
    requested limit is clamped to the server limit via `min`) and is not
    re-evaluated per payment. Lowering a server limit therefore does not
    retroactively tighten connections that already exist.

### Idempotency and Duplicate Prevention

- `pay_invoice` dedupes by BOLT11 payment hash.
- Outgoing duplicate checks must ignore incoming transactions with the same hash.
- Pending outgoing transactions block duplicate payment attempts.
- Settled outgoing transactions return Already Paid.
- In-flight payment keys protect concurrent duplicate requests within a process.
- Persistent transaction state protects against cross-restart duplicates where
  the payment hash or idempotency key is known.
- Keysend must not be advertised until it has a robust backend and idempotency
  contract.

### Persistence Before Effects

- Money-moving operations must write a Pending transaction before invoking the
  backend.
- If the Pending write fails, the payment must not be attempted.
- If a payment succeeds but final persistence fails, return success only if
  retrying would risk duplicate spend, and log enough context for reconciliation.
- Startup must reconcile stale Pending outgoing transactions conservatively.

### Network Correctness

- Pay a BOLT11 invoice only through a federation matching the invoice network.
- Unknown or unparseable network metadata should eventually fail closed rather
  than defaulting to mainnet.
  - Current behavior: an invoice whose currency prefix is not recognized is
    mapped to `Mainnet` and routed through a mainnet federation. This is
    fail-open-to-mainnet, not fail-closed, and is tracked on the hardening
    checklist.
- `get_info` must not fabricate healthy chain data. Unknown block hash/height
  should be represented honestly.
  - Current behavior: block hash is an all-zeros placeholder (honest
    "unavailable"), and block height is the real value from an online allowed
    federation or `0` when none is available. When no online allowed federation
    exists, `get_info` reports network `Mainnet` with height `0`; whether that
    default is correct is Open Question 7.

### Time Correctness

- Quota and audit paths must not silently fall back to epoch `0` on system clock
  errors.
- Startup recovery and stale-pending expiration must handle clock errors
  explicitly.
- Tests should pin time-sensitive behavior where feasible.

## Routing Requirements

Supported strategies:

- `lowest-fee`: choose the online, allowed, network-compatible federation with
  the lowest fee estimate.
- `best-route`: choose by success and uptime metrics. Current caveat: the
  `success_rate` and `uptime_percent` metrics are static placeholders fixed at
  `100.0` and are never updated from real payment outcomes (no code path calls
  the metric-update routine), so `best-route` does not currently differentiate
  federations. See the "Present But Not Product-Ready" table and Open
  Question 9.
- `round-robin`: spread payments across eligible federations.
- `balance-weighted`: choose probabilistically by balance.

Common filtering before strategy:

- Federation status must be online.
- Federation must have balance greater than requested amount plus fee margin for
  outgoing payments.
- Federation must pass the connection's allowed-federation filter.
- Federation must match invoice network when a BOLT11 network is known.

Receive routing:

- Select an online allowed federation.
- Prefer the federation with the highest balance under the current implementation.
- This heuristic should be revisited; receive routing may eventually prefer
  lower inbound risk, better gateway hints, or explicit operator policy.

## Storage Model

Persistent storage uses sled with separate trees for:

- Federations.
- NWC connections.
- Transactions.
- Transaction-by-payment-hash index.
- Transaction-by-invoice index.
- Connection-by-pubkey index.
- Processed Nostr events.

Encryption:

- Transaction data is encrypted with AES-256-GCM when an encryption key is
  configured.
- Transaction index values are encrypted when encryption is enabled.
- Transaction index keys are hashed when encryption is enabled.
- Connection and federation records are currently not described as encrypted in
  the same way; this should be reviewed before production claims.

Retention:

- The processed-events tree grows on every handled Nostr event. A pruning
  routine exists but currently has no caller, so on a long-running daemon this
  tree grows without bound. Wiring up periodic pruning is on the hardening
  checklist.

Important records:

- `Federation`: id, name, invite code, balance, status, network, metrics, client.
- `NwcConnection`: id, name, pubkey, allowed federations, allowed methods, limits,
  created_at, last_used, total_spent_msats.
- `Transaction`: id, federation id, direction, state, invoice, description,
  preimage, payment hash, amount, fees, timestamps, metadata.

Metadata conventions:

- NWC-created transactions should include `connection_id`.
- NWC-created transactions should include `sender_pubkey`.
- Incoming invoice transactions should include `operation_id` when available.
- Keysend attempts include `keysend_dedupe_key` if keysend remains in code.

## Startup and Recovery

On startup:

1. Create or open storage.
2. Load or generate mnemonic according to deployment mode.
3. Refuse to generate a new mnemonic if persistent storage exists without the
   expected mnemonic file.
4. Derive storage encryption key.
5. Derive persistent wallet Nostr key.
6. Load stored federations and reinitialize clients.
7. Refresh balances and federation metadata where possible.
8. Expire stale Pending outgoing transactions after a conservative window.
9. Re-subscribe settlement watchers for Pending incoming invoices that have an
   operation id.
10. Seed federations from startup invite codes.
11. Publish NWC info event.
12. Start Nostr NWC event loop.
13. Start REST server.

Known recovery limitation:

- Fedimint outgoing payment operation recovery is not fully represented in local
  transaction metadata. A crash after payment submission but before local
  settlement update can leave a Pending outgoing transaction. Current startup
  expiration avoids permanent blocking, but may not prove whether the underlying
  payment eventually settled. This must remain explicit in docs and risk notes.
- The current explicit "missing mnemonic with existing storage" guard is
  service-storage oriented. Federation-local client database recovery should get
  the same level of protection before production claims.

## Backend Abstraction Direction

The current code has a Fedimint wrapper, not a general backend abstraction.

Before claiming multi-backend support, introduce an explicit backend interface
with at least:

- Backend id and display name.
- Network.
- Balance query.
- Outgoing invoice payment.
- Incoming invoice creation.
- Invoice settlement subscription or polling.
- Gateway/channel metadata if applicable.
- Fee estimation.
- Capability reporting.
- Recovery hooks.
- Idempotency requirements.

Then implement:

- Fedimint backend using the current wrapper.
- A second real backend or a meaningful test backend that exercises the trait.

Only after that should docs say "multi-backend" as a shipped capability.

## Feature Inventory

### Shipping or Near-Shipping

| Feature | Surface | Status |
| --- | --- | --- |
| Start daemon | CLI | Implemented |
| Persistent data dir | CLI/API | Implemented |
| Encrypted mnemonic storage | Core | Implemented |
| Derived storage key | API/Core | Implemented |
| Derived Nostr wallet key | API/Core | Implemented |
| Add/list/show/remove federations | REST/CLI | Implemented |
| Federation balances | REST/CLI | Implemented |
| Gateway listing | REST/CLI | Implemented |
| Routing strategies | API | Implemented |
| Create NWC connection | REST/CLI | Implemented |
| List NWC connections | REST/CLI | Implemented |
| NWC pay_invoice | Nostr | Implemented with fixed-amount invoices |
| NWC make_invoice | Nostr | Implemented |
| NWC get_balance | Nostr | Implemented |
| NWC list_transactions | Nostr | Implemented |
| NWC lookup_invoice | Nostr | Implemented |
| NWC get_info | Nostr | Implemented with known caveats |
| Manual invoice payment | REST/CLI | Implemented |
| Manual invoice creation | REST/CLI | Implemented |
| Transaction listing | REST/CLI/NWC | Implemented with scalability caveats |
| MCP tools | MCP | Optional feature |

### Present But Not Product-Ready

| Feature | Issue |
| --- | --- |
| Keysend handler | Backend cannot perform keysend; do not advertise. |
| Notification helpers | Helpers exist, but notifications are not advertised and not wired as a complete behavior. |
| Multi-backend messaging | Architecture is not yet backend-generic. |
| Docker README | Contains stale command names and missing required env vars. |
| Production readiness review | Contains stale findings mixed with still-relevant concerns. |
| Transaction queries | Some paths still materialize broad transaction sets before filtering. |
| System time handling | Some paths still fall back to epoch `0` (e.g. transaction `created_at` generation and processed-event timestamps). The quota check itself fails closed, but `created_at` feeds day assignment. |
| Routing metrics | `success_rate`/`uptime_percent` are static `100.0` placeholders; `best-route` cannot differentiate federations. |
| Network fail-open | Unrecognized invoice networks map to `Mainnet` instead of failing closed. |
| Processed-events retention | Dedup tree grows unbounded; the prune routine is never called. |

### Deliberately Deferred

| Feature | Condition to Start |
| --- | --- |
| Keysend | Backend support plus idempotency and tests. |
| Multi-pay | Batch semantics, idempotency, partial-failure response model. |
| Notifications | End-to-end notification emission and tests. |
| Backend trait | Stable Fedimint behavior and a second backend target. |
| Hosted service mode | Authentication, tenant isolation, audit logs, deployment plan. |
| UI | Stable control-plane API and operator workflows. |

## Documentation Requirements

Root documentation should be reorganized around this spec:

- README: short product intro, install, quick start, and link to `SPEC.md`.
  The current README overstates the product and must be corrected: it calls
  Nanduti a "multi-backend" implementation, lists `pay_keysend` as an
  implemented method, marks `lookup_invoice` as in-progress though it is
  shipped and advertised, presents notifications as supported, and claims NIP-04
  support though the code only accepts NIP-44.
- SPEC: product and architecture source of truth. Note that `SPEC.md` itself is
  currently untracked; committing it is the first Phase 0 task.
- `pending.md`: a stale completed-work log referencing a nonexistent `nopus.md`;
  delete or archive it.
- PRODUCTION_READINESS_REVIEW: either refresh with current status or replace
  with a tracked hardening checklist.
- Docker README: update to current `serve` command, `--relay` flags,
  `NANDUTI_MNEMONIC_PASSWORD`, and `API_SECRET`.
- API reference: document REST request/response shapes.
- NWC reference: document advertised methods and exact unsupported cases.
- Recovery guide: document mnemonic/data-dir backup and restore, including the
  consistency requirement that the sled DB, root `.mnemonic`, and per-federation
  directories be backed up together.

## Hardening Checklist Before Production Claims

Security:

- Fail closed on unknown network metadata.
- Remove remaining epoch-zero fallbacks from audit/limit paths.
- Decide whether connection and federation records require encryption at rest.
- Require explicit opt-out for unauthenticated non-loopback REST.
- Review REST manual payment semantics against operator auth expectations.
- Reject an empty `NANDUTI_MNEMONIC_PASSWORD` at startup, not only later when a
  Fedimint client is created.
- Guard federation-local client databases against a missing `.mnemonic` the
  same way the service-level store is guarded, so a missing federation-local
  mnemonic beside an existing federation DB fails startup rather than silently
  generating a fresh root secret.
- Consider a data-plane abuse model. Spending limits are not rate limits: any
  pubkey can make the daemon decrypt and do storage lookups per event. Decide
  whether unauthenticated request volume needs its own rate limiting, and
  whether unauthorized senders should receive any response (today
  `multi_pay_*` returns `NOT_IMPLEMENTED` before the connection-auth gate,
  while other methods are gated first).

Correctness:

- Make `get_info` truthful about block hash and block height.
- Reconcile documentation with actual advertised NWC capabilities.
- Add stronger tests for amountless invoices, method restrictions, allowed
  federations, network routing, duplicate payments, pending reservations, and
  restart recovery.
- Add tests for NWC event deduplication and response-send failure behavior.
- Update `success_rate`/`uptime_percent` from real payment outcomes, or drop
  `best-route` until the metrics are real, so it stops silently behaving like
  every federation is perfect.
- Decide and document the intended NWC error code for each rejection case (for
  example, amountless invoices currently return `NOT_IMPLEMENTED`).

Scalability:

- Avoid full scans in high-volume transaction query paths.
- Add indexes for connection/day spending if daily quota scanning becomes large.
- Cap API transaction limits consistently.
- Prune the processed-events tree periodically to bound its growth.

Operations:

- Add backup/restore instructions, including that the sled DB, the root
  `.mnemonic`, and every per-federation directory must be captured as a
  consistent set (realistically with the daemon stopped, since sled has no
  online-backup story).
- Add startup health/readiness diagnostics.
- Add structured logging for payment lifecycle events.
- Document failure modes around Fedimint outgoing operation recovery.

Documentation:

- Remove or update stale Docker commands.
- Clarify Fedimint-only MVP.
- Stop claiming full NWC support for unsupported methods.

## Suggested Implementation Phases

### Phase 0: Spec and Docs Stabilization

Goal: make the project understandable.

Tasks:

- Adopt this `SPEC.md` as source of truth.
- Update README to match the MVP.
- Update Docker docs.
- Convert the production readiness review into a current checklist.
- Add a concise NWC capability matrix.

Exit criteria:

- A new contributor can tell what is real, what is future, and what is unsafe.

### Phase 1: Production Safety Pass

Goal: remove known safety ambiguities.

Tasks:

- Fail closed on unknown networks.
- Remove epoch-zero fallbacks from security-relevant paths.
- Make `get_info` honest about unavailable chain tip data.
- Tighten REST transaction error handling.
- Add targeted tests for payment and quota invariants.

Exit criteria:

- Core money-moving invariants are covered by tests and docs.

### Phase 2: Recovery and Observability

Goal: make restart and failure behavior operator-safe.

Tasks:

- Document and test mnemonic/data-dir recovery.
- Improve Pending outgoing recovery documentation and diagnostics.
- Add structured payment lifecycle logs.
- Add operational health output for relays, federations, and storage.

Exit criteria:

- Operators know what happened after a crash and what action is safe.

### Phase 3: API and Query Shape

Goal: make control-plane behavior scalable and consistent.

Tasks:

- Normalize REST response errors.
- Enforce transaction query caps uniformly.
- Add or improve indexes for large stores.
- Document REST request/response schema.

Exit criteria:

- Transaction history and admin APIs are predictable under growth.

### Phase 4: Capability Expansion

Goal: add new protocol capabilities only when backend support is real.

Tasks:

- Wire notifications end to end.
- Evaluate keysend backend support.
- Design multi-pay semantics.
- Introduce a backend trait only with a second backend target.

Exit criteria:

- New advertised capabilities are backed by tests and operational recovery
  behavior.

## Open Questions

1. Is the product intended for personal/self-hosted operators only, or eventually
   for hosted multi-user deployments?
2. Should REST manual payments enforce the same spending limits as NWC
   connections, or is REST always an unrestricted operator channel?
3. Should all persistent state, including connections and federation invite
   codes, be encrypted at rest?
4. Should persistent mode refuse startup if `API_SECRET` is missing and the host
   is not loopback?
5. Is `pay_keysend` worth keeping in handler code before backend support exists,
   or should it be removed until implementable?
6. What is the intended support level for MCP: trusted local-only tool, or a
   first-class automation API?
7. Which network should a no-federation or no-online-federation `get_info`
   response report? (Current behavior: defaults to `Mainnet` with height `0`.
   Decide whether that default is acceptable or should be represented as
   unknown.)
8. What is the acceptable recovery behavior for outgoing payments after process
   crash?
9. Are routing metrics (`success_rate`, `uptime_percent`) intended to be real
   measurements or placeholders? (Current behavior: static `100.0` placeholders
   that are never updated. Decide whether to implement real measurements or drop
   `best-route` until they exist.)
10. Should Nanduti support amountless BOLT11 invoices once the backend can do it?
11. What is the intended relay behavior: reconnect policy, whether info and
    response events publish to all configured relays or a subset, and how the
    daemon should behave when every relay is unreachable at startup or at
    response-publish time?

## Glossary

- Backend: a wallet implementation that can hold balance, pay invoices, create
  invoices, estimate fees, and report capabilities.
- Control plane: trusted operator interface, currently REST/CLI/MCP.
- Data plane: external wallet-app request path, currently NWC events over Nostr.
- Federation: a Fedimint federation joined by the daemon.
- NWC connection: a scoped Nostr Wallet Connect credential issued to a client app.
- Wallet pubkey: the daemon's Nostr public key, used as the NWC target.
- Client pubkey: the NWC app's Nostr public key derived from the connection URI
  secret and used for authorization.
- Pending transaction: local record created before a backend operation completes.
- Advertised capability: a method or notification listed in the Nostr info event
  or `get_info` response.
