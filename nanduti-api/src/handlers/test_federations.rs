//! Tests for federation management handlers

#[cfg(test)]
mod tests {
    use super::super::federations::*;
    use crate::state::AppState;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use nanduti_core::{federation::FederationManager, storage::Storage};
    use std::sync::Arc;
    use tower::ServiceExt;

    /// Create a mock app state for testing
    async fn create_mock_app_state() -> Arc<AppState> {
        let storage = Arc::new(Storage::new(None, None).unwrap());
        let federation_manager = Arc::new(
            FederationManager::new_with_load(Some(storage.clone()), None)
                .await
                .unwrap(),
        );

        // We'll create minimal state - real implementation would need full mocks
        let nostr_client = Arc::new(crate::NostrClient::new(vec![], None).await.unwrap());
        let router = Arc::new(crate::FederationRouter::new(
            Arc::clone(&federation_manager),
            crate::RoutingStrategy::RoundRobin,
        ));

        Arc::new(AppState {
            federation_manager: Arc::clone(&federation_manager),
            storage,
            nwc_handler: Arc::new(crate::NwcHandler::new(
                Arc::clone(&federation_manager),
                Arc::clone(&router),
                None,
                Arc::clone(&nostr_client),
            )),
            nostr_client,
            router,
        })
    }

    /// Helper to create test router
    fn create_test_router(state: Arc<AppState>) -> Router {
        Router::new()
            .route(
                "/api/v1/federations",
                get(list_federations).post(add_federation),
            )
            .route(
                "/api/v1/federations/{id}",
                get(get_federation).delete(remove_federation),
            )
            .route(
                "/api/v1/federations/{id}/balance",
                get(get_federation_balance),
            )
            .route(
                "/api/v1/federations/{id}/gateways",
                get(list_federation_gateways),
            )
            .with_state(state)
    }

    #[tokio::test]
    async fn test_list_federations_empty() -> anyhow::Result<()> {
        let state = create_mock_app_state().await;
        let app = create_test_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/federations")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::OK);

        let body = http_body_util::BodyExt::collect(response.into_body())
            .await?
            .to_bytes();
        let federations: Vec<serde_json::Value> = serde_json::from_slice(&body)?;
        assert!(federations.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_get_federation_not_found() -> anyhow::Result<()> {
        let state = create_mock_app_state().await;
        let app = create_test_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/federations/nonexistent")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        Ok(())
    }

    #[tokio::test]
    async fn test_remove_federation_not_found() -> anyhow::Result<()> {
        let state = create_mock_app_state().await;
        let app = create_test_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v1/federations/nonexistent")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        Ok(())
    }

    #[tokio::test]
    async fn test_get_federation_balance_not_found() -> anyhow::Result<()> {
        let state = create_mock_app_state().await;
        let app = create_test_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/federations/nonexistent/balance")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        Ok(())
    }

    #[tokio::test]
    async fn test_list_federation_gateways_not_found() -> anyhow::Result<()> {
        let state = create_mock_app_state().await;
        let app = create_test_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/federations/nonexistent/gateways")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        Ok(())
    }

    // Integration test for add_federation would require mock Fedimint client
    // which is complex to implement. For now, we test basic error handling.
    #[tokio::test]
    async fn test_add_federation_invalid_invite() -> anyhow::Result<()> {
        let state = create_mock_app_state().await;
        let app = create_test_router(state);

        let invalid_req = r#"{"invite_code": "invalid"}"#;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/federations")
                    .header("content-type", "application/json")
                    .body(Body::from(invalid_req))?,
            )
            .await?;

        // Should fail to deserialize invalid invite code
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        Ok(())
    }
}
