//! Telegram MCP server — Bot API tools with per-agent configuration and a live
//! update stream.

mod config;
mod passthrough;
mod server;
mod telegram;
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
            EnvFilter::try_from_env("TELEGRAM_MCP_LOG").unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let cli = config::parse_cli(std::env::args().skip(1));
    let cfg = Arc::new(config::Config::from_env_and_cli(cli)?);
    let client = telegram::TelegramClient::new(&cfg)?;

    tracing::info!(
        api_base = %cfg.api_base,
        has_token = cfg.bot_token.is_some(),
        tools = tools::TOOLS.len(),
        "telegram-mcp starting"
    );

    let service = server::TelegramMcp::new(cfg, client)
        .serve(stdio())
        .await
        .context("starting the MCP stdio transport")?;

    service.waiting().await?;
    Ok(())
}
