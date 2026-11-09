//! `packed@1`: a value is its fields back to back, like a packed C struct.
//!
//! - Integers and floats are fixed-width little-endian; `bool` is one byte,
//!   `00` or `01`.
//! - Strings and byte strings are a varint length, then the bytes. A single
//!   `char` is its UTF-8 bytes alone, 1 to 4: the first byte gives the width.
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

use crate::bounds::{Measure, MessageHead};
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
            .map_err(Error::into_runtime)
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
        let value = seed.deserialize(&mut de).map_err(Error::into_runtime)?;
        if !de.input.is_empty() {
            return Err(RuntimeError::Serialization);
        }
        Ok(value)
    }
}

/// Why encoding or decoding failed. Callers see
/// [`RuntimeError::BoundExceeded`] for a bound and
/// [`RuntimeError::Serialization`] for everything else; the reason is for
/// this module's tests.
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
    /// A value over its type's bound, from [`crate::bounds`]. Only the
    /// measure: with the maximum too, this error would take 8 bytes instead
    /// of 1, and every `Result` on the decode path would grow with it.
    Bound(Measure),
}

impl Error {
    fn into_runtime(self) -> RuntimeError {
        match self {
            Error::Bound(measure) => RuntimeError::BoundExceeded { measure },
            _ => RuntimeError::Serialization,
        }
    }

    fn custom_of(message: &dyn fmt::Display) -> Self {
        MessageHead::of(message).map_or(Error::Custom, |b| Error::Bound(b.measure))
    }
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
            Error::Bound(Measure::Chars) => "more characters than the type allows",
            Error::Bound(Measure::Items) => "more items than the type allows",
            Error::Bound(Measure::Depth) => "nested deeper than the type allows",
        })
    }
}

impl de::StdError for Error {}

impl ser::Error for Error {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Error::custom_of(&message)
    }
}

impl de::Error for Error {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Error::custom_of(&message)
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

    /// A single `char` is its UTF-8 bytes, 1 to 4, with no length: the
    /// first byte says how many follow.
    fn serialize_char(self, v: char) -> Result<(), Error> {
        self.out.put_slice(v.encode_utf8(&mut [0; 4]).as_bytes());
        Ok(())
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

    /// One character's UTF-8 bytes, checked strictly: no overlong form,
    /// surrogate or truncated character.
    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        let (c, width) = match crate::bounds::char_from_utf8(self.input) {
            Ok(read) => read,
            Err(_) if self.input.is_empty() => return Err(Error::Truncated),
            Err(_) => return Err(Error::BadUtf8),
        };
        self.take(width)?;
        visitor.visit_char(c)
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
mod tests;
