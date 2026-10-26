// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Instruction-count benchmarks of the C ABI, by Callgrind through
//! `iai-callgrind`, in pairs: each call through its `extern "C"` function
//! (the `abi` group), and the same work done on `State` and the framings
//! directly (the `core` group), as a Rust host would. The difference is what
//! crossing the boundary costs: the pointer and overlap checks, the handle,
//! `catch_unwind`, the framing chosen at run time, and the event written as
//! spans. CI's job summary lists it per pair.
//!
//! Run them with `cargo bench -p comline-runtime-ffi-bench`. They need
//! Valgrind and `iai-callgrind-runner` at the same version as the
//! `iai-callgrind` dev-dependency. CI gates every benchmark here as it does
//! the runtime's (`runtime/core/benches/runtime.rs`): one that grows by more
//! than 1% fails, unless `runtime/core/benches/accepted-regressions.toml`
//! accepts the cost; see `CONTRIBUTING.md` at the repository's root.
//!
//! The inputs are the runtime benches' (`runtime/conformance/benches/`): the
//! same payload sizes, request id, protocol, call and handshake. A
//! connection is made, and its call put in waiting, by a setup function,
//! which isn't measured, and handed back in the result with every buffer, so
//! freeing them isn't measured either. Outputs go to a buffer that already
//! has room. Connections skip the handshake (`none`): the measured calls
//! take the same path after either.

use std::hint::black_box;
use std::ptr;

use iai_callgrind::{library_benchmark, library_benchmark_group, main};

use comline_runtime::connection::{Event, State};
use comline_runtime::contract::stream::{self, Split, DEFAULT_MAX_FRAME, PREFIX_LEN};
use comline_runtime::contract::{
    name_hash, ConnectionFraming, Datagram2Framing, Envelope, HandshakeFrame, SliceBuf,
};
use comline_runtime::framing::JsonRpcFraming;
use comline_runtime_ffi::*;

#[path = "../../../core/benches/definitions/mod.rs"]
mod definitions;

use definitions::*;

const _: () = assert!(
    PAYLOAD_SIZES[0] == 16
        && PAYLOAD_SIZES[1] == 256
        && PAYLOAD_SIZES[2] == 4096
        && PAYLOAD_SIZES[3] == 65536,
    "the bench ids below name the sizes"
);

/// How frames reach the connection: one per message, or on a byte stream
/// behind a length prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Message,
    Stream,
}

/// Room for a frame around `size` bytes, its prefix included.
fn room(size: usize) -> Vec<u8> {
    vec![0; size + 64 + PREFIX_LEN]
}

// ── the ABI's side ──────────────────────────────────────────────────────────

/// A connection handle, freed when dropped.
struct Conn(*mut comline_connection);

impl Drop for Conn {
    fn drop(&mut self) {
        // SAFETY: made by `comline_connection_new`, freed once.
        unsafe { comline_connection_free(self.0) };
    }
}

/// A connection on `framing`, in `mode`, with room for one waiting call.
fn connection(framing: &str, mode: Mode) -> Conn {
    let cfg = comline_config {
        ir_hash: HANDSHAKE_IR_HASH,
        wire_format: HANDSHAKE_WIRE_FORMAT.as_ptr(),
        wire_format_len: HANDSHAKE_WIRE_FORMAT.len(),
        framing: framing.as_ptr(),
        framing_len: framing.len(),
        max_frame: 0,
        capabilities: HANDSHAKE_CAPABILITIES,
        max_waiting: 1,
        handshake: COMLINE_HANDSHAKE_NONE,
        stream: (mode == Mode::Stream) as u8,
        unordered: 0,
        _reserved: 0,
    };
    let mut conn = ptr::null_mut();
    // SAFETY: a valid config and out-pointer.
    let status = unsafe { comline_connection_new(&cfg, &mut conn) };
    assert_eq!(status, COMLINE_OK);
    Conn(conn)
}

