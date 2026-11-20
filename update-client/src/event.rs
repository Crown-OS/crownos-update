use crownos_ipc::{EventMsg, Message, postcard};
use update_proto::{Snapshot, updated};

/// What the daemon pushed, in the order it pushed it.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    State(Box<Snapshot>),
    Progress(updated::Progress),
}

impl Event {
    pub(crate) fn decode(message: &Message) -> Option<Self> {
        match message.selector {
            <updated::StateChanged as EventMsg>::SELECTOR => {
                postcard::from_bytes::<updated::StateChanged>(&message.payload)
                    .ok()
                    .map(|changed| Self::State(Box::new(changed.snapshot)))
            }
            <updated::Progress as EventMsg>::SELECTOR => postcard::from_bytes(&message.payload)
                .ok()
                .map(Self::Progress),
            _ => None,
        }
    }
}
