use std::sync::Arc;

use anyhow::Context;
use crownos_ipc::{
    Authorizer, ConnLimits, Limits, RemoteError, Server, ServiceBuilder,
    adapter::tokio::serve_with_outbox,
};
use tokio::sync::{mpsc, watch};
use update_proto::{Snapshot, UNAUTHORIZED, Verb, WRONG_PHASE, updated};

use crate::{
    auth::{Access, access_of},
    paths::Paths,
};

/// Every local user can connect, so each one gets little room.
fn limits() -> Limits {
    Limits {
        conn: ConnLimits {
            max_frame: 256 * 1024,
            max_inbox: 16,
            ..ConnLimits::default()
        },
        max_conns_per_uid: 4,
        max_subscriptions: 4,
    }
}

const SOCKET_MODE: u32 = 0o666;

struct Dispatcher {
    paths: Arc<Paths>,
    snapshot: watch::Receiver<Snapshot>,
    verbs: mpsc::UnboundedSender<Verb>,
    caller_uid: Option<u32>,
}

impl Dispatcher {
    fn accept(&self, verb: Verb) -> Result<(), RemoteError> {
        let administrator = self
            .caller_uid
            .is_some_and(|uid| access_of(uid, &self.paths) == Access::Administrator);
        if !administrator {
            return Err(RemoteError::new(
                UNAUTHORIZED,
                "Only an administrator can update this computer.",
            ));
        }
        if let Some(reason) = self.snapshot.borrow().refusal(verb) {
            return Err(RemoteError::new(WRONG_PHASE, reason));
        }
        self.verbs
            .send(verb)
            .map_err(|_| RemoteError::handler("the update engine stopped"))
    }
}

impl updated::Handler for Dispatcher {
    fn state(&mut self) -> Result<Snapshot, RemoteError> {
        Ok(self.snapshot.borrow().clone())
    }

    fn check(&mut self) -> Result<(), RemoteError> {
        self.accept(Verb::Check)
    }

    fn download(&mut self) -> Result<(), RemoteError> {
        self.accept(Verb::Download)
    }

    fn apply(&mut self) -> Result<(), RemoteError> {
        self.accept(Verb::Apply)
    }

    fn restart(&mut self) -> Result<(), RemoteError> {
        self.accept(Verb::Restart)
    }
}

pub fn bind(paths: &Paths) -> anyhow::Result<Server> {
    ServiceBuilder::new(updated::SERVICE)
        .directory(&paths.socket_directory)
        .socket_mode(SOCKET_MODE)
        .authorizer(Authorizer::AnyUser)
        .limits(limits())
        .build()
        .with_context(|| format!("cannot serve in {}", paths.socket_directory.display()))
}

pub async fn serve(
    mut server: Server,
    paths: Arc<Paths>,
    snapshot: watch::Receiver<Snapshot>,
    verbs: mpsc::UnboundedSender<Verb>,
    mut events: mpsc::UnboundedReceiver<updated::Event>,
) -> anyhow::Result<()> {
    let mut dispatcher = Dispatcher {
        paths,
        snapshot,
        verbs,
        caller_uid: None,
    };
    serve_with_outbox(
        &mut server,
        |server, peer, message| {
            dispatcher.caller_uid = server.credentials(peer).map(|credentials| credentials.uid);
            updated::dispatch(&mut dispatcher, server, peer, message).map(|()| true)
        },
        |context| events.poll_recv(context),
        emit,
    )
    .await
    .context("the IPC server stopped")
}

fn emit(server: &mut Server, event: updated::Event) {
    let result = match &event {
        updated::Event::StateChanged(changed) => server.emit(changed),
        updated::Event::Progress(progress) => server.emit(progress),
    };
    if let Err(error) = result {
        tracing::debug!("event not delivered: {error}");
    }
}
