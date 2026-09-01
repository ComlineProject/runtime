use serde::{Deserialize, Serialize};

use crate::contract::{BufMut, RuntimeError};

/// The message-serialization axis. Generated code is `serde`-only; a concrete
/// `WireFormat` (MessagePack, JSON, …) is chosen once at setup.
///
/// `encode` appends into a caller-owned buffer — it never returns a `Vec`.
/// `decode` borrows from the input, so a generated `Msg<'de>` points straight
/// into the receive buffer.
pub trait WireFormat {
    /// A stable name for this format — the connection
    /// [`Handshake`](crate::contract::Handshake) folds it in (hashed) so the
    /// two ends can catch "one speaks MessagePack, one speaks JSON". Built-ins
    /// are bare (`"msgpack"`); a third-party format should namespace it
    /// (`"com.acme.myformat"`) to avoid a clash.
    fn name(&self) -> &'static str;

    fn encode<T: Serialize + ?Sized>(
        &self,
        value: &T,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError>;

    fn decode<'de, T: Deserialize<'de>>(&self, bytes: &'de [u8]) -> Result<T, RuntimeError>;
}