/// Frame a call to `CONNECTION_CALL` with `body`, waiting on a reply, into
/// `out`; its request id.
fn abi_request(conn: &Conn, body: &[u8], out: &mut [u8], written: &mut usize) -> u64 {
    let name = CONNECTION_CALL.1;
    let mut request_id = 0;
    // SAFETY: every pointer is valid for its length, and none overlap.
    let status = unsafe {
        comline_connection_encode_request(
            conn.0,
            CONNECTION_PROTOCOL,
            CONNECTION_CALL.0,
            name.as_ptr(),
            name.len(),
            1,
            body.as_ptr(),
            body.len(),
            out.as_mut_ptr(),
            out.len(),
            written,
            &mut request_id,
        )
    };
    assert_eq!(status, COMLINE_OK);
    request_id
}

/// The `ok` reply to `request_id`, with `size` bytes of payload, as the peer
/// sends it in `mode`.
fn reply_frame<F: ConnectionFraming>(
    framing: &F,
    mode: Mode,
    request_id: u64,
    size: usize,
) -> Vec<u8> {
    let mut frame = Vec::new();
    framing.encode_reply(
        FRAME_PROTOCOL,
        request_id,
        &Envelope::Ok(&payload(size)),
        &mut frame,
    );
    if mode == Mode::Message {
        return frame;
    }
    let mut input = Vec::with_capacity(PREFIX_LEN + frame.len());
    stream::put_prefix(frame.len(), DEFAULT_MAX_FRAME, &mut input).expect("the frame fits");
    input.extend_from_slice(&frame);
    input
}

/// A connection waiting on a call, and the reply to it.
fn abi_feed_setup<F: ConnectionFraming>(framing: F, mode: Mode, size: usize) -> (Conn, Vec<u8>) {
    let conn = connection(framing.name(), mode);
    let request_id = abi_request(&conn, &payload(16), &mut room(16), &mut 0);
    (conn, reply_frame(&framing, mode, request_id, size))
}

/// A connection, the body, and room for the frame.
fn abi_encode_setup<F: ConnectionFraming>(
    framing: F,
    mode: Mode,
    size: usize,
) -> (Conn, Vec<u8>, Vec<u8>) {
    (connection(framing.name(), mode), payload(size), room(size))
}

