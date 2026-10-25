// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bytes through every frame decoder: `datagram@1` requests and responses,
//! `datagram@2` frames, JSON-RPC requests, responses and connection frames,
//! envelopes and the handshake. None may panic.
//!
//! The binary frames are canonical: whatever decodes encodes back to exactly
//! the bytes. Two decoders read a prefix and ignore what follows, in this
//! runtime and in comline-typescript alike, so for them the re-encoding is
//! that prefix: a `Fault` envelope (its tag alone; an `Ok` payload or `Err`
//! body runs to the end, so those are exact) and the fixed-size handshake.
//! JSON-RPC isn't (whitespace, member order, escapes), so a
//! JSON-RPC response or reply is re-encoded and decoded again instead, and
//! must give the same thing. A JSON-RPC request's method name is
//! `&'static str` on the encoding side, so requests are only decoded.

#![no_main]

use comline_runtime::contract::{
    ConnectionFraming, Datagram2Framing, DatagramFraming, Envelope, Frame, Framing, HandshakeFrame,
    Request, RequestCall,
};
use comline_runtime::framing::JsonRpcFraming;
use libfuzzer_sys::fuzz_target;

/// `datagram@1`'s and `datagram@2`'s request head, written out: the
/// framings encode params through a `WireFormat`, and these are already
/// bytes.
fn request_head(call: RequestCall, request: &Request, out: &mut Vec<u8>) {
    let RequestCall::Id(id) = call else {
        panic!("a datagram request carries a call id")
    };
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&request.request_id.to_le_bytes());
    out.extend_from_slice(request.params);
}

/// LEB128, as `datagram@2` writes a protocol number.
fn varint(mut value: u16, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn response(f: &impl Framing, request_id: u64, envelope: &Envelope) -> Vec<u8> {
    let mut out = Vec::new();
    match *envelope {
        Envelope::Ok(payload) => f.encode_response_ok(request_id, payload, &mut out),
        Envelope::Err { id, body } => f.encode_response_err(request_id, id, body, &mut out),
        Envelope::Fault => f.encode_response_fault(request_id, &mut out),
    }
    out
}

/// `out` is all of `bytes`, or for a decoder that reads a prefix (a fault
/// envelope, the handshake), the start of it.
fn reencodes(out: &[u8], bytes: &[u8], prefix_only: bool, what: &dyn std::fmt::Debug) {
    if prefix_only {
        assert!(bytes.starts_with(out), "{what:?} re-encodes as {out:02x?}");
    } else {
        assert_eq!(out, bytes, "{what:?}");
    }
}

fuzz_target!(|bytes: &[u8]| {
    if let Some(envelope) = Envelope::decode(bytes) {
        let mut out = Vec::new();
        envelope.encode(&mut out);
        reencodes(&out, bytes, envelope == Envelope::Fault, &envelope);
    }

    if let Some(frame) = HandshakeFrame::decode(bytes) {
        let mut out = Vec::new();
        frame.encode(&mut out);
        reencodes(&out, bytes, true, &frame);
    }

    if let Some(request) = DatagramFraming.decode_request(bytes) {
        let mut out = Vec::new();
        request_head(request.call, &request, &mut out);
        assert_eq!(out, bytes, "datagram@1 request {request:?}");
    }
    if let Some((request_id, envelope)) = DatagramFraming.decode_response(bytes) {
        let out = response(&DatagramFraming, request_id, &envelope);
        reencodes(&out, bytes, envelope == Envelope::Fault, &envelope);
    }

    match Datagram2Framing.decode(bytes) {
        Some(Frame::Request {
            protocol,
            request,
            one_way,
        }) => {
            assert!(!one_way, "datagram@2 can't say one-way");
            let mut out = vec![0];
            varint(protocol.expect("datagram@2 carries the protocol"), &mut out);
            request_head(request.call, &request, &mut out);
            assert_eq!(out, bytes, "datagram@2 request {request:?}");
        }
        Some(Frame::Reply {
            protocol,
            request_id,
            envelope,
        }) => {
            let protocol = protocol.expect("datagram@2 carries the protocol");
            let mut out = Vec::new();
            Datagram2Framing.encode_reply(protocol, request_id, &envelope, &mut out);
            reencodes(&out, bytes, envelope == Envelope::Fault, &envelope);
        }
        None => {}
    }

    let jsonrpc = JsonRpcFraming;
    let _ = jsonrpc.decode_request(bytes);
    if let Some((request_id, envelope)) = jsonrpc.decode_response(bytes) {
        let out = response(&jsonrpc, request_id, &envelope);
        assert_eq!(
            jsonrpc.decode_response(&out),
            Some((request_id, envelope)),
            "jsonrpc response {envelope:?} re-encoded as {:?}",
            String::from_utf8_lossy(&out)
        );
    }
    if let Some(Frame::Reply {
        protocol,
        request_id,
        envelope,
    }) = ConnectionFraming::decode(&jsonrpc, bytes)
    {
        let mut out = Vec::new();
        jsonrpc.encode_reply(protocol.unwrap_or(0), request_id, &envelope, &mut out);
        assert_eq!(
            ConnectionFraming::decode(&jsonrpc, &out),
            Some(Frame::Reply {
                protocol,
                request_id,
                envelope
            }),
            "jsonrpc reply re-encoded as {:?}",
            String::from_utf8_lossy(&out)
        );
    }
});
