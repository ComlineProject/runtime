//! `packed@1`: a value is its fields back to back, like a packed C struct.
//!
//! - Integers and floats are fixed-width little-endian; `bool` is one byte,
//!   `00` or `01`.
//! - Strings and byte strings are a varint length, then the bytes.
//! - Sequences and maps are a varint count, then the elements; tuples and
//!   fixed-size arrays have no count, their length is in the type.
//! - An option is `00`, or `01` and the value.
//! - A struct is its fields in declaration order, with no names or header.
//! - An enum is its variant index, as a varint, then the variant's fields.
//!
//! Varints are unsigned LEB128 in their shortest form; any other spelling is
//! refused, so a value has one encoding. Decoding is strict: a value must use
//! every byte it is given, and a length or count larger than the bytes left
//! is refused before anything is allocated.
//!
//! Needs neither `std` nor `alloc`: encoding writes straight into the
//! caller's [`BufMut`], and decoding borrows strings and bytes from the
//! input.

use core::fmt;

use serde::de::{self, DeserializeSeed, IntoDeserializer, Visitor};
use serde::ser::{self, Serialize};
use serde::Deserialize;

use crate::contract::{BufMut, RuntimeError, WireFormat};

/// [`WireFormat`] for `packed@1`: the smallest and fastest of the formats,
/// for peers that share the schema. Positional, like
/// [`MsgPack`](super::MsgPack): fields are told apart by their order.
#[derive(Debug, Default, Clone, Copy)]
pub struct Packed;

impl WireFormat for Packed {
    fn name(&self) -> &'static str {
        crate::contract::FORMAT_PACKED
    }

    fn encode<T: Serialize + ?Sized>(
        &self,
        value: &T,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError> {
        value
            .serialize(&mut Serializer { out })
            .map_err(|_| RuntimeError::Serialization)
    }

    fn decode<'de, T: Deserialize<'de>>(&self, bytes: &'de [u8]) -> Result<T, RuntimeError> {
        self.decode_seed(core::marker::PhantomData::<T>, bytes)
    }
}

impl Packed {
    /// [`decode`](WireFormat::decode) through a [`DeserializeSeed`], for a
    /// caller whose type is known only at runtime: a seed that carries the
    /// schema's type can read a value with no Rust type of its own. Just as
    /// strict: the value must use every byte.
    pub fn decode_seed<'de, S: DeserializeSeed<'de>>(
        &self,
        seed: S,
        bytes: &'de [u8],
    ) -> Result<S::Value, RuntimeError> {
        let mut de = Deserializer { input: bytes };
        let value = seed.deserialize(&mut de).map_err(|_| RuntimeError::Serialization)?;
        if !de.input.is_empty() {
            return Err(RuntimeError::Serialization);
        }
        Ok(value)
    }
}

/// Why encoding or decoding failed. Callers only ever see
/// [`RuntimeError::Serialization`]; the reason is for this module's tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Error {
    /// The input ended inside a value.
    Truncated,
    /// A `bool` byte other than `00`/`01`, or an option tag other than those.
    BadTag,
    /// A varint longer than its value needs, or past 64 bits.
    BadVarint,
    /// A length or count larger than the bytes left.
    TooLong,
    /// A string that isn't UTF-8.
    BadUtf8,
    /// Something `packed@1` can't say: a sequence of unknown length, a
    /// skipped field, a self-describing read.
    Unsupported,
    /// A message from the value's own `Serialize` or `Deserialize`.
    Custom,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Error::Truncated => "input ended inside a value",
            Error::BadTag => "a bool or option tag other than 00 or 01",
            Error::BadVarint => "a varint not in its shortest form",
            Error::TooLong => "a length larger than the bytes left",
            Error::BadUtf8 => "a string that isn't UTF-8",
            Error::Unsupported => "a shape packed@1 can't encode",
            Error::Custom => "the value refused itself",
        })
    }
}

impl de::StdError for Error {}

impl ser::Error for Error {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Error::Custom
    }
}

impl de::Error for Error {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Error::Custom
    }
}

