//! Connections: calls in both directions over one transport.
//!
//! [`Client`](crate::client::Client) and [`Server`](crate::serve::Server)
//! work in lockstep: one end asks, the other answers. On a [`Connection`]
//! either end can call the other, a side waiting for a reply keeps serving
//! the peer's requests (so calls nest: a handler can call back the peer that
//! called it), and several protocols share the connection, each under its
//! [`ProtocolNumber`]. It needs a [`ConnectionFraming`], whose frames say
//! whether they're a request or a reply: [`Datagram2Framing`] (`datagram@2`),
//! or [`JsonRpcFraming`](crate::framing::JsonRpcFraming), whose requests name
//! only their method, so the protocol is the one this side provides with a
//! call by that name.
//!
//! Two layers:
//! - [`State`] is the transport-independent core: it hands out request ids,
//!   tracks the calls waiting for a reply, and turns a received frame into an
//!   [`Event`]. It does no I/O, so a blocking, an async or a `no_std` driver
//!   can share it.
//! - [`Connection`] is the blocking driver: it owns the transport, routes
//!   requests to the providers registered on it, and replies to the calls
//!   waiting on them.
//!
//! Who dialled and who answered only matters for the handshake
//! ([`connect`](Connection::connect) runs the consumer's side,
//! [`accept`](Connection::accept) the provider's); after that both ends are
//! the same.
//!
//! [`ProtocolNumber`]: crate::contract::ProtocolNumber
//! [`ConnectionFraming`]: crate::contract::ConnectionFraming
//! [`Datagram2Framing`]: crate::contract::Datagram2Framing
#![cfg_attr(
    feature = "async",
    doc = r#"
# Async

The `async` feature adds a driver for connections on any executor.
[`Connection`] is blocking: one thread serves, and calls the peer only
between requests. An [`AsyncConnection`] is a future the application
spawns ([`run`](AsyncConnection::run)) plus cloneable [`AsyncHandle`]s,
so any task can call the peer while the connection serves it. It shares
the transport-independent [`State`] with the blocking driver, and needs
nothing from an executor but polling: handlers run inside `run`'s
future, not as spawned tasks.

- **Transport:** anything that's a [`Stream`] of frames and a [`Sink`]
  for them ([`AsyncTransport`]). [`async_duplex`] is in-memory; the
  `tokio` feature adds TCP (`transport::tokio::Tcp`).
- **Handshake:** [`connect`](AsyncConnection::connect) /
  [`accept`](AsyncConnection::accept) run `comline@1`;
  [`connect_with`](AsyncConnection::connect_with) /
  [`accept_with`](AsyncConnection::accept_with) run any
  [`AsyncHandshake`], a custom one included, as the blocking
  [`Connection`]'s do a [`Handshake`](crate::handshake::Handshake).
- **Providers** implement [`AsyncDispatch`]; their handlers get an
  [`AsyncPeer`] to call back the peer that called them.
- **Order:** the peer's requests are handled one at a time, in order,
  except while every running handler waits on a call to the peer: then
  the next starts, since it may be that call's nested request.
  [`concurrent`](AsyncConnection::concurrent) lets several run at once.
- **Timeouts** need a clock, which the executor has:
  [`set_timer`](AsyncConnection::set_timer). Without one,
  [`AsyncPeer::call_with_timeout`] waits. Dropping a call's future gives up
  on it too, so the executor's own timeout works as well.
- **Frame limits:** a call over the connection's limit
  ([`set_max_frame`](AsyncConnection::set_max_frame), by default
  [`DEFAULT_MAX_FRAME`]) fails alone with
  [`RuntimeError::FrameTooLarge`] before anything is queued, and a
  handler's reply over it is sent as a fault. A frame over the transport's
  limit arriving from the peer ends [`run`](AsyncConnection::run) with
  that error.

[`Stream`]: futures_util::Stream
[`Sink`]: futures_util::Sink
[`AsyncTransport`]: crate::transport::AsyncTransport
[`async_duplex`]: crate::transport::async_duplex
[`AsyncHandshake`]: crate::handshake::AsyncHandshake
[`DEFAULT_MAX_FRAME`]: crate::contract::stream::DEFAULT_MAX_FRAME
[`RuntimeError::FrameTooLarge`]: crate::contract::RuntimeError::FrameTooLarge
"#
)]

#[cfg(feature = "async")]
mod asynchronous;
mod blocking;
mod extensions;
mod state;
#[cfg(test)]
mod tests;

#[cfg(feature = "async")]
pub use asynchronous::{
    Answer, AsyncConnection, AsyncDispatch, AsyncHandle, AsyncPeer, BoxFuture, OwnedEnvelope, Timer,
};
pub use blocking::{Connection, Handle};
pub use extensions::Extensions;
pub use state::{Event, State};
