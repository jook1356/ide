//! Client side of the protocol: connects to an `ide-server` through a [`Transport`], offers
//! typed async RPC methods, a merged event stream (connection state, notifications, terminal
//! output) and reconnects automatically, re-attaching terminals from the last offset the app
//! received so output is delivered exactly once.
//!
//! ```no_run
//! # async fn demo() -> Result<(), ide_proto::ProtoError> {
//! use ide_client::{Client, ClientEvent, ClientOptions, LocalChild};
//! let transport = LocalChild::find().expect("ide-server binary");
//! let client = Client::connect(transport, ClientOptions::default()).await?;
//! let mut events = client.events().expect("first call");
//! let sessions = client.sessions().await?;
//! client.attach(&sessions[0].id, None).await?;
//! client.write(&sessions[0].id, b"echo hi\r".to_vec()).await?;
//! while let Some(ev) = events.next().await {
//!     if let ClientEvent::Term(chunk) = ev { /* feed chunk.bytes to the terminal model */ }
//! }
//! # Ok(()) }
//! ```

mod client;
pub mod transport;

pub use client::{Client, ClientEvent, ClientEvents, ClientOptions};
pub use ide_proto::{
    Agent, Attached, NewSession, Notify, ProfileInfo, ProtoError, SessionInfo, SessionState,
    TermChunk, UserInfo, Welcome,
};
pub use transport::{
    AsyncIo, ConnectFuture, Connection, LocalChild, Memory, SERVER_BIN_ENV, Ssh, Switchable,
    Transport,
};

/// iced adapter (feature `iced`).
#[cfg(feature = "iced")]
pub mod iced_support {
    use std::hash::{Hash, Hasher};

    use crate::{Client, ClientEvent};

    struct Key(Client);

    impl Hash for Key {
        fn hash<H: Hasher>(&self, state: &mut H) {
            "ide-client".hash(state);
            self.0.id().hash(state);
        }
    }

    /// All events of `client` (connected/disconnected, notifications, terminal chunks) as an
    /// iced subscription. One per client; the event receiver is returned to the client when
    /// the subscription ends, so it can be re-created.
    pub fn subscription(client: &Client) -> iced::Subscription<ClientEvent> {
        iced::Subscription::run_with(Key(client.clone()), |key| {
            futures::stream::unfold(key.0.events(), |events| async move {
                let mut events = events?;
                let e = events.next().await?;
                Some((e, Some(events)))
            })
        })
    }
}
