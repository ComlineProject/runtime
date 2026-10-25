// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Instruction-count benchmarks of the runtime, by Callgrind through
//! `iai-callgrind`: the work `runtime/conformance/benches/` defines, so every
//! language measures the same thing.
//!
//! Run them with `cargo bench -p comline-runtime`. They need Valgrind and
//! `iai-callgrind-runner` at the same version as the `iai-callgrind`
//! dev-dependency.
//!
//! Each benchmark's inputs (values, buffers, encoded bytes) are built by a
//! setup function, which isn't measured, and handed back in the result, so
//! dropping them isn't measured either. Encoding writes into a buffer that
//! already has room, as the call system's reused buffer does.

use std::hint::black_box;

use iai_callgrind::{library_benchmark, library_benchmark_group, main};
use serde::de::DeserializeOwned;
use serde::Serialize;

use comline_runtime::connection::{Event, State};
use comline_runtime::contract::{
    Call, ConnectionFraming, Datagram2Framing, DatagramFraming, Envelope, Frame, Framing,
    HandshakeFrame, WireFormat,
};
use comline_runtime::format::{Json, MsgPack, Packed};
use comline_runtime::framing::JsonRpcFraming;

mod definitions;

use definitions::*;

// ── encode and decode, per format, per shape ────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum Format {
    Json,
    MsgPack,
    Packed,
}

#[inline(always)]
fn encode<T: Serialize>(format: Format, value: &T, out: &mut Vec<u8>) {
    match format {
        Format::Json => Json.encode(value, out),
        Format::MsgPack => MsgPack.encode(value, out),
        Format::Packed => Packed.encode(value, out),
    }
    .expect("bench values encode");
}

#[inline(always)]
fn decode<T: DeserializeOwned>(format: Format, bytes: &[u8]) -> T {
    match format {
        Format::Json => Json.decode(bytes),
        Format::MsgPack => MsgPack.decode(bytes),
        Format::Packed => Packed.decode(bytes),
    }
    .expect("bench bytes decode")
}

fn encoded<T: Serialize>(format: Format, value: &T) -> Vec<u8> {
    let mut out = Vec::new();
    encode(format, value, &mut out);
    out
}

/// A shape's mirror type, and how to build its value.
trait Shape: Serialize + DeserializeOwned {
    fn make() -> Self;
}

impl Shape for Reading {
    fn make() -> Self {
        small_scalar()
    }
}

impl Shape for Profile {
    fn make() -> Self {
        string_heavy()
    }
}

impl Shape for Order {
    fn make() -> Self {
        nested()
    }
}

impl Shape for Vec<Sample> {
    fn make() -> Self {
        large_array()
    }
}

impl Shape for Job {
    fn make() -> Self {
        enum_heavy()
    }
}

/// The value, and room to encode it.
fn shape_encode_setup<T: Shape>(format: Format) -> (Format, T, Vec<u8>) {
    let value = T::make();
    let room = encoded(format, &value).len();
    (format, value, Vec::with_capacity(room))
}

/// The value's bytes.
fn shape_decode_setup<T: Shape>(format: Format) -> (Format, Vec<u8>) {
    (format, encoded(format, &T::make()))
}

/// The encode and decode benchmarks of one shape, `$ty`.
macro_rules! shape_benches {
    ($encode:ident, $decode:ident, $ty:ty) => {
        #[library_benchmark]
        #[bench::json(args = (Format::Json), setup = shape_encode_setup::<$ty>)]
        #[bench::msgpack(args = (Format::MsgPack), setup = shape_encode_setup::<$ty>)]
        #[bench::packed(args = (Format::Packed), setup = shape_encode_setup::<$ty>)]
        fn $encode(input: (Format, $ty, Vec<u8>)) -> ($ty, Vec<u8>) {
            let (format, value, mut out) = input;
            encode(format, &value, &mut out);
            (value, out)
        }

        #[library_benchmark]
        #[bench::json(args = (Format::Json), setup = shape_decode_setup::<$ty>)]
        #[bench::msgpack(args = (Format::MsgPack), setup = shape_decode_setup::<$ty>)]
        #[bench::packed(args = (Format::Packed), setup = shape_decode_setup::<$ty>)]
        fn $decode(input: (Format, Vec<u8>)) -> ($ty, Vec<u8>) {
            let (format, bytes) = input;
            let value: $ty = decode(format, &bytes);
            (value, bytes)
        }
    };
}

shape_benches!(encode_small_scalar, decode_small_scalar, Reading);
shape_benches!(encode_string_heavy, decode_string_heavy, Profile);
shape_benches!(encode_nested, decode_nested, Order);
shape_benches!(encode_large_array, decode_large_array, Vec<Sample>);
shape_benches!(encode_enum_heavy, decode_enum_heavy, Job);

