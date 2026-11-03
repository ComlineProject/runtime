// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounds on types (core#98): the checks generated code calls while decoding
//! and encoding, and the single `char`.
//!
//! A bound is a maximum: a string (`char[max N]`) holds at most N
//! characters, a dynamic array (`T[max N]`) at most N items, and a type that
//! can contain itself (`@max_depth = D`) nests at most D levels deep. It's
//! part of the type, so a value over it isn't a value of that type: a decoder
//! refuses it like malformed input, and an encoder refuses to write it.
//!
//! - **Strings count characters** (Unicode scalar values), never bytes, and
//!   are never normalized. A decoder refuses a byte length above 4 × N before
//!   allocating, since N characters take at most 4 × N bytes of UTF-8, then
//!   counts exactly ([`chars_from_utf8`], [`check_chars`]).
//! - **Arrays** are refused at a count prefix above N, before allocating,
//!   where the format has one (`packed@1`, `msgpack@1`), and at item N + 1
//!   where it doesn't (`json@1`) ([`check_items`], [`ItemsSeed`]).
//! - **Depth** counts the values of the recursive type on the way down: the
//!   outermost is level 1, and a value at level D + 1 is refused ([`Depth`]).
//!
//! Everything here is `no_std` and allocation-free except the `serde`
//! adapters that build a `String` or a `Vec` (`decode`, with `alloc`).
//!
//! A bound failure surfaces as [`RuntimeError::BoundExceeded`] from every
//! format's `encode` and `decode`: the adapters raise it through `serde`'s
//! error as [`BoundExceeded`]'s message, which each format recognises
//! ([`BoundExceeded::from_message`]).

use core::fmt;

use serde::de::{self, DeserializeSeed, IgnoredAny, SeqAccess, Visitor};
use serde::ser::{self, SerializeSeq};
use serde::{Deserializer, Serialize, Serializer};

use crate::contract::RuntimeError;

/// What a bound limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Measure {
    /// A string's characters (`char[max N]`).
    Chars,
    /// A dynamic array's items (`T[max N]`).
    Items,
    /// How deep a type that contains itself nests (`@max_depth`).
    Depth,
}

impl Measure {
    fn words(self) -> &'static str {
        match self {
            Measure::Chars => "characters",
            Measure::Items => "items",
            Measure::Depth => "levels deep",
        }
    }
}

/// A value over its type's bound: more than `max` of `measure`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BoundExceeded {
    pub measure: Measure,
    pub max: u64,
}

/// The prefix of [`BoundExceeded`]'s message, by which a format tells a bound
/// failure from other malformed input.
const MESSAGE_PREFIX: &str = "more than ";

impl BoundExceeded {
    pub const fn new(measure: Measure, max: u64) -> Self {
        Self { measure, max }
    }

    /// Reads back a [`BoundExceeded`] from its message, as a format's
    /// `serde` error carries it (with anything after it, such as
    /// `serde_json`'s " at line 1 column 9").
    pub fn from_message(message: &str) -> Option<Self> {
        // A format may put its own words first (`rmp-serde`'s "syntax
        // error: ").
        let at = message.find(MESSAGE_PREFIX)?;
        let rest = &message[at + MESSAGE_PREFIX.len()..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let max = rest[..digits].parse().ok()?;
        let rest = rest[digits..].strip_prefix(' ')?;
        [Measure::Chars, Measure::Items, Measure::Depth]
            .into_iter()
            .find(|m| rest.starts_with(m.words()))
            .map(|measure| Self { measure, max })
    }
}

impl fmt::Display for BoundExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{MESSAGE_PREFIX}{} {}", self.max, self.measure.words())
    }
}

impl core::error::Error for BoundExceeded {}

impl From<BoundExceeded> for RuntimeError {
    fn from(b: BoundExceeded) -> Self {
        RuntimeError::BoundExceeded {
            measure: b.measure,
            max: b.max,
        }
    }
}

/// The [`RuntimeError`] for a format's `serde` error: a bound's, or
/// [`RuntimeError::Serialization`] for anything else. Reads only the head of
/// the message, with no allocation.
#[allow(dead_code)] // `no_std` builds have no `serde` formats with their own errors.
pub(crate) fn error_of(error: &dyn fmt::Display) -> RuntimeError {
    MessageHead::of(error).map_or(RuntimeError::Serialization, Into::into)
}

/// A `fmt::Write` keeping only the first bytes written: enough of a `serde`
/// error's message to recognise a bound, with no allocation.
pub(crate) struct MessageHead {
    buf: [u8; 128],
    len: usize,
}

