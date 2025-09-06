//! Tests for invoice handlers

#[cfg(test)]
mod tests {
    use super::super::invoices::*;
    use crate::state::AppState;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::post,
        Router,
    };
    use nanduti_core::{federation::FederationManager, storage::Storage};
    use std::sync::Arc;
    use tower::ServiceExt;

    /// Create a mock app state for testing
    async fn create_mock_app_state() -> Arc<AppState> {
        let storage = Arc::new(Storage::new(None).unwrap());
        let federation_manager = Arc::new(
            FederationManager::new_with_load(Some(storage.clone()), None)
                .await
                .unwrap(),
        );

        Arc::new(AppState {
            federation_manager: Arc::clone(&federation_manager),
            storage,
            nwc_handler: Arc::new(crate::NwcHandler::new(
                Arc::clone(&federation_manager),
                Arc::new(crate::FederationRouter::new(
                    Arc::clone(&federation_manager),
                    crate::RoutingStrategy::RoundRobin,
                )),
                None,
            )),
            nostr_client: Arc::new(crate::NostrClient::new(vec![], None).await.unwrap()),
            router: Arc::new(crate::FederationRouter::new(
                Arc::clone(&federation_manager),
                crate::RoutingStrategy::RoundRobin,
            )),
        })
    }

    /// Helper to create test router
    fn create_test_router(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/api/v1/invoices", post(create_invoice))
            .with_state(state)
    }

    #[tokio::test]
    async fn test_create_invoice_missing_amount() -> anyhow::Result<()> {
        let state = create_mock_app_state().await;
        let app = create_test_router(state);

        // Request without amount field
        let invalid_req = r#"{}"#;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/invoices")
                    .header("content-type", "application/json")
                    .body(Body::from(invalid_req))?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        Ok(())
    }

    #[tokio::test]
    async fn test_create_invoice_invalid_amount() -> anyhow::Result<()> {
        let state = create_mock_app_state().await;
        let app = create_test_router(state);

        // Request with negative amount
        let invalid_req = r#"{"amount_msat": -1000, "description": "test"}"#;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/invoices")
                    .header("content-type", "application/json")
                    .body(Body::from(invalid_req))?,
            )
            .await?;

        // Should reject negative amounts
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        Ok(())
    }

    #[tokio::test]
    async fn test_create_invoice_no_federations() -> anyhow::Result<()> {
        let state = create_mock_app_state().await;
        let app = create_test_router(state);

        let valid_req = r#"{"amount_msat": 1000, "description": "test invoice"}"#;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/invoices")
                    .header("content-type", "application/json")
                    .body(Body::from(valid_req))?,
            )
            .await?;

        // Should fail when no federations are configured (returns 422 for validation errors)
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        Ok(())
    }

    #[tokio::test]
    async fn test_create_invoice_with_expiry() -> anyhow::Result<()> {
        let state = create_mock_app_state().await;
        let app = create_test_router(state);

        let req_with_expiry = r#"{
            "amount_msat": 5000,
            "description": "test invoice",
            "expiry_secs": 3600
        }"#;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/invoices")
                    .header("content-type", "application/json")
                    .body(Body::from(req_with_expiry))?,
            )
            .await?;

        // Should fail when no federations are configured (returns 422 for validation errors)
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        Ok(())
    }
}