library_benchmark_group!(
    name = formats;
    benchmarks =
        encode_small_scalar, decode_small_scalar,
        encode_string_heavy, decode_string_heavy,
        encode_nested, decode_nested,
        encode_large_array, decode_large_array,
        encode_enum_heavy, decode_enum_heavy
);

// ── framing encode and decode, per framing, per payload size ────────────────

const _: () = assert!(
    PAYLOAD_SIZES[0] == 16
        && PAYLOAD_SIZES[1] == 256
        && PAYLOAD_SIZES[2] == 4096
        && PAYLOAD_SIZES[3] == 65536,
    "the bench ids below name the sizes"
);

/// The payload, and room for the frame around it.
fn encode_setup(size: usize) -> (Vec<u8>, Vec<u8>) {
    (payload(size), Vec::with_capacity(size + 64))
}

fn datagram1_frame(size: usize) -> Vec<u8> {
    let mut out = Vec::new();
    DatagramFraming.encode_response_ok(FRAME_REQUEST_ID, &payload(size), &mut out);
    out
}

fn datagram2_frame(size: usize) -> Vec<u8> {
    let mut out = Vec::new();
    Datagram2Framing.encode_reply(
        FRAME_PROTOCOL,
        FRAME_REQUEST_ID,
        &Envelope::Ok(&payload(size)),
        &mut out,
    );
    out
}

fn jsonrpc1_frame(size: usize) -> Vec<u8> {
    let mut out = Vec::new();
    Framing::encode_response_ok(&JsonRpcFraming, FRAME_REQUEST_ID, &payload(size), &mut out);
    out
}

#[library_benchmark]
#[bench::b16(args = (16), setup = encode_setup)]
#[bench::b256(args = (256), setup = encode_setup)]
#[bench::b4096(args = (4096), setup = encode_setup)]
#[bench::b65536(args = (65536), setup = encode_setup)]
fn encode_datagram1(input: (Vec<u8>, Vec<u8>)) -> (Vec<u8>, Vec<u8>) {
    let (payload, mut out) = input;
    DatagramFraming.encode_response_ok(FRAME_REQUEST_ID, &payload, &mut out);
    (payload, out)
}

#[library_benchmark]
#[bench::b16(args = (16), setup = encode_setup)]
#[bench::b256(args = (256), setup = encode_setup)]
#[bench::b4096(args = (4096), setup = encode_setup)]
#[bench::b65536(args = (65536), setup = encode_setup)]
fn encode_datagram2(input: (Vec<u8>, Vec<u8>)) -> (Vec<u8>, Vec<u8>) {
    let (payload, mut out) = input;
    Datagram2Framing.encode_reply(
        FRAME_PROTOCOL,
        FRAME_REQUEST_ID,
        &Envelope::Ok(&payload),
        &mut out,
    );
    (payload, out)
}

#[library_benchmark]
#[bench::b16(args = (16), setup = encode_setup)]
#[bench::b256(args = (256), setup = encode_setup)]
#[bench::b4096(args = (4096), setup = encode_setup)]
#[bench::b65536(args = (65536), setup = encode_setup)]
fn encode_jsonrpc1(input: (Vec<u8>, Vec<u8>)) -> (Vec<u8>, Vec<u8>) {
    let (payload, mut out) = input;
    Framing::encode_response_ok(&JsonRpcFraming, FRAME_REQUEST_ID, &payload, &mut out);
    (payload, out)
}

#[library_benchmark]
#[bench::b16(args = (16), setup = datagram1_frame)]
#[bench::b256(args = (256), setup = datagram1_frame)]
#[bench::b4096(args = (4096), setup = datagram1_frame)]
#[bench::b65536(args = (65536), setup = datagram1_frame)]
fn decode_datagram1(frame: Vec<u8>) -> Vec<u8> {
    let decoded = DatagramFraming.decode_response(&frame);
    assert!(matches!(decoded, Some((FRAME_REQUEST_ID, Envelope::Ok(_)))));
    black_box(decoded);
    frame
}

#[library_benchmark]
#[bench::b16(args = (16), setup = datagram2_frame)]
#[bench::b256(args = (256), setup = datagram2_frame)]
#[bench::b4096(args = (4096), setup = datagram2_frame)]
#[bench::b65536(args = (65536), setup = datagram2_frame)]
fn decode_datagram2(frame: Vec<u8>) -> Vec<u8> {
    let decoded = Datagram2Framing.decode(&frame);
    assert!(matches!(
        decoded,
        Some(Frame::Reply {
            request_id: FRAME_REQUEST_ID,
            envelope: Envelope::Ok(_),
            ..
        })
    ));
    black_box(decoded);
    frame
}

