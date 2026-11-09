use serde::Deserialize;
use serde_json::value::RawValue;

use crate::contract::{Envelope, MethodName};

#[derive(Deserialize)]
pub(super) struct ErrIn<'a> {
    code: i64,
    /// Required, and a string: an error object without one isn't JSON-RPC
    /// 2.0. Its text is the sender's, not the schema's, so it isn't kept.
    #[serde(rename = "message")]
    _message: Text,
    #[serde(borrow, default)]
    data: Option<&'a RawValue>,
}

/// `"jsonrpc": "2.0"`, which every JSON-RPC 2.0 message carries: a frame
/// without it, or naming another version, is refused (runtime#75).
pub(super) struct Version;

/// Any JSON string, read without keeping it.
struct Text;

/// Reads a string and hands it to `check`, without allocating: decoding a
/// frame allocates nothing.
struct StrVisitor(fn(&str) -> bool);

impl serde::de::Visitor<'_> for StrVisitor {
    type Value = ();

    fn expecting(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.write_str("a string")
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<(), E> {
        if (self.0)(v) {
            Ok(())
        } else {
            Err(E::custom("unexpected string"))
        }
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_str(StrVisitor(|v| v == "2.0"))
            .map(|()| Version)
    }
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_str(StrVisitor(|_| true)).map(|()| Text)
    }
}

/// A request's `id`, which a notification goes without. `"id": null` is
/// refused rather than read as a notification: the specification still
/// wants a reply to it, and a notification never gets one (runtime#75).
pub(super) fn request_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    u64::deserialize(d).map(Some)
}

/// A request's params, which JSON-RPC 2.0 has be an array or an object
/// when given at all. `null` isn't that, and this framing leaves `params` out
/// for none, but it's read as none: it wrote `null` before spec 0.8, and other
/// peers do.
pub(super) fn params_of<'f>(params: Option<&'f RawValue>) -> Option<&'f [u8]> {
    match params.map(RawValue::get) {
        None => Some(b"null"),
        Some(text) if text.starts_with(['[', '{']) || text == "null" => Some(text.as_bytes()),
        Some(_) => None,
    }
}

/// A JSON-RPC 2.0 error whose `id` is `null`, which is how the specification
/// has a peer answer a request it couldn't read. No other frame has a `null` id:
/// a request's is refused, and so is a `result`'s.
pub(super) fn is_peer_refusal(frame: &[u8]) -> bool {
    #[derive(Deserialize)]
    struct RefusalIn<'a> {
        #[serde(rename = "jsonrpc")]
        _version: Version,
        #[serde(borrow, default)]
        result: Option<&'a RawValue>,
        #[serde(borrow, rename = "error")]
        _error: ErrIn<'a>,
        #[serde(rename = "id")]
        _id: (),
    }
    parse::<RefusalIn>(frame).is_some_and(|r| r.result.is_none())
}

/// A member that's there, whatever it holds: `Option<&RawValue>` alone reads
/// `null` as no member, and a response's `"result":null` is its result.
pub(super) fn present<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<&'de RawValue>, D::Error> {
    <&RawValue>::deserialize(d).map(Some)
}

/// A response has its `result` or its `error`: neither is no response, which is
/// what a request reads as, and a `result` beside an `error` must be `null`.
pub(super) fn one_outcome(result: Option<&RawValue>, error: Option<&ErrIn<'_>>) -> Option<()> {
    match (result, error) {
        (None, None) => None,
        (Some(result), Some(_)) => (result.get() == "null").then_some(()),
        _ => Some(()),
    }
}

/// A response's envelope, from its `result` or `error`.
pub(super) fn envelope_of<'f>(
    result: Option<&'f RawValue>,
    error: Option<ErrIn<'f>>,
) -> Envelope<'f> {
    match error {
        Some(e) => match u16::try_from(e.code) {
            Ok(id) => Envelope::Err {
                id,
                body: e.data.map(|d| d.get().as_bytes()).unwrap_or(b"null"),
            },
            // An ordinal is a `u16`, so it is always in `0..=65535`. Anything
            // outside that — JSON-RPC's reserved negatives, `-32603` among
            // them, or a code too wide to be an ordinal — is not a schema
            // error this wire can name, so it is a fault.
            Err(_) => Envelope::Fault,
        },
        // A `null` result is given as the same static bytes as absent data.
        None => Envelope::Ok(match result {
            Some(result) if result.get() != "null" => result.get().as_bytes(),
            _ => b"null",
        }),
    }
}

/// Reads a frame as `T`. JSON text is UTF-8 (RFC 8259 §8.1), all of it:
/// serde_json checks only the strings it reads, and skips a member `T`
/// ignores (`jsonrpc`, `error.message`, an unknown one) without looking, so
/// the whole frame is checked first. Read from a `str`, serde_json then
/// doesn't check its strings again.
pub(super) fn parse<'f, T: Deserialize<'f>>(frame: &'f [u8]) -> Option<T> {
    serde_json::from_str(core::str::from_utf8(frame).ok()?).ok()
}

/// The method a request names, from its `method` member, which must be a
/// string. It's read as written, so a name in any JSON spelling is borrowed
/// from the frame rather than copied out to unescape it.
pub(super) fn method_of(method: &RawValue) -> Option<MethodName<'_>> {
    let text = method.get().strip_prefix('"')?.strip_suffix('"')?;
    MethodName::from_json(text)
}