impl Default for MessageHead {
    fn default() -> Self {
        Self {
            buf: [0; 128],
            len: 0,
        }
    }
}

impl MessageHead {
    pub(crate) fn of(message: &dyn fmt::Display) -> Option<BoundExceeded> {
        let mut head = MessageHead::default();
        let _ = fmt::write(&mut head, format_args!("{message}"));
        BoundExceeded::from_message(core::str::from_utf8(&head.buf[..head.len]).ok()?)
    }
}

impl fmt::Write for MessageHead {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let room = self.buf.len() - self.len;
        // Cut at a character boundary, so the head stays UTF-8.
        let mut take = s.len().min(room);
        while !s.is_char_boundary(take) {
            take -= 1;
        }
        self.buf[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

// ── the checks ──────────────────────────────────────────────────────────────

/// Whether `count` items fit a maximum of `max`.
#[inline]
pub fn check_items(count: usize, max: u64) -> Result<(), BoundExceeded> {
    if count as u64 > max {
        Err(BoundExceeded::new(Measure::Items, max))
    } else {
        Ok(())
    }
}

/// Whether `s` holds at most `max` characters. A string of at most `max`
/// bytes needs no count.
#[inline]
pub fn check_chars(s: &str, max: u64) -> Result<(), BoundExceeded> {
    let over = || BoundExceeded::new(Measure::Chars, max);
    if s.len() as u64 <= max {
        return Ok(());
    }
    if s.len() as u64 > max.saturating_mul(4) {
        return Err(over());
    }
    // Every byte that isn't a UTF-8 continuation byte starts a character.
    let chars = s.bytes().filter(|&b| (b as i8) >= -0x40).count();
    if chars as u64 > max {
        Err(over())
    } else {
        Ok(())
    }
}

/// The strict UTF-8 check with a maximum: `bytes` as a string of at most
/// `max` characters. A byte length above 4 × `max` is refused before reading
/// any of it; invalid UTF-8 is [`RuntimeError::Serialization`], as every
/// format reports it.
pub fn chars_from_utf8(bytes: &[u8], max: u64) -> Result<&str, RuntimeError> {
    if bytes.len() as u64 > max.saturating_mul(4) {
        return Err(BoundExceeded::new(Measure::Chars, max).into());
    }
    let s = core::str::from_utf8(bytes).map_err(|_| RuntimeError::Serialization)?;
    check_chars(s, max)?;
    Ok(s)
}

/// How many bytes the UTF-8 character starting with `first` takes, or
/// `None` if no character starts with it (a continuation byte, or one of
/// `c0`, `c1`, `f5`..`ff`).
#[inline]
pub fn utf8_width(first: u8) -> Option<usize> {
    match first {
        0x00..=0x7f => Some(1),
        0xc2..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf4 => Some(4),
        _ => None,
    }
}

/// A single `char` as `packed@1` sends it: its UTF-8 bytes, no prefix. Reads
/// the one character at the start of `bytes`, and how many bytes it took.
/// Strict: an overlong form, a surrogate or a truncated character is refused.
pub fn char_from_utf8(bytes: &[u8]) -> Result<(char, usize), RuntimeError> {
    let first = *bytes.first().ok_or(RuntimeError::Serialization)?;
    let width = utf8_width(first).ok_or(RuntimeError::Serialization)?;
    let encoded = bytes.get(..width).ok_or(RuntimeError::Serialization)?;
    let s = core::str::from_utf8(encoded).map_err(|_| RuntimeError::Serialization)?;
    let c = s.chars().next().ok_or(RuntimeError::Serialization)?;
    Ok((c, width))
}

/// The nesting of a type that can contain itself, against its `@max_depth`.
///
/// Call [`enter`](Self::enter) before reading or writing each value of the
/// recursive type and [`leave`](Self::leave) after it: the outermost value is
/// level 1, and entering level `max + 1` is refused. A decoder refuses there,
/// before reading any deeper, so a hostile input can't recurse past it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Depth {
    level: u64,
    max: u64,
}

impl Depth {
    pub const fn new(max: u64) -> Self {
        Self { level: 0, max }
    }

    /// The level the next [`enter`](Self::enter) would reach, less one.
    pub const fn level(&self) -> u64 {
        self.level
    }

    pub const fn max(&self) -> u64 {
        self.max
    }

    #[inline]
    pub fn enter(&mut self) -> Result<(), BoundExceeded> {
        if self.level >= self.max {
            return Err(BoundExceeded::new(Measure::Depth, self.max));
        }
        self.level += 1;
        Ok(())
    }