#[library_benchmark]
#[bench::b16(args = (16), setup = jsonrpc1_frame)]
#[bench::b256(args = (256), setup = jsonrpc1_frame)]
#[bench::b4096(args = (4096), setup = jsonrpc1_frame)]
#[bench::b65536(args = (65536), setup = jsonrpc1_frame)]
fn decode_jsonrpc1(frame: Vec<u8>) -> Vec<u8> {
    let decoded = Framing::decode_response(&JsonRpcFraming, &frame);
    assert!(matches!(decoded, Some((FRAME_REQUEST_ID, Envelope::Ok(_)))));
    black_box(decoded);
    frame
}

library_benchmark_group!(
    name = framings;
    benchmarks =
        encode_datagram1, decode_datagram1,
        encode_datagram2, decode_datagram2,
        encode_jsonrpc1, decode_jsonrpc1
);

// ── the handshake ───────────────────────────────────────────────────────────

fn handshake_frame() -> HandshakeFrame {
    HandshakeFrame::new(
        HANDSHAKE_IR_HASH,
        HANDSHAKE_WIRE_FORMAT,
        HANDSHAKE_FRAMING,
        HANDSHAKE_CAPABILITIES,
    )
}

fn handshake_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    handshake_frame().encode(&mut out);
    out
}

fn handshake_encode_setup() -> (HandshakeFrame, Vec<u8>) {
    (
        handshake_frame(),
        Vec::with_capacity(handshake_bytes().len()),
    )
}

fn handshake_pair() -> (HandshakeFrame, HandshakeFrame) {
    (handshake_frame(), handshake_frame())
}

#[library_benchmark]
#[bench::frame(setup = handshake_encode_setup)]
fn handshake_encode(input: (HandshakeFrame, Vec<u8>)) -> Vec<u8> {
    let (frame, mut out) = input;
    frame.encode(&mut out);
    out
}

#[library_benchmark]
#[bench::frame(setup = handshake_bytes)]
fn handshake_decode(bytes: Vec<u8>) -> (Option<HandshakeFrame>, Vec<u8>) {
    (HandshakeFrame::decode(&bytes), bytes)
}

#[library_benchmark]
#[bench::frame(setup = handshake_pair)]
fn handshake_check(pair: (HandshakeFrame, HandshakeFrame)) {
    let (ours, theirs) = pair;
    black_box(ours.check(&theirs)).expect("the frames agree");
}

library_benchmark_group!(
    name = handshake;
    benchmarks = handshake_encode, handshake_decode, handshake_check
);

// ── the connection step ─────────────────────────────────────────────────────

struct Step {
    state: State,
    params: Reading,
    out: Vec<u8>,
    reply: Vec<u8>,
}

fn call(state: &mut State, params: &Reading, out: &mut Vec<u8>) -> u64 {
    state
        .encode_request(
            &Datagram2Framing,
            CONNECTION_PROTOCOL,
            Call::new(CONNECTION_CALL.0, CONNECTION_CALL.1),
            params,
            &Packed,
            true,
            out,
        )
        .expect("the call encodes")
}

fn connection_setup() -> Step {
    assert_eq!(CONNECTION_FRAMING, Datagram2Framing.name());
    assert_eq!(CONNECTION_FORMAT, Packed.name());
    assert_eq!(CONNECTION_PARAMS, "small-scalar");
    let params = small_scalar();
    let mut out = Vec::new();
    let request_id = call(&mut State::default(), &params, &mut out);
    let mut reply = Vec::new();
    Datagram2Framing.encode_reply(
        CONNECTION_PROTOCOL,
        request_id,
        &Envelope::Ok(&[]),
        &mut reply,
    );
    Step {
        state: State::default(),
        params,
        out: Vec::with_capacity(out.len()),
        reply,
    }
}

#[library_benchmark]
#[bench::step(setup = connection_setup)]
fn connection_step(step: Step) -> Step {
    let Step {
        mut state,
        params,
        mut out,
        reply,
    } = step;
    let request_id = call(&mut state, &params, &mut out);
    let event = state
        .receive(&Datagram2Framing, &reply)
        .expect("the reply is well formed");
    assert!(matches!(event, Event::Reply { request_id: id, .. } if id == request_id));
    black_box(event);
    Step {
        state,
        params,
        out,
        reply,
    }
}

library_benchmark_group!(
    name = connection;
    benchmarks = connection_step
);

main!(
    library_benchmark_groups = formats,
    framings,
    handshake,
    connection
);
