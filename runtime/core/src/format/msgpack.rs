use core::marker::PhantomData;

use serde::de::DeserializeSeed;
use serde::{Deserialize, Serialize};

use super::BufWriter;
use crate::contract::{BufMut, RuntimeError, WireFormat};

/// [`WireFormat`] over MessagePack (`rmp-serde`): compact, `serde`-native,
/// borrows on decode. Structs encode positionally (array form) — the
/// append-only call/field discipline keeps both ends in step.
#[derive(Debug, Default, Clone, Copy)]
pub struct MsgPack;

impl WireFormat for MsgPack {
    fn name(&self) -> &'static str {
        crate::contract::FORMAT_MSGPACK
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
        self.decode_seed(PhantomData::<T>, bytes)
    }
}

impl MsgPack {
    /// [`decode`](WireFormat::decode) through a [`DeserializeSeed`], for a
    /// caller whose type is known only at runtime: a seed that carries the
    /// schema's type can read a value with no Rust type of its own. This is
    /// `rmp_serde::from_slice` spelled out: the same constructor, with the
    /// default config (not human-readable) and its
    /// nesting limit of 1024.
    ///
    /// Unlike [`Packed`](super::Packed) and [`Json`](super::Json), it does not
    /// check for trailing bytes: `rmp_serde::from_slice` reads one value and
    /// ignores whatever follows it, and this keeps that behaviour as is.
    pub fn decode_seed<'de, S: DeserializeSeed<'de>>(
        &self,
        seed: S,
        bytes: &'de [u8],
    ) -> Result<S::Value, RuntimeError> {
        let mut de = rmp_serde::Deserializer::from_read_ref(bytes);
        seed.deserialize(&mut de)
            .map_err(|_| RuntimeError::Serialization)
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
        MsgPack
            .encode(
                &Borrowed {
                    n: 1,
                    s: "borrowed",
                },
                &mut buf,
            )
            .unwrap();

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

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    enum Shape {
        Dot,
        Line(u32),
        Box { w: u16, h: u16 },
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Rich {
        tag: Option<u8>,
        shape: Shape,
        xs: Vec<i32>,
        ip: [u8; 4],
    }

    fn rich_values() -> Vec<Rich> {
        vec![
            Rich {
                tag: None,
                shape: Shape::Dot,
                xs: vec![],
                ip: [127, 0, 0, 1],
            },
            Rich {
                tag: Some(9),
                shape: Shape::Line(70_000),
                xs: vec![-1, 0, 1],
                ip: [10, 0, 0, 2],
            },
            Rich {
                tag: Some(0),
                shape: Shape::Box { w: 3, h: 4 },
                xs: vec![i32::MIN],
                ip: [255; 4],
            },
        ]
    }

    /// The seeded and derived paths decode the same bytes to the same result.
    fn same_both_ways(fmt: &MsgPack, bytes: &[u8]) -> Result<Rich, RuntimeError> {
        let derived = fmt.decode::<Rich>(bytes);
        let seeded = fmt.decode_seed(PhantomData::<Rich>, bytes);
        assert_eq!(
            derived, seeded,
            "decode and decode_seed differ on {bytes:02x?}"
        );
        derived
    }

    /// An integer whose width is chosen at runtime: a seed with no Rust type
    /// of its own for the value it reads.
    struct Width(u8);

    impl<'de> DeserializeSeed<'de> for Width {
        type Value = u64;
        fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<u64, D::Error> {
            match self.0 {
                1 => u8::deserialize(d).map(u64::from),
                2 => u16::deserialize(d).map(u64::from),
                _ => u32::deserialize(d).map(u64::from),
            }
        }
    }

    #[test]
    fn decode_seed_equals_decode() {
        for value in rich_values() {
            let mut buf = Vec::new();
            MsgPack.encode(&value, &mut buf).unwrap();
            assert_eq!(same_both_ways(&MsgPack, &buf), Ok(value));
        }
    }

    #[test]
    fn decode_seed_refuses_what_decode_refuses() {
        let mut buf = Vec::new();
        MsgPack.encode(&rich_values().remove(1), &mut buf).unwrap();

        let garbage: &[u8] = &[0xc1, 0x00, 0x13];
        let truncated = &buf[..buf.len() - 1];
        let empty: &[u8] = &[];
        for bad in [garbage, truncated, empty] {
            assert_eq!(
                same_both_ways(&MsgPack, bad),
                Err(RuntimeError::Serialization)
            );
        }
    }

    /// `rmp_serde::from_slice` reads one value and ignores the rest, and
    /// `decode` keeps that; `decode_seed` must agree with it, bytes left over
    /// included.
    #[test]
    fn trailing_bytes_are_ignored_both_ways() {
        let mut buf = Vec::new();
        MsgPack.encode(&rich_values().remove(1), &mut buf).unwrap();
        buf.extend_from_slice(&[0x01, 0xc1]);
        assert_eq!(same_both_ways(&MsgPack, &buf), Ok(rich_values().remove(1)));
    }

    #[test]
    fn a_seed_decodes_with_the_type_known_at_runtime() {
        let mut buf = Vec::new();
        MsgPack.encode(&300u32, &mut buf).unwrap();
        assert_eq!(MsgPack.decode_seed(Width(2), &buf), Ok(300));
        assert_eq!(MsgPack.decode_seed(Width(4), &buf), Ok(300));
        assert_eq!(
            MsgPack.decode_seed(Width(1), &buf),
            Err(RuntimeError::Serialization)
        );
        assert_eq!(MsgPack.decode_seed(Width(1), &[0x07]), Ok(7));
    }
}
