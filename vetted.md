# Vetted Gateways Implementation Plan

## Overview

This document outlines the implementation of vetted gateway support in fedimint-nwc. Vetted gateways are Lightning gateways that have been approved by the federation guardians and stored in the meta module consensus.

## Background

### What are Vetted Gateways?

Vetted gateways are Lightning gateways that have been explicitly approved by a federation's guardians. This vetting process helps ensure:
- Gateway reliability and uptime
- Fair fee structures
- Trustworthy operators
- Better user experience

### How Fedimint Implements Vetted Gateways

1. **Meta Module Storage**: The list of vetted gateway IDs is stored in the meta module under the key `vetted_gateways`
2. **Consensus Mechanism**: Guardians vote on which gateways to vet, and when a threshold agrees, it becomes consensus
3. **Client Responsibility**: Clients read this list and should prefer vetted gateways when available
4. **Fallback Behavior**: If no vetted gateways are available, clients can fall back to any registered gateway

### Meta Field Structure

The `vetted_gateways` meta field contains a JSON array of hex-encoded gateway public keys:

```json
{
  "vetted_gateways": [
    "0234567890abcdef...",
    "fedcba0987654321..."
  ]
}
```

## Implementation Strategy

### Phase 1: Core Support (fedimint-nwc-core)

#### 1.1 Fetch Vetted Gateway IDs

Add method to `FedimintClientWrapper` to fetch vetted gateway IDs from the meta module:

```rust
impl FedimintClientWrapper {
    /// Fetch the list of vetted gateway IDs from the meta module
    pub async fn fetch_vetted_gateway_ids(&self) -> Result<Vec<String>> {
        // Get meta module
        // Fetch "vetted_gateways" field from DEFAULT_META_KEY (0)
        // Parse JSON array of hex strings
        // Return list of gateway IDs
    }
}
```

#### 1.2 Enhanced Gateway Information

Update gateway fetching to include vetted status:

```rust
pub struct GatewayInfo {
    pub gateway_id: String,
    pub api: String,
    pub node_pub_key: PublicKey,
    pub vetted: bool,  // NEW: indicates if gateway is vetted
    // ... other fields
}
```

#### 1.3 Smart Gateway Selection

Update `select_gateway()` to implement intelligent selection:

```rust
impl FedimintClientWrapper {
    pub async fn select_gateway(&self) -> Result<Option<LightningGateway>> {
        let all_gateways = self.fetch_gateways().await?;
        let vetted_ids = self.fetch_vetted_gateway_ids().await.unwrap_or_default();
        
        // Step 1: Try to select from vetted gateways
        let vetted_gateways: Vec<_> = all_gateways
            .iter()
            .filter(|g| vetted_ids.contains(&g.gateway_id.to_hex()))
            .collect();
        
        if !vetted_gateways.is_empty() {
            // Prefer vetted gateway
            return Ok(Some(select_random(&vetted_gateways)));
        }
        
        // Step 2: Fall back to any available gateway
        if !all_gateways.is_empty() {
            warn!("No vetted gateways available, using non-vetted gateway");
            return Ok(Some(select_random(&all_gateways)));
        }
        
        Ok(None)
    }
}
```

### Phase 2: API Layer (fedimint-nwc-api)

#### 2.1 Enhanced API Responses

Update API response types to include vetted information:

```rust
#[derive(Serialize, Deserialize)]
pub struct GatewayInfo {
    pub gateway_id: String,
    pub api: String,
    pub base_fee_msat: u32,
    pub proportional_fee_ppm: u32,
    pub vetted: bool,  // NEW field
}
```

#### 2.2 New API Endpoints

Add endpoints for vetted gateway management:

```rust
// GET /api/v1/federations/{id}/vetted-gateways
// Returns list of vetted gateway IDs
async fn get_vetted_gateways(
    Path(federation_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<String>>> {
    // Fetch and return vetted gateway IDs
}

// GET /api/v1/federations/{id}/gateways?vetted_only=true
// Filter gateways by vetted status
async fn list_gateways(
    Path(federation_id): Path<String>,
    Query(params): Query<GatewayQueryParams>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<GatewayInfo>>> {
    // Return gateways, optionally filtered by vetted status
}
```

### Phase 3: CLI Integration

#### 3.1 Display Vetted Status

Update CLI output to show vetted status:

```bash
$ fedimint-nwc fm-gateways
Federation: Federation ABC (e9a3a03f...)
  Gateways:
  Gateway ID                                     API                  Fees         Vetted
  ----------------------------------------------------------------------------------
  0234567890abcdef...                          gateway1.com         1000/2500    ✓
  fedcba0987654321...                          gateway2.com         500/1000     ✗
```

#### 3.2 Add Filtering Options

Add CLI flags for gateway filtering:

```bash
# Show only vetted gateways
fedimint-nwc fm-gateways --vetted-only

# Show vetted gateway IDs
fedimint-nwc fm-vetted-gateways
```

## Testing Strategy

### Unit Tests

1. Test parsing of `vetted_gateways` meta field
2. Test gateway selection logic (prefer vetted, fallback to non-vetted)
3. Test API response serialization with vetted field

### Integration Tests

1. Join a federation with vetted gateways configured
2. Verify that invoices are created using vetted gateways
3. Test fallback behavior when no vetted gateways are available
4. Test payment routing through vetted gateways

### Manual Testing

1. Set up a local federation with meta module
2. Configure `vetted_gateways` field with known gateway IDs
3. Verify client respects the vetted gateway list
4. Test with empty vetted list (should fall back gracefully)

## Migration Considerations

- This is a backwards-compatible change
- Federations without `vetted_gateways` will continue to work normally
- Clients will treat all gateways as non-vetted if the field is missing
- No database migrations required

## Security Considerations

1. **Trust Model**: Clients trust the federation's guardian consensus on which gateways to vet
2. **Fallback Security**: Using non-vetted gateways should log warnings
3. **Gateway Validation**: Verify gateway IDs are valid hex-encoded public keys
4. **Availability**: Ensure the system remains functional even without vetted gateways

## Performance Considerations

1. **Caching**: The meta module client automatically caches meta values
2. **Update Frequency**: Meta values are updated periodically in the background
3. **Selection Speed**: Gateway selection should remain fast (< 100ms)

## Future Enhancements

1. **Gateway Reputation System**: Track gateway performance metrics
2. **Automatic Vetting**: Guardians could automatically vet gateways based on performance
3. **User Preferences**: Allow users to override gateway selection preferences
4. **Multi-Federation Routing**: Select optimal gateway across multiple federations

## Implementation Checklist

- [ ] Add `fetch_vetted_gateway_ids()` method to FedimintClientWrapper
- [ ] Update gateway models to include `vetted` field
- [ ] Implement smart gateway selection logic
- [ ] Update API handlers to return vetted status
- [ ] Add new API endpoints for vetted gateway queries
- [ ] Update CLI to display vetted status
- [ ] Add unit tests for vetted gateway logic
- [ ] Add integration tests with real federation
- [ ] Update documentation
- [ ] Test backwards compatibility

## References

- [Fedimint Meta Module Documentation](https://github.com/fedimint/fedimint/tree/master/modules/fedimint-meta-client)
- [Vetted Gateways Meta Field Spec](https://github.com/fedimint/fedimint/blob/master/docs/meta_fields/vetted_gateways.md)
- [Lightning Gateway Documentation](https://github.com/fedimint/fedimint/blob/master/docs/gateway.md)