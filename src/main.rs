mod ai;
mod app;
mod application;
mod config;
mod db;
mod domain;
mod infrastructure;
mod tasks;
mod telegram;
mod web_content;

use anyhow::Result;
use infrastructure::{directories, health, instance_guard, logging, shutdown};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let config = config::load_config()?;
    let paths = directories::ensure_directories(&config.directories)?;
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return health::check(&paths).await;
    }
    logging::init_tracing(&config, &paths)?;
    let _instance_guard = instance_guard::InstanceGuard::acquire(&paths)?;

    let (shutdown, _) = shutdown::Shutdown::new();
    shutdown::install_signal_handlers(shutdown.clone());

    let app = app::SpamGuardApp::initialize(config, paths, shutdown.clone()).await?;
    app.run().await
}
