//! Tests for payment handlers

#[cfg(test)]
mod tests {
    use super::super::payments::*;
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
                std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new())),
            )),
            nostr_client,
            router,
            max_payment_amount: None,
            daily_limit_amount: None,
            relays: vec![],
            in_flight_payments: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
        })
    }

    fn create_test_router(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/api/v1/payments", post(pay_invoice))
            .with_state(state)
    }

    #[tokio::test]
    async fn test_pay_invoice_missing_invoice() -> anyhow::Result<()> {
        let state = create_mock_app_state().await;
        let app = create_test_router(state);

        let invalid_req = r#"{}"#;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/payments")
                    .header("content-type", "application/json")
                    .body(Body::from(invalid_req))?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        Ok(())
    }

    #[tokio::test]
    async fn test_pay_invoice_invalid_bolt11() -> anyhow::Result<()> {
        let state = create_mock_app_state().await;
        let app = create_test_router(state);

        let invalid_req = r#"{"invoice": "not_a_valid_bolt11"}"#;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/payments")
                    .header("content-type", "application/json")
                    .body(Body::from(invalid_req))?,
            )
            .await?;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        Ok(())
    }
}
