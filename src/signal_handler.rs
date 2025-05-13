use log::info;
use std::sync::atomic::{ AtomicBool, Ordering };
use std::sync::Arc;

pub fn setup_signal_handler(shutdown_signal: Arc<AtomicBool>) {
    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("SIGINT received, initiating graceful shutdown...");
            }
            _ = async { // SIGTERM handling
                #[cfg(unix)]
                {
                    let mut term_signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
                    term_signal.recv().await;
                    info!("SIGTERM received, initiating graceful shutdown...");
                }
                #[cfg(not(unix))] // Fallback for non-Unix if necessary, though SIGTERM is Unix-specific
                {
                    std::future::pending::<()>().await; // Keep it pending
                }
            } => {}
        }
        shutdown_signal.store(true, Ordering::SeqCst);
    });
}