/// The `abi` benchmarks of one framing: `feed`, `encode_request` and
/// `encode_reply`, per mode and payload size.
macro_rules! abi_benches {
    ($feed:ident, $request:ident, $reply:ident, $framing:expr) => {
        #[library_benchmark]
        #[bench::message_b16(args = ($framing, Mode::Message, 16), setup = abi_feed_setup)]
        #[bench::message_b256(args = ($framing, Mode::Message, 256), setup = abi_feed_setup)]
        #[bench::message_b4096(args = ($framing, Mode::Message, 4096), setup = abi_feed_setup)]
        #[bench::message_b65536(args = ($framing, Mode::Message, 65536), setup = abi_feed_setup)]
        #[bench::stream_b16(args = ($framing, Mode::Stream, 16), setup = abi_feed_setup)]
        #[bench::stream_b256(args = ($framing, Mode::Stream, 256), setup = abi_feed_setup)]
        #[bench::stream_b4096(args = ($framing, Mode::Stream, 4096), setup = abi_feed_setup)]
        #[bench::stream_b65536(args = ($framing, Mode::Stream, 65536), setup = abi_feed_setup)]
        fn $feed(input: (Conn, Vec<u8>)) -> (Conn, Vec<u8>) {
            let (conn, input) = input;
            let mut consumed = 0;
            let mut ev = comline_event::default();
            // SAFETY: every pointer is valid for its length, and none overlap.
            let status = unsafe {
                comline_connection_feed(conn.0, input.as_ptr(), input.len(), &mut consumed, &mut ev)
            };
            assert_eq!(status, COMLINE_OK);
            assert_eq!(ev.kind, COMLINE_EVENT_REPLY);
            assert_eq!(consumed, input.len());
            black_box(ev);
            (conn, input)
        }

        #[library_benchmark]
        #[bench::message_b16(args = ($framing, Mode::Message, 16), setup = abi_encode_setup)]
        #[bench::message_b256(args = ($framing, Mode::Message, 256), setup = abi_encode_setup)]
        #[bench::message_b4096(args = ($framing, Mode::Message, 4096), setup = abi_encode_setup)]
        #[bench::message_b65536(args = ($framing, Mode::Message, 65536), setup = abi_encode_setup)]
        #[bench::stream_b16(args = ($framing, Mode::Stream, 16), setup = abi_encode_setup)]
        #[bench::stream_b256(args = ($framing, Mode::Stream, 256), setup = abi_encode_setup)]
        #[bench::stream_b4096(args = ($framing, Mode::Stream, 4096), setup = abi_encode_setup)]
        #[bench::stream_b65536(args = ($framing, Mode::Stream, 65536), setup = abi_encode_setup)]
        fn $request(input: (Conn, Vec<u8>, Vec<u8>)) -> (Conn, Vec<u8>, Vec<u8>) {
            let (conn, body, mut out) = input;
            let mut written = 0;
            black_box(abi_request(&conn, &body, &mut out, &mut written));
            (conn, body, out)
        }

        #[library_benchmark]
        #[bench::message_b16(args = ($framing, Mode::Message, 16), setup = abi_encode_setup)]
        #[bench::message_b256(args = ($framing, Mode::Message, 256), setup = abi_encode_setup)]
        #[bench::message_b4096(args = ($framing, Mode::Message, 4096), setup = abi_encode_setup)]
        #[bench::message_b65536(args = ($framing, Mode::Message, 65536), setup = abi_encode_setup)]
        #[bench::stream_b16(args = ($framing, Mode::Stream, 16), setup = abi_encode_setup)]
        #[bench::stream_b256(args = ($framing, Mode::Stream, 256), setup = abi_encode_setup)]
        #[bench::stream_b4096(args = ($framing, Mode::Stream, 4096), setup = abi_encode_setup)]
        #[bench::stream_b65536(args = ($framing, Mode::Stream, 65536), setup = abi_encode_setup)]
        fn $reply(input: (Conn, Vec<u8>, Vec<u8>)) -> (Conn, Vec<u8>, Vec<u8>) {
            let (conn, body, mut out) = input;
            let mut written = 0;
            // SAFETY: every pointer is valid for its length, and none overlap.
            let status = unsafe {
                comline_connection_encode_reply(
                    conn.0,
                    FRAME_PROTOCOL,
                    FRAME_REQUEST_ID,
                    COMLINE_ENVELOPE_OK,
                    0,
                    body.as_ptr(),
                    body.len(),
                    out.as_mut_ptr(),
                    out.len(),
                    &mut written,
                )
            };
            assert_eq!(status, COMLINE_OK);
            black_box(written);
            (conn, body, out)
        }
    };
}

abi_benches!(
    abi_feed_datagram2,
    abi_encode_request_datagram2,
    abi_encode_reply_datagram2,
    Datagram2Framing
);
abi_benches!(
    abi_feed_jsonrpc1,
    abi_encode_request_jsonrpc1,
    abi_encode_reply_jsonrpc1,
    JsonRpcFraming
);

/// The names a handshake carries, and room for its frame.
fn handshake_names() -> (String, String, Vec<u8>) {
    (
        HANDSHAKE_WIRE_FORMAT.into(),
        HANDSHAKE_FRAMING.into(),
        vec![0; COMLINE_HANDSHAKE_LEN as usize],
    )
}

/// Our handshake frame and the peer's, the same.
fn handshake_pair() -> (Vec<u8>, Vec<u8>) {
    let mut ours = Vec::new();
    HandshakeFrame::new(
        HANDSHAKE_IR_HASH,
        HANDSHAKE_WIRE_FORMAT,
        HANDSHAKE_FRAMING,
        HANDSHAKE_CAPABILITIES,
    )
    .encode(&mut ours);
    (ours.clone(), ours)
}

fn wire_format_name() -> String {
    HANDSHAKE_WIRE_FORMAT.into()
}

