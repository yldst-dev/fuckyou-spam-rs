use tokio::sync::watch;

#[derive(Clone)]
pub struct Shutdown {
    sender: watch::Sender<bool>,
}

#[derive(Clone)]
pub struct ShutdownListener {
    receiver: watch::Receiver<bool>,
}

impl Shutdown {
    pub fn new() -> (Self, ShutdownListener) {
        let (sender, receiver) = watch::channel(false);
        (Self { sender }, ShutdownListener { receiver })
    }

    pub fn subscribe(&self) -> ShutdownListener {
        ShutdownListener {
            receiver: self.sender.subscribe(),
        }
    }

    pub fn trigger(&self) {
        let _ = self.sender.send(true);
    }
}

impl ShutdownListener {
    pub async fn notified(&mut self) {
        if *self.receiver.borrow() {
            return;
        }
        let _ = self.receiver.changed().await;
    }

    pub fn is_triggered(&self) -> bool {
        *self.receiver.borrow()
    }
}

pub fn install_signal_handlers(shutdown: Shutdown) {
    let ctrlc = shutdown.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            ctrlc.trigger();
        }
    });

    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let term = shutdown.clone();
        tokio::spawn(async move {
            if let Ok(mut sig) = signal(SignalKind::terminate()) {
                sig.recv().await;
                term.trigger();
            }
        });
    }
}
