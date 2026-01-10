//! Tests for transaction handlers

#[cfg(test)]
mod tests {
    use super::super::transactions::*;
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

    async fn create_mock_app_state() -> Arc<AppState> {
        let storage = Arc::new(Storage::new(None, None).unwrap());
        let federation_manager = Arc::new(
            FederationManager::new_with_load(Some(storage.clone()), None)
                .await
                .unwrap(),
        );

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

    fn create_test_router(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/api/v1/transactions", get(list_transactions))
            .with_state(state)
    }

    #[tokio::test]
    async fn test_list_transactions_empty() -> anyhow::Result<()> {
        let state = create_mock_app_state().await;
        let app = create_test_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/transactions")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::OK);

        let body = http_body_util::BodyExt::collect(response.into_body())
            .await?
            .to_bytes();
        let transactions: Vec<nanduti_core::models::Transaction> = serde_json::from_slice(&body)?;
        assert!(transactions.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_list_transactions_with_limit() -> anyhow::Result<()> {
        let state = create_mock_app_state().await;
        let app = create_test_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/transactions?limit=10")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::OK);
        Ok(())
    }

    #[tokio::test]
    async fn test_list_transactions_with_offset() -> anyhow::Result<()> {
        let state = create_mock_app_state().await;
        let app = create_test_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/transactions?offset=5&limit=10")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::OK);
        Ok(())
    }
}