#[library_benchmark]
#[bench::name(setup = wire_format_name)]
fn abi_name_hash(name: String) -> String {
    let mut hash = 0;
    // SAFETY: every pointer is valid for its length, and none overlap.
    let status = unsafe { comline_name_hash(name.as_ptr(), name.len(), &mut hash) };
    assert_eq!(status, COMLINE_OK);
    black_box(hash);
    name
}

#[library_benchmark]
#[bench::frame(setup = handshake_names)]
fn abi_handshake_encode(input: (String, String, Vec<u8>)) -> (String, String, Vec<u8>) {
    let (wire_format, framing, mut out) = input;
    let mut written = 0;
    // SAFETY: every pointer is valid for its length, and none overlap.
    let status = unsafe {
        comline_handshake_encode(
            HANDSHAKE_IR_HASH,
            wire_format.as_ptr(),
            wire_format.len(),
            framing.as_ptr(),
            framing.len(),
            HANDSHAKE_CAPABILITIES,
            out.as_mut_ptr(),
            out.len(),
            &mut written,
        )
    };
    assert_eq!(status, COMLINE_OK);
    black_box(written);
    (wire_format, framing, out)
}

#[library_benchmark]
#[bench::frame(setup = handshake_pair)]
fn abi_handshake_check(pair: (Vec<u8>, Vec<u8>)) -> (Vec<u8>, Vec<u8>) {
    let (ours, peer) = pair;
    // SAFETY: every pointer is valid for its length; no peer out-pointer.
    let status = unsafe {
        comline_handshake_check(
            ours.as_ptr(),
            ours.len(),
            peer.as_ptr(),
            peer.len(),
            ptr::null_mut(),
        )
    };
    assert_eq!(status, COMLINE_OK);
    (ours, peer)
}

#[library_benchmark]
#[bench::frame(setup = handshake_pair)]
fn abi_handshake_check_out(pair: (Vec<u8>, Vec<u8>)) -> (Vec<u8>, Vec<u8>) {
    let (ours, peer) = pair;
    let mut out = comline_handshake::default();
    // SAFETY: every pointer is valid for its length, and none overlap.
    let status = unsafe {
        comline_handshake_check(
            ours.as_ptr(),
            ours.len(),
            peer.as_ptr(),
            peer.len(),
            &mut out,
        )
    };
    assert_eq!(status, COMLINE_OK);
    black_box(out);
    (ours, peer)
}

library_benchmark_group!(
    name = abi;
    benchmarks =
        abi_feed_datagram2, abi_encode_request_datagram2, abi_encode_reply_datagram2,
        abi_feed_jsonrpc1, abi_encode_request_jsonrpc1, abi_encode_reply_jsonrpc1,
        abi_name_hash, abi_handshake_encode, abi_handshake_check, abi_handshake_check_out
);

// ── the same work, on the core directly ─────────────────────────────────────
//
// What the ABI does inside each call, minus the crossing: the frame limit,
// the stream's prefix, and the state and framing calls, on the concrete
// framing type and into the same buffers.

/// A state waiting on a call, and the reply to it.
fn core_feed_setup<F: ConnectionFraming>(
    framing: F,
    mode: Mode,
    size: usize,
) -> (F, Mode, State, Vec<u8>) {
    let mut state = State::with_capacity(1);
    let mut out = room(16);
    let request_id = state
        .encode_request_body(
            &framing,
            CONNECTION_PROTOCOL,
            CONNECTION_CALL.0,
            CONNECTION_CALL.1,
            &payload(16),
            true,
            &mut SliceBuf::new(&mut out),
        )
        .expect("the call encodes");
    let input = reply_frame(&framing, mode, request_id, size);
    (framing, mode, state, input)
}

/// The framing, a state, the body, and room for the frame.
fn core_encode_setup<F: ConnectionFraming>(
    framing: F,
    mode: Mode,
    size: usize,
) -> (F, Mode, State, Vec<u8>, Vec<u8>) {
    (
        framing,
        mode,
        State::with_capacity(1),
        payload(size),
        room(size),
    )
}