    #[inline]
    pub fn leave(&mut self) {
        self.level = self.level.saturating_sub(1);
    }
}

// ── serde ───────────────────────────────────────────────────────────────────

/// The `serde` error for a bound: a format turns it back into
/// [`RuntimeError::BoundExceeded`].
pub fn de_error<E: de::Error>(b: BoundExceeded) -> E {
    E::custom(b)
}

/// The `serde` error for a bound, on encode.
pub fn ser_error<E: ser::Error>(b: BoundExceeded) -> E {
    E::custom(b)
}

/// Decodes a string of at most `max` characters, as `T`: `&'de str`, or
/// `String` with `alloc`. Checked before the string is allocated: a borrowed
/// string is measured where it lies, and a byte length above 4 × `max`
/// isn't counted at all.
#[derive(Debug, Clone, Copy)]
pub struct CharsSeed<T> {
    pub max: u64,
    marker: core::marker::PhantomData<fn() -> T>,
}

impl<T> CharsSeed<T> {
    pub const fn new(max: u64) -> Self {
        Self {
            max,
            marker: core::marker::PhantomData,
        }
    }
}

/// What a [`CharsSeed`] can build: a borrowed or an owned string.
pub trait FromBoundedStr<'de>: Sized {
    fn from_borrowed<E: de::Error>(s: &'de str) -> Result<Self, E>;
    fn from_transient<E: de::Error>(s: &str) -> Result<Self, E>;
}

impl<'de> FromBoundedStr<'de> for &'de str {
    fn from_borrowed<E: de::Error>(s: &'de str) -> Result<Self, E> {
        Ok(s)
    }

    fn from_transient<E: de::Error>(_: &str) -> Result<Self, E> {
        Err(E::custom("a string that can't be borrowed"))
    }
}

#[cfg(feature = "alloc")]
impl<'de> FromBoundedStr<'de> for alloc::string::String {
    fn from_borrowed<E: de::Error>(s: &'de str) -> Result<Self, E> {
        Ok(s.into())
    }

    fn from_transient<E: de::Error>(s: &str) -> Result<Self, E> {
        Ok(s.into())
    }
}

impl<'de, T: FromBoundedStr<'de>> DeserializeSeed<'de> for CharsSeed<T> {
    type Value = T;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<T, D::Error> {
        d.deserialize_str(self)
    }
}

impl<'de, T: FromBoundedStr<'de>> Visitor<'de> for CharsSeed<T> {
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "a string of at most {} characters", self.max)
    }

    fn visit_borrowed_str<E: de::Error>(self, s: &'de str) -> Result<T, E> {
        check_chars(s, self.max).map_err(de_error)?;
        T::from_borrowed(s)
    }

    fn visit_str<E: de::Error>(self, s: &str) -> Result<T, E> {
        check_chars(s, self.max).map_err(de_error)?;
        T::from_transient(s)
    }
}

/// Decodes a dynamic array of at most `max` items, each through `of`, into
/// any collection that extends from them (a `Vec` with `alloc`). A count
/// prefix above `max` is refused before anything is allocated; a format
/// with none (`json@1`) is refused at item `max + 1`, which is skipped, not
/// built.
#[derive(Debug, Clone, Copy)]
pub struct ItemsSeed<S, C> {
    pub of: S,
    pub max: u64,
    marker: core::marker::PhantomData<fn() -> C>,
}

impl<S, C> ItemsSeed<S, C> {
    pub const fn new(of: S, max: u64) -> Self {
        Self {
            of,
            max,
            marker: core::marker::PhantomData,
        }
    }
}

/// A collection an [`ItemsSeed`] fills, given room for `capacity` items.
pub trait BoundedCollection<T>: Sized {
    fn with_capacity(capacity: usize) -> Self;
    fn push(&mut self, item: T);
}

#[cfg(feature = "alloc")]
impl<T> BoundedCollection<T> for alloc::vec::Vec<T> {
    fn with_capacity(capacity: usize) -> Self {
        alloc::vec::Vec::with_capacity(capacity)
    }

    fn push(&mut self, item: T) {
        alloc::vec::Vec::push(self, item)
    }
}

