use serde::{Deserialize, Serialize};

use crate::contract::{BufMut, RuntimeError};

/// The message-serialization axis. Generated code is `serde`-only; a concrete
/// `WireFormat` (MessagePack, JSON, …) is chosen once at setup.
///
/// `encode` appends into a caller-owned buffer — it never returns a `Vec`.
/// `decode` borrows from the input, so a generated `Msg<'de>` points straight
/// into the receive buffer.
pub trait WireFormat {
    fn encode<T: Serialize + ?Sized>(
        &self,
        value: &T,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError>;

    fn decode<'de, T: Deserialize<'de>>(&self, bytes: &'de [u8]) -> Result<T, RuntimeError>;
}
