use std::sync::Arc;

use anyhow::Result;
use tokio_cron_scheduler::{Job, JobScheduler};

pub type RestartCallback = Arc<dyn Fn() + Send + Sync>;

pub async fn configure_restart_jobs(
    cron_specs: &[String],
    callback: RestartCallback,
) -> Result<JobScheduler> {
    let scheduler = JobScheduler::new().await?;
    for spec in cron_specs {
        let label = spec.clone();
        let cb = callback.clone();
        let job = Job::new_async(spec.as_str(), move |_id, _l| {
            let cb = cb.clone();
            let cron_label = label.clone();
            Box::pin(async move {
                tracing::info!(target: "scheduler", cron = %cron_label, "restart job triggered");
                cb();
            })
        })?;
        scheduler.add(job).await?;
        tracing::info!(target: "scheduler", cron = %spec, "restart job registered");
    }
    scheduler.start().await?;
    Ok(scheduler)
}