impl<'de, S, C> DeserializeSeed<'de> for ItemsSeed<S, C>
where
    S: DeserializeSeed<'de> + Clone,
    C: BoundedCollection<S::Value>,
{
    type Value = C;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<C, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de, S, C> Visitor<'de> for ItemsSeed<S, C>
where
    S: DeserializeSeed<'de> + Clone,
    C: BoundedCollection<S::Value>,
{
    type Value = C;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "a sequence of at most {} items", self.max)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<C, A::Error> {
        let over = || de_error(BoundExceeded::new(Measure::Items, self.max));
        let hint = seq.size_hint().unwrap_or(0);
        check_items(hint, self.max).map_err(|_| over())?;
        // The hint is the input's claim: it only sizes the first allocation,
        // as `serde`'s own `Vec` does.
        let mut items = C::with_capacity(hint.min(4096));
        let mut count: u64 = 0;
        loop {
            if count == self.max {
                // One more is one too many: skip it unbuilt to find out.
                return match seq.next_element::<IgnoredAny>()? {
                    Some(_) => Err(over()),
                    None => Ok(items),
                };
            }
            match seq.next_element_seed(self.of.clone())? {
                Some(item) => items.push(item),
                None => return Ok(items),
            }
            count += 1;
        }
    }
}

/// `serde` adapters for a generated field, by `deserialize_with` and
/// `serialize_with`: `#[serde(deserialize_with = "bounds::decode::string::<_, 32>")]`.
#[cfg(feature = "alloc")]
pub mod decode {
    use super::*;
    use serde::Deserialize;

    /// A `String` of at most `MAX` characters.
    pub fn string<'de, D: Deserializer<'de>, const MAX: u64>(
        d: D,
    ) -> Result<alloc::string::String, D::Error> {
        CharsSeed::new(MAX).deserialize(d)
    }

    /// A `Vec<T>` of at most `MAX` items.
    pub fn vec<'de, D: Deserializer<'de>, T: Deserialize<'de>, const MAX: u64>(
        d: D,
    ) -> Result<alloc::vec::Vec<T>, D::Error> {
        ItemsSeed::new(core::marker::PhantomData::<T>, MAX).deserialize(d)
    }
}

/// The encode side of `decode`: an over-bound value is refused before any of
/// it is written.
pub mod encode {
    use super::*;

    /// A string of at most `MAX` characters.
    pub fn string<S: Serializer, const MAX: u64>(v: &str, s: S) -> Result<S::Ok, S::Error> {
        check_chars(v, MAX).map_err(ser_error)?;
        s.serialize_str(v)
    }

