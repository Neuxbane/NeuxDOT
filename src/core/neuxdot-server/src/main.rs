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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,neuxdot_core=debug,neuxdot_server=debug".into()),
        )
        .init();

    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite:neuxdot.sqlite?mode=rwc".to_string());

    info!("Connecting to SQLite database: {}", db_url);
    let pool = init_db(&db_url).await?;
    let db = Arc::new(Database::new(pool));

    // Retrieve or initialize the root account (password is deterministically derived from created date)
    let (_root_acc, root_password) = db.get_or_init_root_account().await?;

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

    let state = AppState { db, event_tx };

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_methods(AllowMethods::mirror_request())
        .allow_headers(AllowHeaders::mirror_request())
        .allow_credentials(true);

    let app = routes::build_router(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8000);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("🚀 NeuxDOT server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
