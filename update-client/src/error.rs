use crownos_ipc::Error as IpcError;
use thiserror::Error;
use update_proto::{UNAUTHORIZED, WRONG_PHASE};

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("the update service is not running")]
    Unavailable(#[source] IpcError),
    #[error("only an administrator can update this computer")]
    NotAllowed,
    #[error("{0}")]
    WrongPhase(String),
    #[error("the update service failed")]
    Ipc(#[source] IpcError),
}

impl From<IpcError> for ClientError {
    fn from(error: IpcError) -> Self {
        match error {
            IpcError::Remote(refusal) if refusal.code == UNAUTHORIZED => Self::NotAllowed,
            IpcError::Remote(refusal) if refusal.code == WRONG_PHASE => {
                Self::WrongPhase(refusal.message)
            }
            IpcError::Io(_)
            | IpcError::Disconnected
            | IpcError::ServiceUnavailable { .. }
            | IpcError::RuntimeDir => Self::Unavailable(error),
            other => Self::Ipc(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use crownos_ipc::RemoteError;

    use super::*;

    #[test]
    fn refusals_map_to_their_meaning() {
        let refused = |code| ClientError::from(IpcError::Remote(RemoteError::new(code, "why")));
        assert!(matches!(refused(UNAUTHORIZED), ClientError::NotAllowed));
        assert!(
            matches!(refused(WRONG_PHASE), ClientError::WrongPhase(message) if message == "why")
        );
        assert!(matches!(refused(RemoteError::LIMIT), ClientError::Ipc(_)));
        assert!(matches!(
            ClientError::from(IpcError::Disconnected),
            ClientError::Unavailable(_)
        ));
    }
}
