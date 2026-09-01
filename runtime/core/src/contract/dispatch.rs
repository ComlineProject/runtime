use crate::contract::{BufMut, RuntimeError, WireFormat};

/// How a call is addressed on the wire.
///
/// `Id` is the compact form — a function's position in its protocol's
/// declaration order, which is append-only. `Named` is for name-oriented
/// framings (JSON-RPC) and diagnostics; the `&'static str` comes from generated
/// `calls_names()`, never an owned `String`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Id(u16),
    Named(&'static str),
}

/// The provider side of a protocol: given an inbound call and its encoded
/// params, run the user's handler and write the response [`Envelope`] into
/// `out`.
///
/// Sync — no boxed futures on the `no_std` path. An async server layer wraps
/// this behind the `std` feature. The generated `<Proto>Dispatcher` implements
/// it; the call system holds one by value / `&D` (generic — no vtable, no
/// `dyn`) and calls it with the format it was configured with.
///
/// [`Envelope`]: crate::contract::Envelope
pub trait Dispatch {
    fn dispatch<W: WireFormat>(
        &self,
        call: Kind,
        params: &[u8],
        format: &W,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError>;
}
