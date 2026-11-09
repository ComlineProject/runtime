use super::*;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use serde::Serialize;

#[derive(Serialize, Deserialize, PartialEq, Debug)]
enum Class {
    Warrior,
    Mage,
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Player {
    id: u32,
    hp: u16,
    name: String,
    guild: Option<String>,
    class: Class,
    pos: [f32; 2],
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
enum Shape {
    Dot,
    Circle(f64),
    Rect { w: u16, h: u16 },
    Line(u8, u8),
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Borrowed<'a> {
    #[serde(borrow)]
    name: &'a str,
    #[serde(borrow, with = "serde_bytes_borrowed")]
    raw: &'a [u8],
}

mod serde_bytes_borrowed {
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(v)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<&'de [u8], D::Error> {
        <&[u8]>::deserialize(d)
    }

    use serde::Deserialize;
}

fn encode<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
    let mut out = Vec::new();
    Packed.encode(value, &mut out).unwrap();
    out
}

fn reason<'de, T: Deserialize<'de>>(bytes: &'de [u8]) -> Error {
    let mut de = Deserializer { input: bytes };
    match T::deserialize(&mut de) {
        Err(e) => e,
        Ok(_) if !de.input.is_empty() => {
            panic!("left {} bytes, which `decode` refuses", de.input.len())
        }
        Ok(_) => panic!("decoded"),
    }
}

#[test]
fn the_proposal_example_is_nineteen_bytes() {
    let player = Player {
        id: 7,
        hp: 100,
        name: "ab".into(),
        guild: None,
        class: Class::Mage,
        pos: [1.0, 2.0],
    };
    let bytes = encode(&player);
    assert_eq!(
        bytes,
        [7, 0, 0, 0, 100, 0, 2, b'a', b'b', 0, 1, 0, 0, 0x80, 0x3f, 0, 0, 0, 0x40]
    );
    assert_eq!(Packed.decode::<Player>(&bytes).unwrap(), player);
}

/// The decode path returns `Result<_, Error>` everywhere: a bigger
/// error makes each of them bigger, and decoding measurably slower.
#[test]
fn the_error_is_one_byte() {
    assert_eq!(core::mem::size_of::<Error>(), 1);
    assert_eq!(core::mem::size_of::<RuntimeError>(), 4);
}

#[test]
fn scalars_are_fixed_width_little_endian() {
    assert_eq!(encode(&true), [1]);
    assert_eq!(encode(&0x0102u16), [2, 1]);
    assert_eq!(encode(&-2i32), [0xfe, 0xff, 0xff, 0xff]);
    assert_eq!(encode(&1u64), [1, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(encode(&-0.25f64), (-0.25f64).to_le_bytes());
    assert_eq!(encode(&u128::MAX), [0xff; 16]);
}

#[test]
fn lengths_are_varints_and_fixed_arrays_have_none() {
    assert_eq!(encode(""), [0]);
    assert_eq!(encode(&vec![1u8, 2, 3]), [3, 1, 2, 3]);
    assert_eq!(encode(&[1u8, 2, 3]), [1, 2, 3]);
    let long = "x".repeat(300);
    assert_eq!(&encode(long.as_str())[..2], [0xac, 0x02]);
    assert_eq!(
        Packed.decode::<String>(&encode(long.as_str())).unwrap(),
        long
    );
}

#[test]
fn every_enum_variant_shape_round_trips() {
    for shape in [
        Shape::Dot,
        Shape::Circle(1.5),
        Shape::Rect { w: 3, h: 4 },
        Shape::Line(5, 6),
    ] {
        let bytes = encode(&shape);
        assert_eq!(Packed.decode::<Shape>(&bytes).unwrap(), shape);
    }
    assert_eq!(encode(&Shape::Rect { w: 3, h: 4 }), [2, 3, 0, 4, 0]);
}

#[test]
fn maps_round_trip() {
    let map: alloc::collections::BTreeMap<u8, bool> = [(1, true), (2, false)].into();
    let bytes = encode(&map);
    assert_eq!(bytes, [2, 1, 1, 2, 0]);
    assert_eq!(
        Packed
            .decode::<alloc::collections::BTreeMap<u8, bool>>(&bytes)
            .unwrap(),
        map
    );
}

#[test]
fn decode_borrows_from_the_input() {
    let bytes = encode(&Borrowed {
        name: "hi",
        raw: b"\x00\x01",
    });
    let back: Borrowed = Packed.decode(&bytes).unwrap();
    assert_eq!((back.name, back.raw), ("hi", &b"\x00\x01"[..]));
    assert!(bytes.as_ptr_range().contains(&back.name.as_ptr()));
}

#[test]
fn encode_writes_into_a_fixed_buffer() {
    let mut storage = [0u8; 8];
    let mut buf = crate::contract::SliceBuf::new(&mut storage);
    Packed.encode(&(1u16, "ab"), &mut buf).unwrap();
    assert_eq!(buf.written(), [1, 0, 2, b'a', b'b']);
}

#[test]
fn refuses_what_isnt_a_value() {
    assert_eq!(reason::<bool>(&[2]), Error::BadTag);
    assert_eq!(reason::<Option<u8>>(&[2, 0]), Error::BadTag);
    assert_eq!(reason::<u32>(&[1, 2, 3]), Error::Truncated);
    assert_eq!(
        reason::<String>(&[0x80, 0x00]),
        Error::BadVarint,
        "overlong"
    );
    assert_eq!(reason::<String>(&[0x80]), Error::Truncated);
    assert_eq!(
        reason::<String>(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02]),
        Error::BadVarint,
        "past 64 bits"
    );
    assert_eq!(reason::<String>(&[5, b'a', b'b']), Error::TooLong);
    assert_eq!(
        reason::<Vec<()>>(&[0xff, 0x01]),
        Error::TooLong,
        "a count, too"
    );
    assert_eq!(reason::<String>(&[2, 0xc3, 0x28]), Error::BadUtf8);
    assert_eq!(reason::<Class>(&[2]), Error::Custom, "no such variant");
}

#[test]
fn collected_strings_are_length_prefixed() {
    struct Shown;
    impl Serialize for Shown {
        fn serialize<S: ser::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            s.collect_str(&format_args!("{}-{}", 1, "ab"))
        }
    }
    assert_eq!(encode(&Shown), [4, b'1', b'-', b'a', b'b']);
}