fn put_varint(mut value: u64, out: &mut dyn BufMut) {
    while value >= 0x80 {
        out.put_u8((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.put_u8(value as u8);
}

fn put_len(len: usize, out: &mut dyn BufMut) {
    put_varint(len as u64, out);
}

// ── encoding ────────────────────────────────────────────────────────────────

struct Serializer<'a> {
    out: &'a mut dyn BufMut,
}

impl ser::Serializer for &mut Serializer<'_> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn is_human_readable(&self) -> bool {
        false
    }

    fn serialize_bool(self, v: bool) -> Result<(), Error> {
        self.out.put_u8(v as u8);
        Ok(())
    }

    fn serialize_i8(self, v: i8) -> Result<(), Error> {
        self.out.put_slice(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_i16(self, v: i16) -> Result<(), Error> {
        self.out.put_slice(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_i32(self, v: i32) -> Result<(), Error> {
        self.out.put_slice(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_i64(self, v: i64) -> Result<(), Error> {
        self.out.put_slice(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_i128(self, v: i128) -> Result<(), Error> {
        self.out.put_slice(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_u8(self, v: u8) -> Result<(), Error> {
        self.out.put_u8(v);
        Ok(())
    }

    fn serialize_u16(self, v: u16) -> Result<(), Error> {
        self.out.put_u16_le(v);
        Ok(())
    }

    fn serialize_u32(self, v: u32) -> Result<(), Error> {
        self.out.put_u32_le(v);
        Ok(())
    }

    fn serialize_u64(self, v: u64) -> Result<(), Error> {
        self.out.put_u64_le(v);
        Ok(())
    }

    fn serialize_u128(self, v: u128) -> Result<(), Error> {
        self.out.put_slice(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_f32(self, v: f32) -> Result<(), Error> {
        self.out.put_slice(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_f64(self, v: f64) -> Result<(), Error> {
        self.out.put_slice(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_char(self, v: char) -> Result<(), Error> {
        self.serialize_str(v.encode_utf8(&mut [0; 4]))
    }

    fn serialize_str(self, v: &str) -> Result<(), Error> {
        self.serialize_bytes(v.as_bytes())
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<(), Error> {
        put_len(v.len(), self.out);
        self.out.put_slice(v);
        Ok(())
    }

    /// Formats twice, once to learn the length that goes first: nothing to
    /// allocate a `String` with here.
    fn collect_str<T: fmt::Display + ?Sized>(self, value: &T) -> Result<(), Error> {
        struct Count(usize);
        impl fmt::Write for Count {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                self.0 += s.len();
                Ok(())
            }
        }
        struct Put<'a>(&'a mut dyn BufMut);
        impl fmt::Write for Put<'_> {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                self.0.put_slice(s.as_bytes());
                Ok(())
            }
        }
        let mut count = Count(0);
        fmt::write(&mut count, format_args!("{value}")).map_err(|_| Error::Custom)?;
        put_len(count.0, self.out);
        fmt::write(&mut Put(self.out), format_args!("{value}")).map_err(|_| Error::Custom)
    }

    fn serialize_none(self) -> Result<(), Error> {
        self.out.put_u8(0);
        Ok(())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Error> {
        self.out.put_u8(1);
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<(), Error> {
        Ok(())
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Error> {
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
    ) -> Result<(), Error> {
        put_varint(index.into(), self.out);
        Ok(())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        put_varint(index.into(), self.out);
        value.serialize(self)
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self, Error> {
        put_len(len.ok_or(Error::Unsupported)?, self.out);
        Ok(self)
    }

    fn serialize_tuple(self, _: usize) -> Result<Self, Error> {
        Ok(self)
    }

    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, Error> {
        Ok(self)
    }

    fn serialize_tuple_variant(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Error> {
        put_varint(index.into(), self.out);
        Ok(self)
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self, Error> {
        put_len(len.ok_or(Error::Unsupported)?, self.out);
        Ok(self)
    }

    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, Error> {
        Ok(self)
    }

    fn serialize_struct_variant(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Error> {
        put_varint(index.into(), self.out);
        Ok(self)
    }
}

/// Every compound writes its parts back to back; only the opening differs.
macro_rules! back_to_back {
    ($($trait:ident :: $method:ident),*) => {$(
        impl ser::$trait for &mut Serializer<'_> {
            type Ok = ();
            type Error = Error;

            fn $method<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
                value.serialize(&mut **self)
            }

            fn end(self) -> Result<(), Error> {
                Ok(())
            }
        }
    )*};
}

back_to_back!(
    SerializeSeq::serialize_element,
    SerializeTuple::serialize_element,
    SerializeTupleStruct::serialize_field,
    SerializeTupleVariant::serialize_field
);

impl ser::SerializeMap for &mut Serializer<'_> {
    type Ok = ();
    type Error = Error;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Error> {
        key.serialize(&mut **self)
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result<(), Error> {
        Ok(())
    }
}

impl ser::SerializeStruct for &mut Serializer<'_> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        value.serialize(&mut **self)
    }

    /// A skipped field would shift every field after it.
    fn skip_field(&mut self, _: &'static str) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    fn end(self) -> Result<(), Error> {
        Ok(())
    }
}

impl ser::SerializeStructVariant for &mut Serializer<'_> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        value.serialize(&mut **self)
    }

    fn skip_field(&mut self, _: &'static str) -> Result<(), Error> {
        Err(Error::Unsupported)
    }

    fn end(self) -> Result<(), Error> {
        Ok(())
    }
}

// ── decoding ────────────────────────────────────────────────────────────────

struct Deserializer<'de> {
    input: &'de [u8],
}

impl<'de> Deserializer<'de> {
    fn take(&mut self, n: usize) -> Result<&'de [u8], Error> {
        let (head, rest) = self.input.split_at_checked(n).ok_or(Error::Truncated)?;
        self.input = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        Ok(self.take(N)?.try_into().expect("took N bytes"))
    }

    fn varint(&mut self) -> Result<u64, Error> {
        let mut value: u64 = 0;
        for i in 0..10 {
            let [byte] = self.array()?;
            // The tenth byte holds bit 63 alone.
            if i == 9 && byte > 1 {
                return Err(Error::BadVarint);
            }
            value |= u64::from(byte & 0x7f) << (7 * i);
            if byte & 0x80 == 0 {
                // A trailing zero byte would be a second spelling of a
                // shorter number.
                if i > 0 && byte == 0 {
                    return Err(Error::BadVarint);
                }
                return Ok(value);
            }
        }
        Err(Error::BadVarint)
    }

    /// A length or count: never more than the bytes left, so a hostile one
    /// can't make a reader reserve memory or spin.
    fn len(&mut self) -> Result<usize, Error> {
        let len = self.varint()?;
        if len > self.input.len() as u64 {
            return Err(Error::TooLong);
        }
        Ok(len as usize)
    }

    fn bytes(&mut self) -> Result<&'de [u8], Error> {
        let len = self.len()?;
        self.take(len)
    }

    fn str(&mut self) -> Result<&'de str, Error> {
        core::str::from_utf8(self.bytes()?).map_err(|_| Error::BadUtf8)
    }

    fn tag(&mut self) -> Result<bool, Error> {
        match self.array()? {
            [0] => Ok(false),
            [1] => Ok(true),
            _ => Err(Error::BadTag),
        }
    }
}

macro_rules! fixed {
    ($($method:ident => $visit:ident($ty:ty)),* $(,)?) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            visitor.$visit(<$ty>::from_le_bytes(self.array()?))
        }
    )*};
}

impl<'de> de::Deserializer<'de> for &mut Deserializer<'de> {
    type Error = Error;

    fn is_human_readable(&self) -> bool {
        false
    }

    /// The bytes don't say what they hold; only the type reading them does.
    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Error> {
        Err(Error::Unsupported)
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_bool(self.tag()?)
    }

    fixed!(
        deserialize_i8 => visit_i8(i8),
        deserialize_i16 => visit_i16(i16),
        deserialize_i32 => visit_i32(i32),
        deserialize_i64 => visit_i64(i64),
        deserialize_i128 => visit_i128(i128),
        deserialize_u8 => visit_u8(u8),
        deserialize_u16 => visit_u16(u16),
        deserialize_u32 => visit_u32(u32),
        deserialize_u64 => visit_u64(u64),
        deserialize_u128 => visit_u128(u128),
        deserialize_f32 => visit_f32(f32),
        deserialize_f64 => visit_f64(f64),
    );

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let mut chars = self.str()?.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => visitor.visit_char(c),
            _ => Err(Error::Custom),
        }
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_borrowed_str(self.str()?)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_borrowed_bytes(self.bytes()?)
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        if self.tag()? {
            visitor.visit_some(self)
        } else {
            visitor.visit_none()
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let len = self.len()?;
        visitor.visit_seq(Parts {
            de: self,
            left: len,
        })
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_seq(Parts {
            de: self,
            left: len,
        })
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let len = self.len()?;
        visitor.visit_map(Parts {
            de: self,
            left: len,
        })
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_tuple(fields.len(), visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_enum(self)
    }

    /// Names never cross the wire: a variant is read by index (see
    /// [`de::EnumAccess`] below), and a struct as a sequence.
    fn deserialize_identifier<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Error> {
        Err(Error::Unsupported)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Error> {
        Err(Error::Unsupported)
    }
}

/// The parts of a sequence, tuple, struct or map: `left` more to read.
struct Parts<'a, 'de> {
    de: &'a mut Deserializer<'de>,
    left: usize,
}

impl<'de> de::SeqAccess<'de> for Parts<'_, 'de> {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Error> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        seed.deserialize(&mut *self.de).map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.left)
    }
}

impl<'de> de::MapAccess<'de> for Parts<'_, 'de> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        seed.deserialize(&mut *self.de).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        seed.deserialize(&mut *self.de)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.left)
    }
}

impl<'de> de::EnumAccess<'de> for &mut Deserializer<'de> {
    type Error = Error;
    type Variant = Self;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self), Error> {
        let index = u32::try_from(self.varint()?).map_err(|_| Error::BadVarint)?;
        let value = seed.deserialize(index.into_deserializer())?;
        Ok((value, self))
    }
}

impl<'de> de::VariantAccess<'de> for &mut Deserializer<'de> {
    type Error = Error;

    fn unit_variant(self) -> Result<(), Error> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Error> {
        seed.deserialize(self)
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, Error> {
        de::Deserializer::deserialize_tuple(self, len, visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        de::Deserializer::deserialize_tuple(self, fields.len(), visitor)
    }
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
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
                    fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u64>, A::Error> {
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
}