    /// A slice of at most `MAX` items.
    pub fn vec<S: Serializer, T: Serialize, const MAX: u64>(
        v: &[T],
        s: S,
    ) -> Result<S::Ok, S::Error> {
        check_items(v.len(), MAX).map_err(ser_error)?;
        let mut seq = s.serialize_seq(Some(v.len()))?;
        for item in v {
            seq.serialize_element(item)?;
        }
        seq.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_reads_back() {
        for measure in [Measure::Chars, Measure::Items, Measure::Depth] {
            let b = BoundExceeded::new(measure, 32);
            let mut head = MessageHead::default();
            fmt::write(&mut head, format_args!("{b} at line 1 column 9")).unwrap();
            assert_eq!(MessageHead::of(&b), Some(b));
            assert_eq!(
                BoundExceeded::from_message(core::str::from_utf8(&head.buf[..head.len]).unwrap()),
                Some(b)
            );
        }
        assert_eq!(BoundExceeded::from_message("more than x items"), None);
        assert_eq!(BoundExceeded::from_message("invalid type"), None);
    }

    #[test]
    fn characters_not_bytes() {
        assert!(check_chars("abcd", 4).is_ok());
        assert!(check_chars("abcde", 4).is_err());
        // 4 characters, 16 bytes: the most 4 characters can take.
        assert!(check_chars("😀😀😀😀", 4).is_ok());
        assert!(check_chars("😀😀😀😀a", 4).is_err());
        assert!(check_chars("ééééé", 4).is_err());
        assert!(check_chars("", 0).is_ok());
        assert!(check_chars("a", 0).is_err());
    }

    #[test]
    fn utf8_is_checked_and_counted() {
        assert_eq!(chars_from_utf8("é€😀".as_bytes(), 3), Ok("é€😀"));
        assert_eq!(
            chars_from_utf8(&[b'a'; 13], 3),
            Err(RuntimeError::BoundExceeded {
                measure: Measure::Chars,
                max: 3
            })
        );
        assert_eq!(
            chars_from_utf8(&[0xff], 3),
            Err(RuntimeError::Serialization)
        );
    }

    #[test]
    fn a_single_char_is_strict_utf8() {
        assert_eq!(char_from_utf8(b"a"), Ok(('a', 1)));
        assert_eq!(char_from_utf8("é".as_bytes()), Ok(('é', 2)));
        assert_eq!(char_from_utf8("€".as_bytes()), Ok(('€', 3)));
        assert_eq!(char_from_utf8("😀x".as_bytes()), Ok(('😀', 4)));
        for bad in [
            &[][..],
            &[0x80],                   // a continuation byte
            &[0xc3],                   // truncated
            &[0xc0, 0x80],             // overlong
            &[0xed, 0xa0, 0x80],       // a surrogate
            &[0xf4, 0x90, 0x80, 0x80], // past U+10FFFF
            &[0xff],
        ] {
            assert_eq!(
                char_from_utf8(bad),
                Err(RuntimeError::Serialization),
                "{bad:x?}"
            );
        }
    }

    #[test]
    fn depth_counts_levels() {
        let mut depth = Depth::new(2);
        assert!(depth.enter().is_ok());
        assert!(depth.enter().is_ok());
        assert_eq!(depth.enter(), Err(BoundExceeded::new(Measure::Depth, 2)));
        depth.leave();
        assert!(depth.enter().is_ok());
    }

    /// Every format reports a bound as `BoundExceeded`, on decode and encode.
    #[cfg(feature = "std")]
    #[test]
    fn every_format_reports_the_bound() {
        use crate::contract::WireFormat;
        use crate::format::{Json, MsgPack, Packed};
        use alloc::string::String;
        use alloc::vec::Vec;
        use serde::Deserialize;

        #[derive(Serialize, Deserialize, Debug, PartialEq)]
        struct Named {
            #[serde(
                deserialize_with = "decode::string::<_, 3>",
                serialize_with = "encode::string::<_, 3>"
            )]
            name: String,
            #[serde(
                deserialize_with = "decode::vec::<_, u8, 2>",
                serialize_with = "encode::vec::<_, u8, 2>"
            )]
            tags: Vec<u8>,
        }
        #[derive(Serialize)]
        struct Unbounded<'a> {
            name: &'a str,
            tags: &'a [u8],
        }

        fn check<F: WireFormat>(f: F) {
            let chars = RuntimeError::BoundExceeded {
                measure: Measure::Chars,
                max: 3,
            };
            let items = RuntimeError::BoundExceeded {
                measure: Measure::Items,
                max: 2,
            };
            let encode = |name: &str, tags: &[u8]| {
                let mut out = Vec::new();
                f.encode(&Unbounded { name, tags }, &mut out).map(|()| out)
            };
            let fits = encode("é😀a", &[1, 2]).unwrap();
            assert_eq!(
                f.decode::<Named>(&fits),
                Ok(Named {
                    name: "é😀a".into(),
                    tags: vec![1, 2]
                })
            );
            assert_eq!(f.decode::<Named>(&encode("abcd", &[]).unwrap()), Err(chars));
            assert_eq!(
                f.decode::<Named>(&encode("", &[1, 2, 3]).unwrap()),
                Err(items)
            );
            let mut out = Vec::new();
            let over = Named {
                name: "ééée".into(),
                tags: vec![],
            };
            assert_eq!(f.encode(&over, &mut out), Err(chars));
            let over = Named {
                name: "".into(),
                tags: vec![1, 2, 3],
            };
            assert_eq!(f.encode(&over, &mut out), Err(items));
        }
        check(Packed);
        check(MsgPack);
        check(Json);
    }

    #[cfg(feature = "std")]
    #[test]
    fn a_single_char_on_each_format() {
        use crate::contract::WireFormat;
        use crate::format::{Json, MsgPack, Packed};
        use alloc::vec::Vec;

        let bytes = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut out = Vec::new();
            f(&mut out);
            out
        };
        for c in ['a', 'é', '€', '😀'] {
            let mut utf8 = [0; 4];
            let utf8 = c.encode_utf8(&mut utf8).as_bytes();
            let packed = bytes(&|o| Packed.encode(&c, o).unwrap());
            assert_eq!(packed, utf8, "packed@1: raw UTF-8, no prefix");
            assert_eq!(Packed.decode::<char>(&packed), Ok(c));
            let msgpack = bytes(&|o| MsgPack.encode(&c, o).unwrap());
            assert_eq!(msgpack[0], 0xa0 | utf8.len() as u8, "msgpack@1: a fixstr");
            assert_eq!(MsgPack.decode::<char>(&msgpack), Ok(c));
            let json = bytes(&|o| Json.encode(&c, o).unwrap());
            assert_eq!(Json.decode::<char>(&json), Ok(c));
        }
        assert!(Packed.decode::<char>(b"ab").is_err());
        assert!(Packed.decode::<char>(&[0xc3]).is_err());
        assert!(MsgPack.decode::<char>(&[0xa2, b'a', b'b']).is_err());
        assert!(Json.decode::<char>(b"\"ab\"").is_err());
        assert!(Json.decode::<char>(b"\"\"").is_err());
    }
}
