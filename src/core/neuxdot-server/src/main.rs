use neuxdot_core::db::init_db;
use neuxdot_core::db::repos::Database;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

use neuxdot_server::routes;
use neuxdot_server::state::AppState;
use neuxdot_server::subscriptions::SubscriptionHub;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,neuxdot_core=debug,neuxdot_server=debug".into()),
        )
        .init();

    // Ensure data directory exists and migrate root neuxdot.sqlite if present
    let _ = std::fs::create_dir_all("data");
    if std::path::Path::new("neuxdot.sqlite").exists()
        && !std::path::Path::new("data/neuxdot.sqlite").exists()
    {
        let _ = std::fs::rename("neuxdot.sqlite", "data/neuxdot.sqlite");
    }

    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite:data/neuxdot.sqlite?mode=rwc".to_string());

    // Resolved before seeding: the control plugin is pinned to the server's own
    // loopback URL, so it must know the real port rather than a default.
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8000);

    info!("Connecting to SQLite database: {}", db_url);
    let pool = init_db(&db_url).await?;
    let db = Arc::new(Database::new(pool));

    // Retrieve or initialize the root account (password is deterministically derived from created date)
    let (_root_acc, root_password) = db.get_or_init_root_account().await?;

    // Server-native control MCP: install it as if it had always been there, so
    // an agent can create/manage other agents, plugins and providers. Seeded
    // with no credential on purpose — each agent supplies its own HTTP Basic
    // header via the per-agent Auth/Config override.
    let control_url = format!("http://127.0.0.1:{port}/api/control/mcp");
    match neuxdot_core::db::seed::ensure_control_plugins(db.pool(), &control_url).await {
        Ok(n) => info!(
            "control MCP seeded for {n} account(s) at {control_url} \
             (each agent must set its own Authorization header)"
        ),
        Err(e) => tracing::warn!("could not seed the control MCP plugin: {e}"),
    }

    println!("\n┌────────────────────────────────────────────────────────────────────────┐");
    println!("│                       NeuxDOT Server Setup                             │");
    println!("│                                                                        │");
    println!("│  Root user credentials:                                                │");
    println!("│    Username: root                                                      │");
    println!("│    Password: {:<56}│", root_password);
    println!("│                                                                        │");
    println!("│  Use these credentials to log in on the web dashboard.                 │");
    println!("└────────────────────────────────────────────────────────────────────────┘\n");

    let (event_tx, _) = broadcast::channel(1024);

    // Push subscriptions have to outlive any single turn: a Telegram poller is
    // only useful while the agent is idle. Start the reconciler before serving
    // so already-subscribed agents are listening from boot.
    let subscriptions = SubscriptionHub::new(Arc::clone(&db), event_tx.clone());
    let hub_for_boot = Arc::clone(&subscriptions);
    tokio::spawn(async move { hub_for_boot.sync_all().await });

    let state = AppState {
        db,
        event_tx,
        subscriptions,
    };

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_methods(AllowMethods::mirror_request())
        .allow_headers(AllowHeaders::mirror_request())
        .allow_credentials(true);

    let app = routes::build_router(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("🚀 NeuxDOT server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
