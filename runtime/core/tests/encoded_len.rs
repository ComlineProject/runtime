//! [`WireFormat::encoded_len`] is the length of what `encode` writes, in every
//! format, and fails as `encode` does.
#![cfg(feature = "std")]

use comline_runtime::bounds::encode::string;
use comline_runtime::contract::{RuntimeError, WireFormat};
use comline_runtime::format::{Json, MsgPack, Packed};
use serde::{Serialize, Serializer};

#[derive(Serialize)]
struct Msg<'a> {
    id: u32,
    name: &'a str,
    tags: Vec<u16>,
    note: Option<&'a str>,
    ratio: f64,
}

/// A string over its bound, which `encode` refuses before writing it.
struct Short<'a>(&'a str);

impl Serialize for Short<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        string::<S, 3>(self.0, s)
    }
}

fn agrees<W: WireFormat>(format: W) {
    let messages = [
        Msg {
            id: 0,
            name: "",
            tags: vec![],
            note: None,
            ratio: 0.0,
        },
        Msg {
            id: 300,
            name: "hello",
            tags: vec![1, 2, 65535],
            note: Some("x"),
            ratio: 0.1,
        },
        Msg {
            id: u32::MAX,
            name: "e\u{301}\u{1f600}\"\\\n",
            tags: vec![0; 200],
            note: None,
            ratio: f64::MAX,
        },
    ];
    for message in &messages {
        let mut written = Vec::new();
        format.encode(message, &mut written).unwrap();
        assert_eq!(
            format.encoded_len(message),
            Ok(written.len()),
            "{}",
            format.name()
        );
    }
    assert_eq!(
        format.encoded_len(&Short("abc")),
        Ok(format.encoded_len("abc").unwrap())
    );
    let mut out = Vec::new();
    let refused = format.encode(&Short("abcd"), &mut out).unwrap_err();
    assert!(
        matches!(refused, RuntimeError::BoundExceeded { .. }),
        "{refused:?}"
    );
    assert_eq!(format.encoded_len(&Short("abcd")), Err(refused));
}

#[test]
fn packed() {
    agrees(Packed);
}

#[test]
fn msgpack() {
    agrees(MsgPack);
}

#[test]
fn json() {
    agrees(Json);
}