/// `out`'s room for a frame in `mode`: after the prefix's, at most
/// `DEFAULT_MAX_FRAME`.
fn split_out(mode: Mode, out: &mut [u8]) -> (&mut [u8], &mut [u8]) {
    let prefix = if mode == Mode::Stream { PREFIX_LEN } else { 0 };
    let (head, rest) = out.split_at_mut(prefix);
    let room = rest.len().min(DEFAULT_MAX_FRAME);
    (head, &mut rest[..room])
}

/// Writes a stream frame's prefix into `head`; the total written.
fn finish(mode: Mode, head: &mut [u8], len: usize) -> usize {
    if mode == Mode::Message {
        return len;
    }
    let mut prefix = SliceBuf::new(head);
    stream::put_prefix(len, DEFAULT_MAX_FRAME, &mut prefix).expect("the frame fits");
    assert!(!prefix.overflowed());
    PREFIX_LEN + len
}

/// The `core` benchmarks of one framing, the pairs of `abi_benches!`'s.
macro_rules! core_benches {
    ($feed:ident, $request:ident, $reply:ident, $framing:expr, $ty:ty) => {
        #[library_benchmark]
        #[bench::message_b16(args = ($framing, Mode::Message, 16), setup = core_feed_setup)]
        #[bench::message_b256(args = ($framing, Mode::Message, 256), setup = core_feed_setup)]
        #[bench::message_b4096(args = ($framing, Mode::Message, 4096), setup = core_feed_setup)]
        #[bench::message_b65536(args = ($framing, Mode::Message, 65536), setup = core_feed_setup)]
        #[bench::stream_b16(args = ($framing, Mode::Stream, 16), setup = core_feed_setup)]
        #[bench::stream_b256(args = ($framing, Mode::Stream, 256), setup = core_feed_setup)]
        #[bench::stream_b4096(args = ($framing, Mode::Stream, 4096), setup = core_feed_setup)]
        #[bench::stream_b65536(args = ($framing, Mode::Stream, 65536), setup = core_feed_setup)]
        fn $feed(input: ($ty, Mode, State, Vec<u8>)) -> (State, Vec<u8>) {
            let (framing, mode, mut state, input) = input;
            let frame = match mode {
                Mode::Stream => match stream::split_frame(&input, DEFAULT_MAX_FRAME) {
                    Split::Frame { start, len } => &input[start..start + len],
                    _ => panic!("the input is one whole frame"),
                },
                Mode::Message => {
                    assert!(input.len() <= DEFAULT_MAX_FRAME);
                    &input[..]
                }
            };
            let event = state
                .receive(&framing, frame)
                .expect("the reply is well formed");
            assert!(matches!(event, Event::Reply { .. }));
            black_box(event);
            (state, input)
        }

        #[library_benchmark]
        #[bench::message_b16(args = ($framing, Mode::Message, 16), setup = core_encode_setup)]
        #[bench::message_b256(args = ($framing, Mode::Message, 256), setup = core_encode_setup)]
        #[bench::message_b4096(args = ($framing, Mode::Message, 4096), setup = core_encode_setup)]
        #[bench::message_b65536(args = ($framing, Mode::Message, 65536), setup = core_encode_setup)]
        #[bench::stream_b16(args = ($framing, Mode::Stream, 16), setup = core_encode_setup)]
        #[bench::stream_b256(args = ($framing, Mode::Stream, 256), setup = core_encode_setup)]
        #[bench::stream_b4096(args = ($framing, Mode::Stream, 4096), setup = core_encode_setup)]
        #[bench::stream_b65536(args = ($framing, Mode::Stream, 65536), setup = core_encode_setup)]
        fn $request(input: ($ty, Mode, State, Vec<u8>, Vec<u8>)) -> (State, Vec<u8>, Vec<u8>) {
            let (framing, mode, mut state, body, mut out) = input;
            let (head, room) = split_out(mode, &mut out);
            let mut buf = SliceBuf::new(room);
            let request_id = state
                .encode_request_body(
                    &framing,
                    CONNECTION_PROTOCOL,
                    CONNECTION_CALL.0,
                    CONNECTION_CALL.1,
                    &body,
                    true,
                    &mut buf,
                )
                .expect("the call encodes");
            let len = buf.written().len();
            black_box((finish(mode, head, len), request_id));
            (state, body, out)
        }

        #[library_benchmark]
        #[bench::message_b16(args = ($framing, Mode::Message, 16), setup = core_encode_setup)]
        #[bench::message_b256(args = ($framing, Mode::Message, 256), setup = core_encode_setup)]
        #[bench::message_b4096(args = ($framing, Mode::Message, 4096), setup = core_encode_setup)]
        #[bench::message_b65536(args = ($framing, Mode::Message, 65536), setup = core_encode_setup)]
        #[bench::stream_b16(args = ($framing, Mode::Stream, 16), setup = core_encode_setup)]
        #[bench::stream_b256(args = ($framing, Mode::Stream, 256), setup = core_encode_setup)]
        #[bench::stream_b4096(args = ($framing, Mode::Stream, 4096), setup = core_encode_setup)]
        #[bench::stream_b65536(args = ($framing, Mode::Stream, 65536), setup = core_encode_setup)]
        fn $reply(input: ($ty, Mode, State, Vec<u8>, Vec<u8>)) -> (State, Vec<u8>, Vec<u8>) {
            let (framing, mode, state, body, mut out) = input;
            let (head, room) = split_out(mode, &mut out);
            let mut buf = SliceBuf::new(room);
            framing.encode_reply(
                FRAME_PROTOCOL,
                FRAME_REQUEST_ID,
                &Envelope::Ok(&body),
                &mut buf,
            );
            assert!(!buf.overflowed());
            let len = buf.written().len();
            black_box(finish(mode, head, len));
            (state, body, out)
        }
    };
}

