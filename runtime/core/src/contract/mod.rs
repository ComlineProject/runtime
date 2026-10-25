//! The `core ↔ target` contract — runtime side.
//!
//! `no_std`, allocation-free: the shapes every generated protocol and every
//! `WireFormat` build against. Nothing here spends the call path on a heap
//! allocation or a payload copy.
//!
//! See `ComlineProject/docs` → Design → *The `core` ↔ target contract*, §4.

mod buf;
mod call;
mod connection_framing;
mod dispatch;
mod envelope;
mod error;
mod framing;
mod handshake;
mod method_name;
mod peer;
pub mod stream;
mod wire;

pub use buf::{BufMut, CountBuf, SliceBuf};
pub use call::{CallError, Fault};
pub use connection_framing::{ConnectionFraming, Datagram2Framing, Frame, ProtocolNumber};
pub use dispatch::{Dispatch, Kind, Outcome, Reply};
pub use envelope::Envelope;
pub use error::RuntimeError;
pub use framing::{Call, DatagramFraming, Framing, Request, RequestCall};
pub use handshake::{
    name_hash, HandshakeFrame, FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED, FRAMING_DATAGRAM,
    FRAMING_DATAGRAM2, FRAMING_JSONRPC, HANDSHAKE_COMLINE, HANDSHAKE_NONE,
};
pub use method_name::{Chars as MethodNameChars, MethodName};
pub use peer::{ConnectionId, Peer};
pub use wire::WireFormat;
