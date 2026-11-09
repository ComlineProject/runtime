//! The `core ↔ target` contract — runtime side.
//!
//! `no_std`, allocation-free: the shapes every generated protocol and every
//! `WireFormat` build against. Nothing here spends the call path on a heap
//! allocation or a payload copy.
//!
//! See `ComlineProject/docs` → Design → *The `core` ↔ target contract*, §4.

mod buf;
mod call;
mod dispatch;
mod envelope;
mod error;
mod framing;
mod handshake;
mod method_name;
mod names;
mod peer;
mod refusal;
pub mod stream;
mod wire;

pub use buf::{BufMut, CountBuf, SliceBuf};
pub use call::{CallError, Fault};
pub use dispatch::{Dispatch, Kind, Outcome, Reply};
pub use envelope::Envelope;
pub use error::RuntimeError;
pub use framing::{
    put_varint, take_varint, Call, ConnectionFraming, Datagram2Framing, Datagram3Framing,
    DatagramFraming, Frame, Framing, ProtocolNumber, Request, RequestCall,
};
pub use handshake::HandshakeFrame;
pub use method_name::{Chars as MethodNameChars, MethodName};
pub use names::{
    name_hash, FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED, FRAMING_DATAGRAM, FRAMING_DATAGRAM2,
    FRAMING_DATAGRAM3, FRAMING_JSONRPC, HANDSHAKE_COMLINE, HANDSHAKE_COMLINE2, HANDSHAKE_NONE,
};
pub use peer::{ConnectionId, Peer};
pub use refusal::{Reason, Side};
pub use wire::WireFormat;
