use core::time::Duration;

use serde::Serialize;

use crate::contract::{Call, Envelope, ProtocolNumber, RuntimeError, WireFormat};

/// A connection's id, unique within the process. The runtime assigns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnectionId(pub u64);

/// The other end of a connection, as this side's code sees it: what a
/// generated sender calls through, and what a handler is given to call back
/// the peer that called it. [`Connection`](crate::connection::Connection)
/// implements it, and so does the handle its handlers receive.
///
/// A call waits for its reply, and while it waits this side keeps serving
/// the peer's requests, so calls can nest in both directions.
pub trait Peer {
    /// The wire format both ends agreed on.
    type Format: WireFormat;

    /// The format, to decode a reply or a request's params with.
    fn format(&self) -> &Self::Format;

    /// This connection's id.
    fn id(&self) -> ConnectionId;

    /// Call `call` on the peer's `protocol` and wait for the reply: its
    /// [`Envelope`], with the format to decode it. The envelope borrows this
    /// side's receive buffer, so it has to be decoded (or dropped) before the
    /// next call.
    fn call<P>(
        &mut self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
    ) -> Result<(Envelope<'_>, &Self::Format), RuntimeError>
    where
        P: Serialize + ?Sized;

    /// [`call`](Self::call), but give up after `timeout` with
    /// [`RuntimeError::Timeout`]. Honoured by transports that override
    /// [`Transport::recv_timeout`](crate::transport::Transport::recv_timeout)
    /// and on `std`; elsewhere it waits.
    fn call_with_timeout<P>(
        &mut self,
        protocol: ProtocolNumber,
        call: Call,
        params: &P,
        timeout: Duration,
    ) -> Result<(Envelope<'_>, &Self::Format), RuntimeError>
    where
        P: Serialize + ?Sized;

    /// Send a one-way call: no reply is awaited.
    fn notify<P>(&mut self, protocol: ProtocolNumber, call: Call, params: &P) -> Result<(), RuntimeError>
    where
        P: Serialize + ?Sized;

    /// Typed data the application keeps on this connection: a session, a
    /// user id, a rate-limit bucket.
    #[cfg(feature = "alloc")]
    fn extensions(&mut self) -> &mut crate::connection::Extensions;
}