#[test]
fn decode_refuses_trailing_bytes() {
    assert_eq!(
        Packed.decode::<u16>(&[1, 0, 0]),
        Err(RuntimeError::Serialization)
    );
    assert_eq!(Packed.decode::<u16>(&[1, 0]), Ok(1));
}

#[test]
fn refuses_what_it_cant_say() {
    #[derive(Serialize)]
    struct Skips {
        #[serde(skip_serializing_if = "Option::is_none")]
        a: Option<u8>,
    }
    let mut out = Vec::new();
    assert_eq!(
        Packed.encode(&Skips { a: None }, &mut out),
        Err(RuntimeError::Serialization)
    );

    struct Unsized;
    impl Serialize for Unsized {
        fn serialize<S: ser::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            use ser::SerializeSeq;
            s.serialize_seq(None)?.end()
        }
    }
    assert_eq!(
        Packed.encode(&Unsized, &mut out),
        Err(RuntimeError::Serialization)
    );

    #[derive(Deserialize, Debug)]
    #[serde(untagged)]
    #[allow(dead_code)]
    enum Untagged {
        A(u8),
    }
    assert_eq!(reason::<Untagged>(&[1]), Error::Unsupported);
}

/// A seed reads a value whose type is data, not a Rust type: here, a
/// struct of two fields whose widths come from a list.
#[test]
fn a_seed_decodes_with_the_type_known_at_runtime() {
    struct Widths(&'static [u8]);
    impl<'de> DeserializeSeed<'de> for Widths {
        type Value = Vec<u64>;
        fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Vec<u64>, D::Error> {
            struct V(&'static [u8]);
            impl<'de> Visitor<'de> for V {
                type Value = Vec<u64>;
                fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                    f.write_str("fields")
                }
                fn visit_seq<A: de::SeqAccess<'de>>(
                    self,
                    mut seq: A,
                ) -> Result<Vec<u64>, A::Error> {
                    let mut out = Vec::new();
                    for &width in self.0 {
                        out.push(match width {
                            1 => u64::from(seq.next_element::<u8>()?.unwrap()),
                            _ => u64::from(seq.next_element::<u32>()?.unwrap()),
                        });
                    }
                    Ok(out)
                }
            }
            d.deserialize_tuple(self.0.len(), V(self.0))
        }
    }
    let bytes = [0x07, 0x2a, 0x00, 0x00, 0x00];
    assert_eq!(Packed.decode_seed(Widths(&[1, 4]), &bytes), Ok(vec![7, 42]));
    // As strict as `decode`: a byte left over is an error.
    assert_eq!(
        Packed.decode_seed(Widths(&[1, 4]), &[0x07, 0x2a, 0, 0, 0, 0]),
        Err(RuntimeError::Serialization)
    );
}
