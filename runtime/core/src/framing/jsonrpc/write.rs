use serde::Serialize;

use crate::contract::{BufMut, RuntimeError, WireFormat};

/// Writes `{"jsonrpc":"2.0","method":<name>`, up to the name's closing quote,
/// which comes with what follows it, with the name a JSON string escaped as serde_json escapes one: `"` and `\` by backslash,
/// control characters as `\b`, `\f`, `\n`, `\r`, `\t` or `\u00XX`, and
/// nothing else. A generated name needs no escape, and goes out in one slice.
fn put_method(name: &str, out: &mut dyn BufMut) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.put_slice(br#"{"jsonrpc":"2.0","method":""#);
    let bytes = name.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let control;
        let escaped: &[u8] = match b {
            b'"' => br#"\""#,
            b'\\' => br"\\",
            b'\n' => br"\n",
            b'\r' => br"\r",
            b'\t' => br"\t",
            0x08 => br"\b",
            0x0c => br"\f",
            0x00..=0x1f => {
                control = [
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[usize::from(b >> 4)],
                    HEX[usize::from(b & 0xf)],
                ];
                &control
            }
            _ => continue,
        };
        out.put_slice(&bytes[start..i]);
        out.put_slice(escaped);
        start = i + 1;
    }
    out.put_slice(&bytes[start..]);
}

/// Writes `n` in decimal, as `u64`'s `Display` writes it, from a buffer on
/// the stack: 20 digits hold `u64::MAX`.
pub(super) fn put_u64(n: u64, out: &mut dyn BufMut) {
    let mut digits = [0u8; 20];
    let mut start = digits.len();
    let mut n = n;
    loop {
        start -= 1;
        digits[start] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.put_slice(&digits[start..]);
}

const METHOD_END: &[u8] = br#"""#;
const METHOD_THEN_PARAMS: &[u8] = br#"","params":"#;

/// Writes a request from its encoded params, which say nothing when empty or
/// `null`: JSON-RPC 2.0 has `params` omitted then, not `null`.
#[inline(always)]
pub(super) fn put_request_body(
    name: &str,
    request_id: Option<u64>,
    params: &[u8],
    out: &mut dyn BufMut,
) {
    put_method(name, out);
    // By length first: params that say something are almost all longer.
    if params.len() > 4 || !matches!(params, b"" | b"null") {
        out.put_slice(METHOD_THEN_PARAMS);
        out.put_slice(params);
    } else {
        out.put_slice(METHOD_END);
    }
    put_request_end(request_id, out);
}

/// Writes a request with `fmt` encoding its params, to the same frame
/// [`put_request_body`] writes for those bytes.
pub(super) fn put_request<W, P>(
    name: &str,
    request_id: Option<u64>,
    params: &P,
    fmt: &W,
    out: &mut dyn BufMut,
) -> Result<(), RuntimeError>
where
    W: WireFormat,
    P: Serialize + ?Sized,
{
    put_method(name, out);
    if params.serialize(NoParams).is_err() {
        out.put_slice(METHOD_THEN_PARAMS);
        fmt.encode(params, out)?;
    } else {
        out.put_slice(METHOD_END);
        let mut held = HeldNull {
            out,
            matched: 0,
            passing: false,
        };
        fmt.encode(params, &mut held)?;
        held.finish();
    }
    put_request_end(request_id, out);
    Ok(())
}

/// The `id`, which a notification (`None`) goes without, and the closing brace.
/// Inlined: this is on every request, and a call of its own shows on the gate.
#[inline(always)]
fn put_request_end(request_id: Option<u64>, out: &mut dyn BufMut) {
    if let Some(id) = request_id {
        out.put_slice(br#","id":"#);
        put_u64(id, out);
    }
    out.put_slice(b"}");
}

/// Where a format writes a value that serialises as none. The format decides
/// the bytes, not the value (a format can ignore it and write its own), so
/// `,"params":` waits until what it writes turns out not to be `null`. Only a
/// call without arguments comes through here.
struct HeldNull<'o> {
    out: &'o mut dyn BufMut,
    /// How much of `null` has been written and held back.
    matched: usize,
    passing: bool,
}

impl HeldNull<'_> {
    fn pass(&mut self) {
        self.passing = true;
        self.out.put_slice(br#","params":"#);
        self.out.put_slice(&b"null"[..self.matched]);
    }

    /// What was held back and isn't all of `null` wasn't none after all.
    fn finish(&mut self) {
        if !self.passing && self.matched != 0 && self.matched != b"null".len() {
            self.pass();
        }
    }
}

impl BufMut for HeldNull<'_> {
    fn put_slice(&mut self, bytes: &[u8]) {
        if !self.passing {
            let end = self.matched + bytes.len();
            if end <= b"null".len() && b"null"[self.matched..end] == *bytes {
                self.matched = end;
                return;
            }
            self.pass();
        }
        self.out.put_slice(bytes);
    }

    fn overflowed(&self) -> bool {
        self.out.overflowed()
    }
}

