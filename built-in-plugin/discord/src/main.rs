//! Discord MCP server — REST tools with per-agent configuration and a live
//! Gateway message stream.

mod config;
mod discord;
mod gateway;
mod passthrough;
mod server;
mod tools;

use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::ServiceExt;
use rmcp::transport::stdio;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // stdout carries the MCP stream, so every log line goes to stderr.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("DISCORD_MCP_LOG").unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let cli = config::parse_cli(std::env::args().skip(1));
    let cfg = Arc::new(config::Config::from_env_and_cli(cli)?);
    let client = discord::DiscordClient::new(&cfg)?;

    tracing::info!(
        api_base = %cfg.api_base,
        gateway_url = %cfg.gateway_url,
        has_token = cfg.bot_token.is_some(),
        tools = tools::TOOLS.len(),
        "discord-mcp starting"
    );

    let service = server::DiscordMcp::new(cfg, client)
        .serve(stdio())
        .await
        .context("starting the MCP stdio transport")?;

    service.waiting().await?;
    Ok(())
}
