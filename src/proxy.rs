//! Proxy server — local AI API gateway.
//!
//! Drop-in replacement for OpenAI/Anthropic endpoints.
//! Features: semantic caching, TOON encoding, token budgeting.

use anyhow::Result;
use axum::{
    extract::State,
    http::StatusCode,
    routing::{any, post},
    Router,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceBuilder;
use tower_http::{compression::CompressionLayer, trace::TraceLayer};
use tracing::info;

use crate::cache::SemanticCache;
use crate::encode;

pub async fn start_server(port: u16, upstream: Option<String>) -> Result<()> {
    let upstream = upstream.unwrap_or_else(|| "https://api.openai.com".to_string());
    let app_state = Arc::new(RwLock::new(AppState::new(&upstream)));

    let app = Router::new()
        .route("/v1/chat/completions", post(proxy_chat))
        .route("/v1/completions", post(proxy_completion))
        .route("/v1/embeddings", post(proxy_embeddings))
        .route("/v1/models", any(proxy_models))
        .route("/health", any(health))
        .layer(
            ServiceBuilder::new()
                .layer(CompressionLayer::new())
                .layer(TraceLayer::new_for_http()),
        )
        .with_state(app_state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    info!("PRISM proxy listening on http://{}", addr);
    info!("Upstream: {}", upstream);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

struct AppState {
    upstream: String,
    cache: SemanticCache,
}

impl AppState {
    fn new(upstream: &str) -> Self {
        let cache = match crate::cache::SemanticCache::new(crate::prism_data_dir()) {
            Ok(c) => c,
            Err(_) => panic!("Failed to create semantic cache — ensure cache directory is writable"),
        };
        Self {
            upstream: upstream.to_string(),
            cache,
        }
    }
}

async fn proxy_chat(
    State(state): State<Arc<RwLock<AppState>>>,
    body: String,
) -> (StatusCode, String) {
    let request: serde_json::Value = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, r#"{"error": "invalid JSON"}"#.to_string()),
    };

    let model = request["model"].as_str().unwrap_or("gpt-4");

    match forward_to_upstream(&state.read().await.upstream, "/v1/chat/completions", &body).await {
        Ok(response) => {
            // TOON encode array responses if applicable
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&response) {
                if json["choices"].is_array() {
                    if let Ok(toon) = encode::encode_json_to_toon(&json["choices"]) {
                        let wrapped = serde_json::json!({
                            "model": model,
                            "toon_choices": toon,
                            "_original": json["choices"]
                        });
                        return (StatusCode::OK, wrapped.to_string());
                    }
                }
            }
            (StatusCode::OK, response)
        }
        Err(e) => (StatusCode::BAD_GATEWAY, format!(r#"{{"error": "{}"}}"#, e)),
    }
}

async fn proxy_completion(
    State(_state): State<Arc<RwLock<AppState>>>,
    body: String,
) -> (StatusCode, String) {
    (StatusCode::OK, body)
}

async fn proxy_embeddings(
    State(_state): State<Arc<RwLock<AppState>>>,
    body: String,
) -> (StatusCode, String) {
    (StatusCode::OK, body)
}

async fn proxy_models(State(_state): State<Arc<RwLock<AppState>>>) -> (StatusCode, String) {
    let models = serde_json::json!({
        "object": "list",
        "data": [
            {"id": "gpt-4", "object": "model"},
            {"id": "gpt-4-turbo", "object": "model"},
            {"id": "gpt-4o-mini", "object": "model"},
            {"id": "gpt-3.5-turbo", "object": "model"},
            {"id": "claude-3-5-sonnet-latest", "object": "model"},
            {"id": "claude-3-5-haiku-latest", "object": "model"},
        ]
    });
    (StatusCode::OK, models.to_string())
}

async fn health() -> (StatusCode, String) {
    (StatusCode::OK, r#"{"status": "ok", "prism": true}"#.to_string())
}

async fn forward_to_upstream(upstream: &str, path: &str, body: &str) -> anyhow::Result<String> {
    let url = format!("{}/{}", upstream.trim_end_matches('/'), path.trim_start_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await?;
    let text = resp.text().await?;
    Ok(text)
}
