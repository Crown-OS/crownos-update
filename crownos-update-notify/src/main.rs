//! crownos-update-notify: a user-session service that follows crownos-updated
//! and posts one notification for each new release. A click opens Settings.

mod announced;
mod notification;

use std::time::Duration;

use anyhow::Context;
use tracing_subscriber::EnvFilter;
use update_client::{ClientError, Event, UpdateClient, proto::Snapshot};

use announced::{AnnouncedRelease, release_to_announce};
use notification::Notifier;

const FIRST_RETRY: Duration = Duration::from_secs(1);
const LONGEST_RETRY: Duration = Duration::from_secs(60);

struct Announcer {
    announced: AnnouncedRelease,
    notifier: Notifier,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let announcer = Announcer {
        announced: AnnouncedRelease::at_default_path(),
        notifier: Notifier::default(),
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?
        .block_on(announcer.run());
    Ok(())
}

impl Announcer {
    /// Follows the daemon forever and reconnects with a growing delay.
    async fn run(mut self) {
        let mut retry = FIRST_RETRY;
        loop {
            match UpdateClient::connect().await {
                Ok(mut client) => {
                    retry = FIRST_RETRY;
                    if let Err(error) = self.follow(&mut client).await {
                        tracing::info!("lost crownos-updated: {error}");
                    }
                }
                Err(error) => tracing::debug!("crownos-updated is not reachable: {error}"),
            }
            tokio::time::sleep(retry).await;
            retry = (retry * 2).min(LONGEST_RETRY);
        }
    }

    async fn follow(&mut self, client: &mut UpdateClient) -> Result<(), ClientError> {
        let snapshot = client.state().await?;
        self.consider(&snapshot).await;
        while !client.is_closed() {
            client.pump().await?;
            for event in client.drain_events() {
                if let Event::State(snapshot) = event {
                    self.consider(&snapshot).await;
                }
            }
        }
        Ok(())
    }

    async fn consider(&mut self, snapshot: &Snapshot) {
        let last_announced = self.announced.read().await;
        let Some(version) = release_to_announce(snapshot, last_announced.as_ref()) else {
            return;
        };
        match self.notifier.announce(version).await {
            Ok(()) => self.announced.remember(version).await,
            Err(error) => tracing::warn!("cannot post the update notification: {error}"),
        }
    }
}
