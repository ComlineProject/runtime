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
        "json"
    }

    fn encode<T: Serialize + ?Sized>(
        &self,
        value: &T,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError> {
        serde_json::to_writer(BufWriter(out), value).map_err(|_| RuntimeError::Serialization)
    }

    fn decode<'de, T: Deserialize<'de>>(&self, bytes: &'de [u8]) -> Result<T, RuntimeError> {
        serde_json::from_slice(bytes).map_err(|_| RuntimeError::Serialization)
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

    #[test]
    fn garbage_is_a_serialization_error() {
        let err = Json.decode::<u32>(b"not json").unwrap_err();
        assert_eq!(err, RuntimeError::Serialization);
    }
}
