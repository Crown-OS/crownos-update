//! crownos-updated: checks, downloads and installs CrownOS releases one at a
//! time, with a snapshot to roll back to, and serves the state on
//! `/run/crownos/crownos-updated.sock`.

mod auth;
mod engine;
mod ipc;
mod pacman_conf;
mod paths;
mod process;
mod rollback;
mod utils;

use std::{pin::pin, sync::Arc};

use anyhow::{Context, bail};
use tokio::{
    signal::unix::{SignalKind, signal},
    sync::mpsc,
};
use tracing_subscriber::EnvFilter;

use engine::Engine;
use paths::Paths;
use utils::socket_dir::ensure_socket_directory;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let paths = Arc::new(Paths::from_environment());
    ensure_socket_directory(&paths.socket_directory)
        .with_context(|| format!("cannot create {}", paths.socket_directory.display()))?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?
        .block_on(run(paths))
}

/// The engine future is never dropped. When a signal arrives or the IPC
/// server ends, the server is dropped, which closes the verb queue; the
/// running verb finishes, then the process exits: status 0 after SIGTERM or
/// SIGINT, an error after a server failure.
async fn run(paths: Arc<Paths>) -> anyhow::Result<()> {
    let mut terminate = signal(SignalKind::terminate()).context("cannot watch for SIGTERM")?;
    let mut interrupt = signal(SignalKind::interrupt()).context("cannot watch for SIGINT")?;
    let (events, outbox) = mpsc::unbounded_channel();
    let (verbs, queued_verbs) = mpsc::unbounded_channel();
    let engine = Engine::start(Arc::clone(&paths), events).await;
    let snapshot = engine.subscribe();
    let server = ipc::bind(&paths)?;
    tracing::info!("serving in {}", paths.socket_directory.display());
    let mut running = pin!(engine.run(queued_verbs));
    let served = tokio::select! {
        () = &mut running => bail!("the update engine stopped"),
        served = ipc::serve(server, paths, snapshot, verbs, outbox) => served,
        _ = terminate.recv() => Ok(()),
        _ = interrupt.recv() => Ok(()),
    };
    tracing::info!("stopping after the running verb");
    running.await;
    served
}
