use serde::{Deserialize, Serialize};

use crate::contract::{BufMut, CountBuf, RuntimeError};

/// The message-serialization axis. Generated code is `serde`-only; a concrete
/// `WireFormat` (MessagePack, JSON, …) is chosen once at setup.
///
/// `encode` appends into a caller-owned buffer — it never returns a `Vec`.
/// `decode` borrows from the input, so a generated `Msg<'de>` points straight
/// into the receive buffer.
pub trait WireFormat {
    /// A stable name for this format — the connection
    /// [`HandshakeFrame`](crate::contract::HandshakeFrame) folds it in (hashed) so the
    /// two ends can catch "one speaks MessagePack, one speaks JSON". Names are
    /// versioned, `name@major` (`"msgpack@1"`); a third-party format should
    /// namespace it (`"com.acme.myformat@1"`) to avoid a clash.
    fn name(&self) -> &'static str;

    fn encode<T: Serialize + ?Sized>(
        &self,
        value: &T,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError>;

    fn decode<'de, T: Deserialize<'de>>(&self, bytes: &'de [u8]) -> Result<T, RuntimeError>;

    /// The exact number of bytes [`encode`](WireFormat::encode) writes for
    /// `value`, or the error it fails with (a value over its bounds).
    ///
    /// It runs `encode` itself into a [`CountBuf`], which keeps nothing and
    /// only counts, so it can't differ from what `encode` writes, and needs
    /// neither `alloc` nor a buffer. A format writing through `std::io::Write`
    /// (JSON, MessagePack) counts there too: its writer is over the same sink.
    /// Use it to size a buffer before encoding into it, or to check a value
    /// against a frame limit; it costs a full encode, so don't call it to
    /// then encode on the success path.
    fn encoded_len<T: Serialize + ?Sized>(&self, value: &T) -> Result<usize, RuntimeError> {
        let mut count = CountBuf::new();
        self.encode(value, &mut count)?;
        Ok(count.len())
    }
}
