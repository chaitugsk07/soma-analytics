//! soma-analytics server binary.
//!
//! Boot order (§7):
//! 1. Telemetry
//! 2. Config (env)
//! 3. Metadata pool (DATABASE_URL)
//! 4. Migrate (soma-schema)
//! 5. Audit install + sink
//! 6. Data-source pool (ANALYTICS_DB_URL)
//! 7. Cache (REDIS_URL)
//! 8. AppState + LocalTokenVerifier
//! 9. Router + CORS + TraceLayer
//! 10. serve_with_shutdown

#![forbid(unsafe_code)]

use std::sync::Arc;

// ── Embedded dashboard (feature = "dashboard") ────────────────────────────────
//
// Embeds dashboard/dist at compile time via RustEmbed. The feature is OFF by
// default so `cargo build` works without dashboard/dist existing.
// To produce a single-binary build:
//   1. cd dashboard && trunk build --release
//   2. cargo build -p soma-server --release --features dashboard
#[cfg(feature = "dashboard")]
#[derive(rust_embed::RustEmbed)]
#[folder = "../../dashboard/dist"]
struct Dashboard;

use anyhow::{Context, Result};
use soma_api::{AppState, LocalTokenVerifier};
use soma_audit_pg::{AuditKeys, LocalSink};
use soma_storage::Store;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Telemetry
    soma_infra::telemetry::init();

    // 2. Config
    let bind_addr = soma_infra::config::env_or("SOMA_BIND", "0.0.0.0:8080");
    let analytics_url = soma_infra::config::require_env("ANALYTICS_DB_URL")
        .context("ANALYTICS_DB_URL must be set (the read-only data-source pool)")?;
    let embed_secret = soma_infra::config::require_env("ANALYTICS_EMBED_SECRET")
        .context("ANALYTICS_EMBED_SECRET must be set")?;

    // Optional KEK for encrypting per-source DSNs (Phase-1 default: None → use env source).
    let dsn_kek = match std::env::var("ANALYTICS_DSN_KEK_HEX") {
        Ok(_) => Some(
            soma_infra::crypto::CryptoKey::from_env("ANALYTICS_DSN_KEK_HEX")
                .context("ANALYTICS_DSN_KEK_HEX must be a valid 64-char hex string")?,
        ),
        Err(_) => None,
    };

    // CORS: fail-closed — empty/absent ⇒ same-origin only.
    let cors_origins = soma_infra::config::env_or("ANALYTICS_CORS_ORIGINS", "");
    if cors_origins.trim().is_empty() {
        tracing::warn!(
            "ANALYTICS_CORS_ORIGINS is not set — CORS disabled (same-origin only). \
             Set this env var if embedding into external products."
        );
    }

    // 3. Metadata pool (DATABASE_URL)
    let pool = soma_infra::connect_from_env()
        .await
        .context("connecting to Postgres (DATABASE_URL)")?;
    tracing::info!("metadata pool connected");

    // 4. Migrate (soma-schema; schema soma_analytics, advisory key 0x50A1_A7C5_0001)
    soma_storage::migrate(&pool)
        .await
        .context("soma-analytics migrations failed")?;
    tracing::info!("migrations applied");

    // 5. Audit: install schema + build sink
    soma_audit_pg::install(&pool)
        .await
        .context("soma-audit install failed")?;
    let audit_keys = Arc::new(
        AuditKeys::from_env_local()
            .context("SOMA_AUDIT_MASTER_SECRET must be set")?,
    );
    let audit_sink = Arc::new(LocalSink::new(pool.clone(), audit_keys, "soma-analytics"));
    tracing::info!("audit sink ready");

    // 6. Data-source pool (ANALYTICS_DB_URL — the read-only source we query)
    let ds_pool = soma_infra::db::connect(
        &soma_infra::db::PoolConfig::new(analytics_url),
    )
    .await
    .context("connecting to data-source pool (ANALYTICS_DB_URL)")?;
    tracing::info!("data-source pool connected");

    // 7. Cache (REDIS_URL)
    let cache = soma_infra::cache::connect_from_env()
        .await
        .context("connecting to Redis (REDIS_URL)")?;
    tracing::info!("cache connected");

    // 8. AppState + verifier
    let store = Arc::new(Store::new(
        pool.clone(),
        ds_pool,
        cache,
        audit_sink.clone(),
        dsn_kek,
    ));

    let verifier = Arc::new(LocalTokenVerifier {
        meta_pool: pool,
        embed_secret: embed_secret.clone(),
    });

    // AI seam (Phase 1.5): enabled when ANALYTICS_AI_ENABLED is "true", "1", "yes", or "on"
    // (case-insensitive). Any other value (including "false", absent) disables it.
    // Fix #5: tolerant parse — no startup error on "1"/"TRUE"/etc.
    let ai_flag = soma_infra::config::env_or("ANALYTICS_AI_ENABLED", "false");
    let ai_enabled = matches!(ai_flag.to_lowercase().as_str(), "true" | "1" | "yes" | "on");

    let ai_client_and_model = if ai_enabled {
        let cfg = soma_infra::llm::LlmConfig::from_env()
            .context("AI enabled but LLM client config is invalid (check ANTHROPIC_API_KEY + ANTHROPIC_MODEL)")?;
        let model = cfg.model.clone();
        let client = soma_infra::llm::LlmClient::new(cfg)
            .context("failed to build LLM client")?;
        tracing::info!("AI seam enabled (ANALYTICS_AI_ENABLED=true)");
        Some((client, model))
    } else {
        tracing::info!("AI seam disabled (ANALYTICS_AI_ENABLED not set or false)");
        None
    };

    let mut state = AppState::new(
        store.clone(),
        verifier as Arc<dyn soma_api::TokenVerifier>,
        audit_sink,
        embed_secret,
    );
    if let Some((client, model)) = ai_client_and_model {
        state = state.with_ai(client, model);
    }

    // Bootstrap: if no active tokens exist for the default tenant, create a root admin
    // token and write it to a file. The plaintext is never logged.
    let default_tenant = Uuid::nil(); // Phase-1: single default tenant
    match store.bootstrap_root_token(default_tenant).await {
        Ok(Some(token)) => {
            let token_path = soma_infra::config::env_or("SOMA_TOKEN_FILE", "./soma-analytics-root-token");
            // analytics_url not logged — contains credentials
            match std::fs::write(&token_path, &token) {
                Ok(()) => {
                    // Set permissions to 0600 on unix.
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600));
                    }
                    tracing::info!(path = %token_path, "root token written to file (shown once)");
                }
                Err(e) => tracing::warn!(err = %e, path = %token_path, "failed to write root token file"),
            }
        }
        Ok(None) => tracing::info!("active tokens exist; skipping bootstrap"),
        Err(e) => tracing::warn!(err = %e, "bootstrap_root_token failed (non-fatal)"),
    }

    // 9. Router + CORS + TraceLayer
    #[cfg(feature = "dashboard")]
    let app = {
        soma_api::router(state)
            .fallback(|uri: axum::http::Uri| async move {
                soma_infra::web::serve_spa::<Dashboard>(&uri)
            })
            .layer(soma_api::cors_layer(&cors_origins))
            .layer(TraceLayer::new_for_http())
    };
    #[cfg(not(feature = "dashboard"))]
    let app = soma_api::router(state)
        .layer(soma_api::cors_layer(&cors_origins))
        .layer(TraceLayer::new_for_http());

    // 10. Serve with graceful shutdown
    tracing::info!(addr = %bind_addr, "soma-analytics listening");
    soma_infra::web::serve_with_shutdown(&bind_addr, app)
        .await
        .context("server error")?;

    tracing::info!("shutdown complete");
    Ok(())
}
