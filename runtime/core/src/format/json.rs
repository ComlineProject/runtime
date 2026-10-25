use core::marker::PhantomData;

use serde::de::DeserializeSeed;
use serde::{Deserialize, Serialize};

use super::BufWriter;
use crate::contract::{BufMut, RuntimeError, WireFormat};

/// [`WireFormat`] over JSON (`serde_json`). Verbose next to MessagePack, but
/// the pairing for [`JsonRpcFraming`](crate::framing::JsonRpcFraming) and handy
/// for debugging on the wire.
#[derive(Debug, Default, Clone, Copy)]
pub struct Json;

impl WireFormat for Json {
    fn name(&self) -> &'static str {
        crate::contract::FORMAT_JSON
    }

    fn encode<T: Serialize + ?Sized>(
        &self,
        value: &T,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError> {
        serde_json::to_writer(BufWriter(out), value).map_err(|_| RuntimeError::Serialization)
    }

    fn decode<'de, T: Deserialize<'de>>(&self, bytes: &'de [u8]) -> Result<T, RuntimeError> {
        self.decode_seed(PhantomData::<T>, bytes)
    }
}

impl Json {
    /// [`decode`](WireFormat::decode) through a [`DeserializeSeed`], for a
    /// caller whose type is known only at runtime: a seed that carries the
    /// schema's type can read a value with no Rust type of its own. This is
    /// `serde_json::from_slice` spelled out, so it is just as strict: after
    /// the value only whitespace may follow.
    pub fn decode_seed<'de, S: DeserializeSeed<'de>>(
        &self,
        seed: S,
        bytes: &'de [u8],
    ) -> Result<S::Value, RuntimeError> {
        let mut de = serde_json::Deserializer::from_slice(bytes);
        let value = seed
            .deserialize(&mut de)
            .map_err(|_| RuntimeError::Serialization)?;
        de.end().map_err(|_| RuntimeError::Serialization)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Msg<'a> {
        n: u32,
        #[serde(borrow)]
        s: &'a str,
    }

    #[test]
    fn round_trips_borrowed() {
        let mut buf = Vec::new();
        Json.encode(&Msg { n: 7, s: "hi" }, &mut buf).unwrap();
        assert_eq!(buf, br#"{"n":7,"s":"hi"}"#);

        let back: Msg = Json.decode(&buf).unwrap();
        assert_eq!(back, Msg { n: 7, s: "hi" });
    }

    /// serde_json's default float parser is off by one ulp here; the
    /// `float_roundtrip` feature reads every written `f64` back exactly.
    /// Found by `conformance/tests/properties.rs`.
    #[test]
    fn an_f64_reads_back_exactly() {
        let x = 4.520618856987477e-279_f64;
        let mut buf = Vec::new();
        Json.encode(&x, &mut buf).unwrap();
        assert_eq!(Json.decode::<f64>(&buf).map(f64::to_bits), Ok(x.to_bits()));
    }

    #[test]
    fn garbage_is_a_serialization_error() {
        let err = Json.decode::<u32>(b"not json").unwrap_err();
        assert_eq!(err, RuntimeError::Serialization);
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
    fn same_both_ways(fmt: &Json, bytes: &[u8]) -> Result<Rich, RuntimeError> {
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
            Json.encode(&value, &mut buf).unwrap();
            assert_eq!(same_both_ways(&Json, &buf), Ok(value));
        }
    }

    #[test]
    fn decode_seed_refuses_what_decode_refuses() {
        let mut buf = Vec::new();
        Json.encode(&rich_values().remove(1), &mut buf).unwrap();

        let garbage: &[u8] = b"not json";
        let truncated = &buf[..buf.len() - 1];
        let mut trailing = buf.clone();
        trailing.extend_from_slice(b" 1");
        for bad in [garbage, truncated, &trailing] {
            assert_eq!(same_both_ways(&Json, bad), Err(RuntimeError::Serialization));
        }

        // As in `serde_json::from_slice`, trailing whitespace is fine.
        let mut spaced = buf.clone();
        spaced.extend_from_slice(b" \n");
        assert_eq!(same_both_ways(&Json, &spaced), Ok(rich_values().remove(1)));
    }

    #[test]
    fn a_seed_decodes_with_the_type_known_at_runtime() {
        assert_eq!(Json.decode_seed(Width(1), b"200"), Ok(200));
        assert_eq!(Json.decode_seed(Width(4), b"300"), Ok(300));
        assert_eq!(
            Json.decode_seed(Width(1), b"300"),
            Err(RuntimeError::Serialization)
        );
        assert_eq!(
            Json.decode_seed(Width(4), b"300 1"),
            Err(RuntimeError::Serialization)
        );
    }
}
