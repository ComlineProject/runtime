use std::io::Write;

use serde::{Deserialize, Serialize};

use crate::contract::{BufMut, RuntimeError, WireFormat};

/// [`WireFormat`] over MessagePack (`rmp-serde`): compact, `serde`-native,
/// borrows on decode. Structs encode positionally (array form) — the
/// append-only call/field discipline keeps both ends in step.
#[derive(Debug, Default, Clone, Copy)]
pub struct MsgPack;

/// `std::io::Write` over a `BufMut`, so `encode` serialises straight into the
/// caller's buffer with no intermediate `Vec`.
struct BufWriter<'a>(&'a mut dyn BufMut);

impl Write for BufWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.put_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl WireFormat for MsgPack {
    fn name(&self) -> &'static str {
        "msgpack"
    }

    fn encode<T: Serialize + ?Sized>(
        &self,
        value: &T,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError> {
        let mut serializer = rmp_serde::Serializer::new(BufWriter(out));
        value
            .serialize(&mut serializer)
            .map_err(|_| RuntimeError::Serialization)
    }

    fn decode<'de, T: Deserialize<'de>>(&self, bytes: &'de [u8]) -> Result<T, RuntimeError> {
        rmp_serde::from_slice(bytes).map_err(|_| RuntimeError::Serialization)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Owned {
        n: u32,
        s: String,
        xs: Vec<u8>,
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Borrowed<'a> {
        n: u32,
        #[serde(borrow)]
        s: &'a str,
    }

    #[test]
    fn round_trips_an_owned_struct() {
        let value = Owned {
            n: 7,
            s: "hi".into(),
            xs: vec![1, 2, 3],
        };
        let mut buf = Vec::new();
        MsgPack.encode(&value, &mut buf).unwrap();

        let back: Owned = MsgPack.decode(&buf).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn decode_borrows_from_the_input() {
        let mut buf = Vec::new();
        MsgPack.encode(&Borrowed { n: 1, s: "borrowed" }, &mut buf).unwrap();

        let back: Borrowed = MsgPack.decode(&buf).unwrap();
        assert_eq!(back.s, "borrowed"); // `back.s` points into `buf`
    }

    #[test]
    fn garbage_is_a_serialization_error() {
        let err = MsgPack.decode::<Owned>(&[0xc1, 0x00, 0x13]).unwrap_err();
        assert_eq!(err, RuntimeError::Serialization);
    }

    #[test]
    fn encode_appends_no_intermediate_alloc() {
        // Encoding into a pre-sized buffer must not grow it (proxy for
        // "one pass, straight into the buffer").
        let mut buf = Vec::with_capacity(64);
        let ptr = buf.as_ptr();
        MsgPack.encode(&(1u8, "x"), &mut buf).unwrap();
        assert_eq!(buf.as_ptr(), ptr, "buffer reallocated");
    }
}
