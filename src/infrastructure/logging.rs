use std::io;

use anyhow::Result;
use once_cell::sync::OnceCell;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use crate::{config::AppConfig, infrastructure::directories::ResolvedPaths};

static INIT: OnceCell<()> = OnceCell::new();
static GUARD: OnceCell<tracing_appender::non_blocking::WorkerGuard> = OnceCell::new();

pub fn init_tracing(config: &AppConfig, paths: &ResolvedPaths) -> Result<()> {
    INIT.get_or_try_init::<_, anyhow::Error>(|| {
        let env_filter = EnvFilter::try_from_default_env()
            .or_else(|_| EnvFilter::try_new(&config.logging.level))
            .unwrap_or_else(|_| EnvFilter::new("info"));

        let file_appender = tracing_appender::rolling::daily(&paths.logs_dir, "bot.log");
        let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
        let _ = GUARD.set(guard);

        let console_layer = fmt::layer()
            .with_writer(io::stdout)
            .with_target(true)
            .with_ansi(true);

        let file_layer = fmt::layer()
            .with_writer(file_writer)
            .with_target(true)
            .with_ansi(false);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(console_layer)
            .with(file_layer)
            .init();

        tracing::info!(logs = %paths.logs_dir.display(), "tracing initialized");
        Ok(())
    })?;
    Ok(())
}
