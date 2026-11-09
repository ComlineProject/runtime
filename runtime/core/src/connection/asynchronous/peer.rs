use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, MutexGuard};
use std::time::Duration;

use serde::Serialize;

use crate::connection::Extensions;
use crate::contract::{
    Call, ConnectionId, Envelope, Kind, Outcome, ProtocolNumber, RuntimeError, WireFormat,
};

/// A boxed, `Send` future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A clock for call timeouts: a future that completes after the duration.
/// With tokio, [`tokio::timer`](crate::transport::tokio::timer).
pub type Timer = Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>;

/// A reply that owns its bytes: what an async call returns, since it crosses
/// from the driver to the calling task. [`envelope`](Self::envelope) borrows
/// it as the blocking driver's [`Envelope`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedEnvelope {
    Ok(Vec<u8>),
    Err { id: u16, body: Vec<u8> },
    Fault,
}

impl OwnedEnvelope {
    pub fn envelope(&self) -> Envelope<'_> {
        match self {
            OwnedEnvelope::Ok(payload) => Envelope::Ok(payload),
            OwnedEnvelope::Err { id, body } => Envelope::Err { id: *id, body },
            OwnedEnvelope::Fault => Envelope::Fault,
        }
    }
}

impl From<Envelope<'_>> for OwnedEnvelope {
    fn from(envelope: Envelope<'_>) -> Self {
        match envelope {
            Envelope::Ok(payload) => OwnedEnvelope::Ok(payload.to_vec()),
            Envelope::Err { id, body } => OwnedEnvelope::Err {
                id,
                body: body.to_vec(),
            },
            Envelope::Fault => OwnedEnvelope::Fault,
        }
    }
}

/// What an async handler produced: the [`Reply`](crate::contract::Reply) of
/// the blocking side, owned, so it can be returned from a future.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    outcome: Outcome,
    body: Vec<u8>,
}

impl Answer {
    /// A success: `payload` is the serialised return value.
    pub fn ok(payload: Vec<u8>) -> Self {
        Self {
            outcome: Outcome::Ok,
            body: payload,
        }
    }

    /// A raised schema error: `body` is the serialised error struct, `id`
    /// its ordinal.
    pub fn err(id: u16, body: Vec<u8>) -> Self {
        Self {
            outcome: Outcome::Err(id),
            body,
        }
    }

    /// A failure the schema doesn't describe.
    pub fn fault() -> Self {
        Self {
            outcome: Outcome::Fault,
            body: Vec::new(),
        }
    }

    /// A one-way call's: nothing is sent back.
    pub fn none() -> Self {
        Self {
            outcome: Outcome::None,
            body: Vec::new(),
        }
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome
    }

    pub(super) fn envelope(&self) -> Option<Envelope<'_>> {
        match self.outcome {
            Outcome::None => None,
            Outcome::Ok => Some(Envelope::Ok(&self.body)),
            Outcome::Err(id) => Some(Envelope::Err {
                id,
                body: &self.body,
            }),
            Outcome::Fault => Some(Envelope::Fault),
        }
    }
}

/// The other end of an async connection, as this side's code sees it: what
/// a generated async sender calls through, and what an async handler is
/// given. [`AsyncHandle`](crate::connection::AsyncHandle) implements it.
pub trait AsyncPeer: Send + Sync {
    /// The wire format both ends agreed on.
    type Format: WireFormat + Send + Sync;

    /// The format, to decode a reply or a request's params with.
    fn format(&self) -> &Self::Format;

    /// This connection's id.
    fn id(&self) -> ConnectionId;

    /// Call `call` on the peer's `protocol`. The request is sent before the
    /// future is first polled; awaiting it waits for the reply. Dropping the
    /// future gives up on the call, and a late reply is dropped.
    fn call<P: Serialize + ?Sized>(
        &self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
    ) -> impl Future<Output = Result<OwnedEnvelope, RuntimeError>> + Send;

    /// [`call`](Self::call), but give up after `timeout` with
    /// [`RuntimeError::Timeout`], using the connection's
    /// [`Timer`](crate::connection::AsyncConnection::set_timer). Without one it waits.
    fn call_with_timeout<P: Serialize + ?Sized>(
        &self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
        timeout: Duration,
    ) -> impl Future<Output = Result<OwnedEnvelope, RuntimeError>> + Send;

    /// Send a one-way call: no reply is awaited.
    fn notify<P: Serialize + ?Sized>(
        &self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
    ) -> Result<(), RuntimeError>;

    /// Typed data the application keeps on this connection. A lock: drop
    /// the guard before awaiting.
    fn extensions(&self) -> MutexGuard<'_, Extensions>;
}

/// The provider side of a protocol on an [`AsyncConnection`](crate::connection::AsyncConnection): the async
/// counterpart of [`Dispatch`](crate::contract::Dispatch). The generated
/// `<Proto>AsyncDispatcher` implements it.
pub trait AsyncDispatch: Send + Sync + 'static {
    /// The protocol's function names, in declaration order.
    fn calls(&self) -> &'static [&'static str];

    /// Run the handler for `call` with its encoded `params`, decoded with
    /// `peer.format()`.
    fn dispatch<'a, P: AsyncPeer>(
        &'a self,
        call: Kind,
        params: &'a [u8],
        peer: &'a P,
    ) -> impl Future<Output = Result<Answer, RuntimeError>> + Send + 'a;
}
