mod ai;
mod app;
mod config;
mod db;
mod domain;
mod infrastructure;
mod tasks;
mod telegram;
mod web_content;

use anyhow::Result;
use infrastructure::{directories, logging, shutdown};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let config = config::load_config()?;
    let paths = directories::ensure_directories(&config.directories)?;
    logging::init_tracing(&config, &paths)?;

    let (shutdown, _) = shutdown::Shutdown::new();
    shutdown::install_signal_handlers(shutdown.clone());

    let app = app::SpamGuardApp::initialize(config, paths, shutdown.clone()).await?;
    app.run().await
}