core_benches!(
    core_feed_datagram2,
    core_encode_request_datagram2,
    core_encode_reply_datagram2,
    Datagram2Framing,
    Datagram2Framing
);
core_benches!(
    core_feed_jsonrpc1,
    core_encode_request_jsonrpc1,
    core_encode_reply_jsonrpc1,
    JsonRpcFraming,
    JsonRpcFraming
);

#[library_benchmark]
#[bench::name(setup = wire_format_name)]
fn core_name_hash(name: String) -> String {
    black_box(name_hash(&name));
    name
}

#[library_benchmark]
#[bench::frame(setup = handshake_names)]
fn core_handshake_encode(input: (String, String, Vec<u8>)) -> (String, String, Vec<u8>) {
    let (wire_format, framing, mut out) = input;
    let frame = HandshakeFrame::new(
        HANDSHAKE_IR_HASH,
        &wire_format,
        &framing,
        HANDSHAKE_CAPABILITIES,
    );
    let mut buf = SliceBuf::new(&mut out);
    frame.encode(&mut buf);
    assert!(!buf.overflowed());
    black_box(buf.written().len());
    (wire_format, framing, out)
}

#[library_benchmark]
#[bench::frame(setup = handshake_pair)]
fn core_handshake_check(pair: (Vec<u8>, Vec<u8>)) -> (Vec<u8>, Vec<u8>) {
    let (ours, peer) = pair;
    let frame = HandshakeFrame::decode(&ours).expect("our frame parses");
    black_box(frame.check_bytes(&peer)).expect("the frames agree");
    (ours, peer)
}

#[library_benchmark]
#[bench::frame(setup = handshake_pair)]
fn core_handshake_check_out(pair: (Vec<u8>, Vec<u8>)) -> (Vec<u8>, Vec<u8>) {
    let (ours, peer) = pair;
    let frame = HandshakeFrame::decode(&ours).expect("our frame parses");
    black_box(HandshakeFrame::decode(&peer));
    black_box(frame.check_bytes(&peer)).expect("the frames agree");
    (ours, peer)
}

library_benchmark_group!(
    name = core;
    benchmarks =
        core_feed_datagram2, core_encode_request_datagram2, core_encode_reply_datagram2,
        core_feed_jsonrpc1, core_encode_request_jsonrpc1, core_encode_reply_jsonrpc1,
        core_name_hash, core_handshake_encode, core_handshake_check, core_handshake_check_out
);

main!(library_benchmark_groups = abi, core);
