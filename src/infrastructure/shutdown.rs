use std::sync::Arc;

use tokio::sync::watch;

pub(crate) type RestartCallback = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub(crate) struct Shutdown {
    sender: watch::Sender<bool>,
}

#[derive(Clone)]
pub(crate) struct ShutdownListener {
    receiver: watch::Receiver<bool>,
}

impl Shutdown {
    pub(crate) fn new() -> (Self, ShutdownListener) {
        let (sender, receiver) = watch::channel(false);
        (Self { sender }, ShutdownListener { receiver })
    }

    pub(crate) fn subscribe(&self) -> ShutdownListener {
        ShutdownListener {
            receiver: self.sender.subscribe(),
        }
    }

    pub(crate) fn trigger(&self) {
        let _ = self.sender.send(true);
    }

    pub(crate) fn is_triggered(&self) -> bool {
        *self.sender.borrow()
    }
}

impl ShutdownListener {
    pub(crate) async fn notified(&mut self) {
        if *self.receiver.borrow() {
            return;
        }
        let _ = self.receiver.changed().await;
    }

    pub(crate) fn is_triggered(&self) -> bool {
        *self.receiver.borrow()
    }
}

pub(crate) fn install_signal_handlers(shutdown: Shutdown) {
    let ctrlc = shutdown.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            ctrlc.trigger();
        }
    });

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let term = shutdown.clone();
        tokio::spawn(async move {
            if let Ok(mut sig) = signal(SignalKind::terminate()) {
                sig.recv().await;
                term.trigger();
            }
        });
    }
}
