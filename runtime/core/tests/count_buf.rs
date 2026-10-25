//! [`CountBuf`] counts exactly what an encode writes: every framing's
//! requests and replies, the envelope and the handshake, encoded once into a
//! `Vec` and once into a `CountBuf`, give the same length.
#![cfg(feature = "alloc")]

use comline_runtime::contract::{
    BufMut, Call, ConnectionFraming, CountBuf, Datagram2Framing, DatagramFraming, Envelope,
    Framing, HandshakeFrame, WireFormat, FORMAT_PACKED, FRAMING_DATAGRAM, FRAMING_DATAGRAM2,
};
use comline_runtime::format::Packed;
use serde::Serialize;

/// Run `encode` into a `Vec` and into a `CountBuf`, and compare the lengths.
fn counts_as_written(what: &str, encode: impl Fn(&mut dyn BufMut)) {
    let mut written = Vec::new();
    encode(&mut written);
    let mut count = CountBuf::new();
    encode(&mut count);
    assert_eq!(count.len(), written.len(), "{what}");
}

#[derive(Serialize)]
struct Params<'a> {
    id: u32,
    name: &'a str,
    tags: &'a [u16],
    note: Option<&'a str>,
}

const NAMES: [&str; 4] = ["", "a", "greet", "chat.Chat/send"];
const BODIES: [&[u8]; 4] = [b"", b"\x00", b"null", b"{\"ok\":[1,2,3],\"s\":\"x\\\"y\"}"];
const REQUEST_IDS: [u64; 5] = [0, 1, 255, 1 << 32, u64::MAX];

fn envelopes(body: &[u8]) -> [Envelope<'_>; 4] {
    [
        Envelope::Ok(body),
        Envelope::Err { id: 0, body },
        Envelope::Err { id: u16::MAX, body },
        Envelope::Fault,
    ]
}

fn framing_cases<F: Framing, W: WireFormat>(framing: &F, fmt: &W) {
    let tags = [1u16, 2, 3, 65535];
    for name in NAMES {
        for rid in REQUEST_IDS {
            let params = Params {
                id: rid as u32,
                name,
                tags: &tags,
                note: Some(name).filter(|n| !n.is_empty()),
            };
            counts_as_written(&format!("{} request {name} {rid}", framing.name()), |out| {
                framing
                    .encode_request(Call::new(7, name), rid, &params, fmt, out)
                    .unwrap()
            });
        }
    }
    for rid in REQUEST_IDS {
        for body in BODIES {
            let what = format!("{} response {rid} {body:?}", framing.name());
            counts_as_written(&what, |out| framing.encode_response_ok(rid, body, out));
            counts_as_written(&what, |out| framing.encode_response_err(rid, 9, body, out));
        }
        counts_as_written(&format!("{} fault {rid}", framing.name()), |out| {
            framing.encode_response_fault(rid, out)
        });
    }
}

fn connection_framing_cases<F: ConnectionFraming, W: WireFormat>(framing: &F, fmt: &W) {
    for name in NAMES {
        for rid in REQUEST_IDS {
            for one_way in [false, true] {
                for protocol in [0u16, 3, u16::MAX] {
                    let what = format!("{} request {name} {rid} {one_way}", framing.name());
                    counts_as_written(&what, |out| {
                        framing
                            .encode_request(
                                protocol,
                                Call::new(7, name),
                                rid,
                                one_way,
                                &(rid, name),
                                fmt,
                                out,
                            )
                            .unwrap()
                    });
                }
            }
        }
    }
    for rid in REQUEST_IDS {
        for body in BODIES {
            for reply in envelopes(body) {
                let what = format!("{} reply {rid} {reply:?}", framing.name());
                counts_as_written(&what, |out| framing.encode_reply(2, rid, &reply, out));
            }
        }
    }
}

#[test]
fn datagram() {
    framing_cases(&DatagramFraming, &Packed);
}

#[test]
fn datagram2() {
    connection_framing_cases(&Datagram2Framing, &Packed);
}

#[cfg(feature = "std")]
#[test]
fn jsonrpc() {
    use comline_runtime::format::{Json, MsgPack};
    use comline_runtime::framing::JsonRpcFraming;

    framing_cases(&JsonRpcFraming, &Json);
    connection_framing_cases(&JsonRpcFraming, &Json);
    // The datagram framings with the other formats too.
    framing_cases(&DatagramFraming, &Json);
    framing_cases(&DatagramFraming, &MsgPack);
    connection_framing_cases(&Datagram2Framing, &MsgPack);
}

#[test]
fn envelope() {
    for body in BODIES {
        for envelope in envelopes(body) {
            counts_as_written(&format!("{envelope:?}"), |out| envelope.encode(out));
        }
    }
}

#[test]
fn handshake() {
    for (ir_hash, format, framing, caps) in [
        (0, FORMAT_PACKED, FRAMING_DATAGRAM2, 0),
        (u64::MAX, "json@1", FRAMING_DATAGRAM, u32::MAX),
        (0x0102_0304_0506_0708, "com.acme.custom@1", "jsonrpc@1", 5),
    ] {
        let frame = HandshakeFrame::new(ir_hash, format, framing, caps);
        counts_as_written(&format!("{frame:?}"), |out| frame.encode(out));
    }
}
