use std::{
    env,
    path::{Path, PathBuf},
};

use crownos_ipc::{MethodMsg, adapter::tokio::AsyncClient};
use update_proto::{SOCKET_DIR, Snapshot, updated};

use crate::{ClientError, Event};

const ROOT_OVERRIDE: &str = "CROWNOS_UPDATE_ROOT";

/// A connection to crownos-updated that already receives every event.
pub struct UpdateClient {
    connection: AsyncClient,
}

impl UpdateClient {
    /// Connects in `/run/crownos`, or under `CROWNOS_UPDATE_ROOT` when a
    /// daemon runs against a fixture root.
    pub async fn connect() -> Result<Self, ClientError> {
        Self::connect_in(&socket_directory()).await
    }

    pub async fn connect_in(directory: &Path) -> Result<Self, ClientError> {
        let mut connection = AsyncClient::connect_in(directory, updated::SERVICE)?;
        connection.subscribe::<updated::StateChanged>().await?;
        connection.subscribe::<updated::Progress>().await?;
        Ok(Self { connection })
    }

    pub async fn state(&mut self) -> Result<Snapshot, ClientError> {
        self.call(&updated::state {}).await
    }

    pub async fn check(&mut self) -> Result<(), ClientError> {
        self.call(&updated::check {}).await
    }

    pub async fn download(&mut self) -> Result<(), ClientError> {
        self.call(&updated::download {}).await
    }

    pub async fn apply(&mut self) -> Result<(), ClientError> {
        self.call(&updated::apply {}).await
    }

    pub async fn restart(&mut self) -> Result<(), ClientError> {
        self.call(&updated::restart {}).await
    }

    /// Waits for inbound frames; then [`Self::drain_events`] hands them out.
    pub async fn pump(&mut self) -> Result<(), ClientError> {
        Ok(self.connection.pump().await?)
    }

    pub fn drain_events(&mut self) -> Vec<Event> {
        let client = self.connection.client_mut();
        std::iter::from_fn(|| client.next_event_raw())
            .filter_map(|message| Event::decode(&message))
            .collect()
    }

    pub fn is_closed(&self) -> bool {
        self.connection.client().is_closed()
    }

    async fn call<M>(&mut self, request: &M) -> Result<M::Reply, ClientError>
    where
        M: MethodMsg<Args = M>,
    {
        Ok(self.connection.call::<M>(request, Vec::new()).await?)
    }
}

fn socket_directory() -> PathBuf {
    let socket_directory = Path::new(SOCKET_DIR);
    match env::var_os(ROOT_OVERRIDE) {
        Some(root) => Path::new(&root).join(
            socket_directory
                .strip_prefix("/")
                .unwrap_or(socket_directory),
        ),
        None => socket_directory.to_owned(),
    }
}