/// A value that serialises as `null` in JSON, which is what a call without
/// arguments passes (`&()`). Asked before anything is written, as a `BufMut`
/// can't take back `"params":`. Anything else stops at its first serializer
/// call and goes straight to the format, so a call with arguments pays for
/// no more than this.
struct NoParams;

#[derive(Debug)]
struct SaysParams;

impl core::fmt::Display for SaysParams {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.write_str("params")
    }
}

impl serde::ser::StdError for SaysParams {}

impl serde::ser::Error for SaysParams {
    fn custom<T: core::fmt::Display>(_: T) -> Self {
        SaysParams
    }
}

macro_rules! says_params {
    ($($method:ident($($ty:ty),*);)+) => {$(
        fn $method(self, $(_: $ty),*) -> Result<(), SaysParams> {
            Err(SaysParams)
        }
    )+};
}

impl serde::Serializer for NoParams {
    type Ok = ();
    type Error = SaysParams;
    type SerializeSeq = serde::ser::Impossible<(), SaysParams>;
    type SerializeTuple = serde::ser::Impossible<(), SaysParams>;
    type SerializeTupleStruct = serde::ser::Impossible<(), SaysParams>;
    type SerializeTupleVariant = serde::ser::Impossible<(), SaysParams>;
    type SerializeMap = serde::ser::Impossible<(), SaysParams>;
    type SerializeStruct = serde::ser::Impossible<(), SaysParams>;
    type SerializeStructVariant = serde::ser::Impossible<(), SaysParams>;

    fn serialize_unit(self) -> Result<(), SaysParams> {
        Ok(())
    }
    fn serialize_none(self) -> Result<(), SaysParams> {
        Ok(())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), SaysParams> {
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), SaysParams> {
        value.serialize(self)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), SaysParams> {
        value.serialize(self)
    }

    says_params! {
        serialize_bool(bool);
        serialize_i8(i8);
        serialize_i16(i16);
        serialize_i32(i32);
        serialize_i64(i64);
        serialize_u8(u8);
        serialize_u16(u16);
        serialize_u32(u32);
        serialize_u64(u64);
        serialize_f32(f32);
        serialize_f64(f64);
        serialize_char(char);
        serialize_str(&str);
        serialize_bytes(&[u8]);
        serialize_unit_variant(&'static str, u32, &'static str);
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<(), SaysParams> {
        Err(SaysParams)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, SaysParams> {
        Err(SaysParams)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, SaysParams> {
        Err(SaysParams)
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, SaysParams> {
        Err(SaysParams)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, SaysParams> {
        Err(SaysParams)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, SaysParams> {
        Err(SaysParams)
    }
    fn serialize_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStruct, SaysParams> {
        Err(SaysParams)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, SaysParams> {
        Err(SaysParams)
    }
}
