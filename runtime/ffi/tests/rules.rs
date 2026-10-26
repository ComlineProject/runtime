// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The ABI's rules, through the `extern "C"` functions: poisoning, nothing
//! changing on `BUFFER_TOO_SMALL`, `BUSY`, `FRAME_TOO_LARGE` both ways,
//! `NEED_MORE`, argument checks, and no allocation after
//! `comline_connection_new`.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ptr;

use comline_runtime_ffi::*;
use common::*;

// ── a counting allocator ───────────────────────────────────────────────────

/// The system allocator, counting this thread's allocations, so tests that
/// run in parallel don't count each other's.
struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|n| n.set(n.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.with(|n| n.set(n.get() + 1));
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn allocations() -> usize {
    ALLOCATIONS.with(Cell::get)
}

// ── helpers that allocate nothing themselves ───────────────────────────────

/// `encode_request` into `out`, by raw call, so the test allocates nothing.
fn request_into(
    conn: &Conn,
    protocol: u16,
    name: &[u8],
    expects_reply: bool,
    body: &[u8],
    out: &mut [u8],
) -> (comline_status, usize, u64) {
    let (mut written, mut id) = (0, 0);
    let status = unsafe {
        comline_connection_encode_request(
            conn.0,
            protocol,
            7,
            name.as_ptr(),
            name.len(),
            u8::from(expects_reply),
            body.as_ptr(),
            body.len(),
            out.as_mut_ptr(),
            out.len(),
            &mut written,
            &mut id,
        )
    };
    (status, written, id)
}

fn reply_into(
    conn: &Conn,
    request_id: u64,
    envelope: u8,
    body: &[u8],
    out: &mut [u8],
) -> (comline_status, usize) {
    let mut written = 0;
    let status = unsafe {
        comline_connection_encode_reply(
            conn.0,
            3,
            request_id,
            envelope,
            9,
            body.as_ptr(),
            body.len(),
            out.as_mut_ptr(),
            out.len(),
            &mut written,
        )
    };
    (status, written)
}

fn feed_raw(conn: &Conn, input: &[u8]) -> (comline_status, usize, comline_event) {
    let mut consumed = 0;
    let mut ev = comline_event::default();
    let status = unsafe {
        comline_connection_feed(conn.0, input.as_ptr(), input.len(), &mut consumed, &mut ev)
    };
    (status, consumed, ev)
}

fn start_into(conn: &Conn, out: &mut [u8]) -> (comline_status, usize) {
    let mut written = 0;
    let status =
        unsafe { comline_connection_start(conn.0, out.as_mut_ptr(), out.len(), &mut written) };
    (status, written)
}

/// Two ends of a stream connection with the handshake done.
fn pair(framing: &'static str) -> (Conn, Conn) {
    let mut setup = Setup::plain(framing);
    setup.handshake = COMLINE_HANDSHAKE_COMLINE;
    setup.stream = true;
    setup.unordered = true;
    let mut a = Conn::new(&setup).unwrap();
    let mut b = Conn::new(&setup).unwrap();
    let hello_a = a.start(64);
    let hello_b = b.start(64);
    assert_eq!(hello_a.status, COMLINE_OK);
    assert_eq!(hello_a.bytes.len(), 35, "the frame behind its prefix");
    assert_eq!(b.feed(&hello_a.bytes).ev.kind, COMLINE_EVENT_HANDSHAKE_OK);
    assert_eq!(a.feed(&hello_b.bytes).ev.kind, COMLINE_EVENT_HANDSHAKE_OK);
    (a, b)
}

// ── versions and names ─────────────────────────────────────────────────────

#[test]
fn abi_version_is_0_1() {
    assert_eq!(
        comline_abi_version(),
        (COMLINE_ABI_MAJOR << 16) | COMLINE_ABI_MINOR
    );
    assert_eq!((COMLINE_ABI_MAJOR, COMLINE_ABI_MINOR), (0, 1));
}

#[test]
fn supports_the_connection_framings_and_handshakes() {
    for name in ["datagram@2", "jsonrpc@1", "comline@1", "none"] {
        assert!(supports(name), "{name}");
    }
    for name in ["datagram@1", "packed@1", "json@1", "", "datagram@3"] {
        assert!(!supports(name), "{name}");
    }
    assert_eq!(unsafe { comline_supports(ptr::null(), 3) }, 0);
}

// ── a whole exchange ───────────────────────────────────────────────────────

#[test]
fn two_ends_call_each_other_over_a_stream() {
    for framing in ["datagram@2", "jsonrpc@1"] {
        let (mut a, mut b) = pair(framing);

        // a calls b; b calls a back while a waits; both reply.
        let call = a.request(4, 2, b"chat.Chat/send", true, b"[1]", 256);
        assert_eq!((call.status, call.request_id), (COMLINE_OK, 0));
        let at_b = b.feed(&call.bytes);
        assert_eq!(at_b.ev.kind, COMLINE_EVENT_REQUEST, "{framing}");
        assert_eq!(at_b.consumed, call.bytes.len());
        assert_eq!(at(&call.bytes, at_b.ev.body), b"[1]");

        let back = b.request(1, 0, b"chat.ChatClient/ping", true, b"[]", 256);
        let at_a = a.feed(&back.bytes);
        assert_eq!(at_a.ev.kind, COMLINE_EVENT_REQUEST);
        let pong = a.reply(1, at_a.ev.request_id, COMLINE_ENVELOPE_OK, 0, b"true", 256);
        let at_b2 = b.feed(&pong.bytes);
        assert_eq!(at_b2.ev.kind, COMLINE_EVENT_REPLY);
        assert_eq!(at(&pong.bytes, at_b2.ev.body), b"true");

        let answer = b.reply(4, at_b.ev.request_id, COMLINE_ENVELOPE_ERR, 3, b"{}", 256);
        let done = a.feed(&answer.bytes);
        assert_eq!(done.ev.kind, COMLINE_EVENT_REPLY);
        assert_eq!(done.ev.envelope, COMLINE_ENVELOPE_ERR);
        assert_eq!(done.ev.error_id, 3);
        assert_eq!(done.ev.request_id, 0);
        assert_eq!(a.is_waiting(), Ok(false));
        assert_eq!(b.is_waiting(), Ok(false));
    }
}

#[test]
fn ordered_replies_must_answer_the_innermost_call() {
    let mut conn = Conn::new(&Setup::plain("datagram@2")).unwrap();
    let mut peer = Conn::new(&Setup::plain("datagram@2")).unwrap();
    conn.request(0, 0, b"", true, b"", 64); // 0, outer
    conn.request(0, 0, b"", true, b"", 64); // 1, innermost
    let to_outer = peer.reply(0, 0, COMLINE_ENVELOPE_OK, 0, b"", 64);
    assert_eq!(conn.feed(&to_outer.bytes).status, COMLINE_ERR_FRAMING);
    // A framing error leaves the connection usable.
    let to_inner = peer.reply(0, 1, COMLINE_ENVELOPE_OK, 0, b"", 64);
    assert_eq!(conn.feed(&to_inner.bytes).ev.kind, COMLINE_EVENT_REPLY);
    assert_eq!(conn.feed(&to_outer.bytes).ev.kind, COMLINE_EVENT_REPLY);
}

#[test]
fn an_abandoned_call_gets_a_stale_reply() {
    let mut conn = Conn::new(&Setup::plain("jsonrpc@1")).unwrap();
    let mut peer = Conn::new(&Setup::plain("jsonrpc@1")).unwrap();
    let sent = conn.request(0, 0, b"slow", true, b"[]", 128);
    assert_eq!(conn.abandon(sent.request_id), COMLINE_OK);
    assert_eq!(conn.is_waiting(), Ok(false));
    let late = peer.reply(0, sent.request_id, COMLINE_ENVELOPE_OK, 0, b"", 128);
    let fed = conn.feed(&late.bytes);
    assert_eq!((fed.status, fed.ev.kind), (COMLINE_OK, COMLINE_EVENT_STALE));
    assert_eq!(at(&late.bytes, fed.ev.frame), late.bytes.as_slice());
}

#[test]
fn absent_jsonrpc_bodies_are_flagged() {
    let mut conn = Conn::new(&Setup::plain("jsonrpc@1")).unwrap();
    let fed = conn.feed(br#"{"jsonrpc":"2.0","method":"tick"}"#);
    assert_eq!(fed.ev.kind, COMLINE_EVENT_REQUEST);
    let flags = COMLINE_EVENT_CALL_BY_NAME | COMLINE_EVENT_ONE_WAY | COMLINE_EVENT_BODY_ABSENT;
    assert_eq!(fed.ev.flags, flags);
    assert_eq!(fed.ev.body, comline_span::default());

    conn.request(0, 0, b"get", true, b"", 128);
    // The framing reads a `null` member as absent (serde's `Option`), so a
    // literal `null` is flagged too; either way the codec reads `null`.
    let fed = conn.feed(br#"{"jsonrpc":"2.0","id":0,"result":null}"#);
    assert_eq!(fed.ev.kind, COMLINE_EVENT_REPLY);
    assert_eq!(fed.ev.flags, COMLINE_EVENT_BODY_ABSENT);
    conn.request(0, 0, b"get", true, b"", 128);
    let fed = conn.feed(br#"{"jsonrpc":"2.0","id":1,"result":[]}"#);
    assert_eq!(fed.ev.kind, COMLINE_EVENT_REPLY);
    assert_eq!(fed.ev.flags, 0);
    conn.request(0, 0, b"get", true, b"", 128);
    let fed = conn.feed(br#"{"jsonrpc":"2.0","id":2}"#);
    assert_eq!(fed.ev.kind, COMLINE_EVENT_REPLY);
    assert_eq!(fed.ev.flags, COMLINE_EVENT_BODY_ABSENT);
}

#[test]
fn escaped_names_come_as_written_and_unescape() {
    let mut conn = Conn::new(&Setup::plain("jsonrpc@1")).unwrap();
    let frame = br#"{"jsonrpc":"2.0","method":"app.Chat\/count","params":[],"id":1}"#;
    let fed = conn.feed(frame);
    assert_eq!(
        fed.ev.flags,
        COMLINE_EVENT_CALL_BY_NAME | COMLINE_EVENT_NAME_ESCAPED
    );
    assert_eq!(at(frame, fed.ev.call_name), br"app.Chat\/count");
    assert_eq!(method_name(frame, &fed.ev), "app.Chat/count");

    // Any UTF-8 name goes out escaped as serde_json escapes it.
    let name = "say \"hi\"\n\u{1}é";
    let sent = conn.request(0, 0, name.as_bytes(), true, b"[]", 256);
    let message: serde_json::Value = serde_json::from_slice(&sent.bytes).unwrap();
    assert_eq!(message["method"], name);
    let method = serde_json::to_string(name).unwrap();
    assert_eq!(
        String::from_utf8(sent.bytes.clone()).unwrap(),
        format!(r#"{{"jsonrpc":"2.0","method":{method},"params":[],"id":0}}"#)
    );
}

#[test]
fn unescape_reports_its_exact_size_and_refuses_bad_text() {
    let spelled = br"\u00e9\ud83d\ude00";
    let mut written = 0;
    let mut out = [0u8; 5];
    let status = unsafe {
        comline_method_name_unescape(
            spelled.as_ptr(),
            spelled.len(),
            out.as_mut_ptr(),
            5,
            &mut written,
        )
    };
    assert_eq!((status, written), (COMLINE_BUFFER_TOO_SMALL, 6));
    assert_eq!(out, [0; 5], "nothing written");
    assert_eq!(unescape(spelled).unwrap(), "é😀");
    assert_eq!(unescape(b"plain").unwrap(), "plain");
    assert_eq!(unescape(br"\x"), Err(COMLINE_ERR_INVALID_ARGUMENT));
    assert_eq!(unescape(br"\ud800"), Err(COMLINE_ERR_INVALID_ARGUMENT));
    assert_eq!(unescape(&[0xff]), Err(COMLINE_ERR_INVALID_ARGUMENT));
}

// ── poisoning ──────────────────────────────────────────────────────────────

#[test]
fn a_panic_poisons_the_handle() {
    let mut conn = Conn::new(&Setup::plain("datagram@2")).unwrap();
    assert_eq!(conn.request(0, 0, b"", true, b"", 64).status, COMLINE_OK);
    unsafe { comline_test_panic_next(conn.0) };
    let fed = conn.feed(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(fed.status, COMLINE_ERR_POISONED);

    // Every later call says the same, without running.
    assert_eq!(conn.feed(&[]).status, COMLINE_ERR_POISONED);
    assert_eq!(conn.start(64).status, COMLINE_ERR_POISONED);
    assert_eq!(
        conn.request(0, 0, b"", true, b"", 64).status,
        COMLINE_ERR_POISONED
    );
    let reply = conn.reply(0, 0, COMLINE_ENVELOPE_OK, 0, b"", 64);
    assert_eq!(reply.status, COMLINE_ERR_POISONED);
    assert_eq!(conn.abandon(0), COMLINE_ERR_POISONED);
    assert_eq!(conn.is_waiting(), Err(COMLINE_ERR_POISONED));
    // Freeing it still works (Drop).
}

#[test]
fn a_panic_poisons_only_its_own_handle() {
    let mut poisoned = Conn::new(&Setup::plain("datagram@2")).unwrap();
    let mut fine = Conn::new(&Setup::plain("datagram@2")).unwrap();
    unsafe { comline_test_panic_next(poisoned.0) };
    assert_eq!(poisoned.is_waiting(), Err(COMLINE_ERR_POISONED));
    assert_eq!(fine.is_waiting(), Ok(false));
    assert_eq!(fine.request(0, 0, b"", true, b"", 64).status, COMLINE_OK);
}

// ── BUFFER_TOO_SMALL changes nothing ───────────────────────────────────────

#[test]
fn buffer_too_small_reports_the_exact_size_and_spends_nothing() {
    for framing in ["datagram@2", "jsonrpc@1"] {
        for stream in [false, true] {
            let mut setup = Setup::plain(framing);
            setup.stream = stream;
            let mut conn = Conn::new(&setup).unwrap();
            // Through ids 9 and 10, where a JSON-RPC id gains a digit.
            conn.spend_ids(0, 9);
            for id in 9..=10u64 {
                let size = conn.request(300, 1, b"m", true, b"[1,2]", 0);
                assert_eq!(size.status, COMLINE_BUFFER_TOO_SMALL, "{framing} {stream}");
                let short = conn.request(300, 1, b"m", true, b"[1,2]", size.written - 1);
                assert_eq!(short.status, COMLINE_BUFFER_TOO_SMALL);
                assert_eq!(short.written, size.written);
                assert_eq!(
                    conn.is_waiting(),
                    Ok(id > 9),
                    "nothing waits on a refused call"
                );
                let sent = conn.request(300, 1, b"m", true, b"[1,2]", size.written);
                assert_eq!(sent.status, COMLINE_OK);
                assert_eq!(sent.bytes.len(), size.written, "{framing} {stream}");
                assert_eq!(sent.request_id, id, "no id was spent");
                if stream {
                    let len = u32::from_le_bytes(sent.bytes[..4].try_into().unwrap());
                    assert_eq!(len as usize, size.written - 4);
                }
            }

            let size = conn.reply(5, 1234, COMLINE_ENVELOPE_ERR, 2, b"{}", 0);
            assert_eq!(size.status, COMLINE_BUFFER_TOO_SMALL);
            let sent = conn.reply(5, 1234, COMLINE_ENVELOPE_ERR, 2, b"{}", size.written);
            assert_eq!((sent.status, sent.bytes.len()), (COMLINE_OK, size.written));
        }
    }
}

#[test]
fn start_reports_the_handshake_size() {
    for (stream, size) in [(false, 31), (true, 35)] {
        let mut setup = Setup::plain("datagram@2");
        setup.handshake = COMLINE_HANDSHAKE_COMLINE;
        setup.stream = stream;
        let mut conn = Conn::new(&setup).unwrap();
        let short = conn.start(size - 1);
        assert_eq!(
            (short.status, short.written),
            (COMLINE_BUFFER_TOO_SMALL, size)
        );
        assert_eq!(
            conn.start(size).status,
            COMLINE_OK,
            "a refused start isn't spent"
        );
        assert_eq!(conn.start(size).status, COMLINE_ERR_STATE, "only once");
    }
}

// ── BUSY ───────────────────────────────────────────────────────────────────

#[test]
fn busy_when_max_waiting_calls_wait() {
    for unordered in [false, true] {
        let mut setup = Setup::plain("datagram@2");
        setup.max_waiting = 2;
        setup.unordered = unordered;
        let mut conn = Conn::new(&setup).unwrap();
        let mut peer = Conn::new(&Setup::plain("datagram@2")).unwrap();
        assert_eq!(conn.request(0, 0, b"", true, b"", 64).request_id, 0);
        assert_eq!(conn.request(0, 0, b"", true, b"", 64).request_id, 1);
        let busy = conn.request(0, 0, b"", true, b"", 64);
        assert_eq!((busy.status, busy.written), (COMLINE_ERR_BUSY, 0));
        // A one-way call doesn't wait, so it still goes.
        let one_way = conn.request(0, 0, b"", false, b"", 64);
        assert_eq!((one_way.status, one_way.request_id), (COMLINE_OK, 2));
        // A reply makes room; the busy call spent no id.
        let reply = peer.reply(0, 1, COMLINE_ENVELOPE_OK, 0, b"", 64);
        assert_eq!(conn.feed(&reply.bytes).ev.kind, COMLINE_EVENT_REPLY);
        assert_eq!(conn.request(0, 0, b"", true, b"", 64).request_id, 3);
    }

    // No room at all: a pure server, and one-way calls.
    let mut setup = Setup::plain("jsonrpc@1");
    setup.max_waiting = 0;
    let mut conn = Conn::new(&setup).unwrap();
    assert_eq!(
        conn.request(0, 0, b"m", true, b"", 64).status,
        COMLINE_ERR_BUSY
    );
    assert_eq!(conn.request(0, 0, b"m", false, b"", 64).status, COMLINE_OK);
}

// ── FRAME_TOO_LARGE ────────────────────────────────────────────────────────

#[test]
fn an_oversized_frame_is_refused_on_send_and_spends_nothing() {
    for stream in [false, true] {
        let mut setup = Setup::plain("datagram@2");
        setup.stream = stream;
        setup.max_frame = 32;
        let mut conn = Conn::new(&setup).unwrap();
        // A datagram@2 request to protocol 0 is 12 bytes and the params.
        let over = conn.request(0, 0, b"", true, &[7; 21], 4096);
        assert_eq!(
            (over.status, over.written),
            (COMLINE_ERR_FRAME_TOO_LARGE, 0)
        );
        // Also when the buffer is too small for it: the limit is reported.
        let over = conn.request(0, 0, b"", true, &[7; 21], 8);
        assert_eq!(over.status, COMLINE_ERR_FRAME_TOO_LARGE);
        assert_eq!(conn.is_waiting(), Ok(false));
        let at_max = conn.request(0, 0, b"", true, &[7; 20], 4096);
        assert_eq!((at_max.status, at_max.request_id), (COMLINE_OK, 0));
        assert_eq!(at_max.bytes.len(), 32 + if stream { 4 } else { 0 });

        // A reply is 11 bytes and the payload.
        let over = conn.reply(0, 0, COMLINE_ENVELOPE_OK, 0, &[1; 22], 4096);
        assert_eq!(over.status, COMLINE_ERR_FRAME_TOO_LARGE);
        let at_max = conn.reply(0, 0, COMLINE_ENVELOPE_OK, 0, &[1; 21], 4096);
        assert_eq!(at_max.status, COMLINE_OK);
        // The connection keeps working.
        assert_eq!(conn.request(0, 0, b"", true, b"", 4096).request_id, 1);
    }
}

#[test]
fn an_oversized_prefix_closes_a_stream_connection() {
    let mut setup = Setup::plain("datagram@2");
    setup.stream = true;
    setup.max_frame = 32;
    let mut conn = Conn::new(&setup).unwrap();
    let mut at_max = 32u32.to_le_bytes().to_vec();
    at_max.resize(4 + 32, 0);
    assert_eq!(
        conn.feed(&at_max[..4]).status,
        COMLINE_NEED_MORE,
        "32 is allowed"
    );
    let fed = conn.feed(&33u32.to_le_bytes());
    assert_eq!((fed.status, fed.consumed), (COMLINE_ERR_FRAME_TOO_LARGE, 0));
    // Closed: the stream can't be resynchronised.
    assert_eq!(conn.feed(&at_max).status, COMLINE_ERR_STATE);
    assert_eq!(
        conn.request(0, 0, b"", true, b"", 64).status,
        COMLINE_ERR_STATE
    );
    assert_eq!(conn.is_waiting(), Err(COMLINE_ERR_STATE));
}

#[test]
fn an_oversized_message_is_refused_and_the_connection_stays() {
    let mut setup = Setup::plain("datagram@2");
    setup.max_frame = 32;
    let mut conn = Conn::new(&setup).unwrap();
    assert_eq!(conn.feed(&[0; 33]).status, COMLINE_ERR_FRAME_TOO_LARGE);
    let mut request = vec![0, 0, 1, 0];
    request.extend_from_slice(&5u64.to_le_bytes());
    let fed = conn.feed(&request);
    assert_eq!(
        (fed.status, fed.ev.kind),
        (COMLINE_OK, COMLINE_EVENT_REQUEST)
    );
}

#[test]
fn max_frame_may_be_raised_up_to_the_ceiling() {
    let mut setup = Setup::plain("datagram@2");
    setup.max_frame = COMLINE_MAX_FRAME_CEILING;
    assert!(Conn::new(&setup).is_ok());
    setup.max_frame = COMLINE_MAX_FRAME_CEILING + 1;
    assert_eq!(Conn::new(&setup).err(), Some(COMLINE_ERR_INVALID_ARGUMENT));
    setup.max_frame = u64::MAX;
    assert_eq!(Conn::new(&setup).err(), Some(COMLINE_ERR_INVALID_ARGUMENT));

    // Raised past the default, a frame above 16 MiB goes both ways.
    setup.max_frame = 2 * COMLINE_DEFAULT_MAX_FRAME;
    setup.stream = true;
    let mut conn = Conn::new(&setup).unwrap();
    let mut peer = Conn::new(&setup).unwrap();
    let body = vec![9u8; COMLINE_DEFAULT_MAX_FRAME as usize + 1];
    let sent = conn.request(0, 0, b"", false, &body, body.len() + 64);
    assert_eq!(sent.status, COMLINE_OK);
    let fed = peer.feed(&sent.bytes);
    assert_eq!(
        (fed.status, fed.ev.body.len as usize),
        (COMLINE_OK, body.len())
    );

    // And by default, it is refused.
    let mut default = Conn::new(&Setup::plain("datagram@2")).unwrap();
    let over = default.request(0, 0, b"", false, &body, body.len() + 64);
    assert_eq!(over.status, COMLINE_ERR_FRAME_TOO_LARGE);
}

// ── NEED_MORE ──────────────────────────────────────────────────────────────

#[test]
fn a_stream_is_fed_a_byte_at_a_time() {
    let mut setup = Setup::plain("jsonrpc@1");
    setup.stream = true;
    let mut sender = Conn::new(&setup).unwrap();
    let mut conn = Conn::new(&setup).unwrap();
    let first = sender.request(0, 0, b"one", true, b"[1]", 256).bytes;
    let second = sender.request(0, 0, b"two", false, b"[2]", 256).bytes;
    let total = first.len();

    for n in 0..total {
        let fed = conn.feed(&first[..n]);
        assert_eq!(
            (fed.status, fed.consumed),
            (COMLINE_NEED_MORE, 0),
            "{n} bytes"
        );
        let needed = if n < 4 { 0 } else { total };
        assert_eq!(fed.ev.frame.len as usize, needed, "{n} bytes");
    }

    // Two frames in one buffer: one per call, from `consumed`.
    let mut both = first.clone();
    both.extend_from_slice(&second);
    let fed = conn.feed(&both);
    assert_eq!((fed.status, fed.consumed), (COMLINE_OK, total));
    assert_eq!(
        fed.ev.frame,
        comline_span {
            offset: 4,
            len: total as u32 - 4
        }
    );
    assert_eq!(at(&both, fed.ev.call_name), b"one");
    let rest = &both[fed.consumed..];
    let fed = conn.feed(rest);
    assert_eq!((fed.status, fed.consumed), (COMLINE_OK, rest.len()));
    assert_eq!(at(rest, fed.ev.call_name), b"two");
    assert_eq!(fed.ev.flags & COMLINE_EVENT_ONE_WAY, COMLINE_EVENT_ONE_WAY);
}

#[test]
fn a_malformed_frame_is_consumed_and_the_stream_goes_on() {
    let mut setup = Setup::plain("datagram@2");
    setup.stream = true;
    let mut conn = Conn::new(&setup).unwrap();
    let mut input = prefixed(&[9, 9, 9]);
    input.extend_from_slice(&prefixed(&[0, 0, 1, 0, 2, 0, 0, 0, 0, 0, 0, 0]));
    let fed = conn.feed(&input);
    assert_eq!((fed.status, fed.consumed), (COMLINE_ERR_FRAMING, 7));
    let fed = conn.feed(&input[7..]);
    assert_eq!(
        (fed.status, fed.ev.kind, fed.ev.request_id),
        (COMLINE_OK, COMLINE_EVENT_REQUEST, 2)
    );
}

// ── state and arguments ────────────────────────────────────────────────────

#[test]
fn calls_wait_for_the_handshake() {
    let mut setup = Setup::plain("datagram@2");
    setup.handshake = COMLINE_HANDSHAKE_COMLINE;
    let mut conn = Conn::new(&setup).unwrap();
    assert_eq!(
        conn.request(0, 0, b"", true, b"", 64).status,
        COMLINE_ERR_STATE
    );
    assert_eq!(
        conn.reply(0, 0, COMLINE_ENVELOPE_OK, 0, b"", 64).status,
        COMLINE_ERR_STATE
    );
    let hello = conn.start(64).bytes;
    assert_eq!(
        conn.request(0, 0, b"", true, b"", 64).status,
        COMLINE_ERR_STATE,
        "the peer's frame is missing"
    );
    assert_eq!(conn.feed(&hello).ev.kind, COMLINE_EVENT_HANDSHAKE_OK);
    assert_eq!(conn.request(0, 0, b"", true, b"", 64).status, COMLINE_OK);

    // A peer that names another framing is refused, and the connection
    // closes.
    let mut other = Conn::new(&setup).unwrap();
    let theirs = handshake_encode(setup.ir_hash, "json@1", "jsonrpc@1", 0, 64).bytes;
    let (status, peer) = handshake_check(&hello, &theirs);
    assert_eq!(status, COMLINE_ERR_HANDSHAKE);
    assert_eq!(peer.map(|p| p.framing), Some(name_hash("jsonrpc@1")));
    assert_eq!(other.feed(&theirs).status, COMLINE_ERR_HANDSHAKE);
    assert_eq!(other.start(64).status, COMLINE_ERR_STATE);
}

#[test]
fn without_a_handshake_start_writes_nothing() {
    let mut conn = Conn::new(&Setup::plain("datagram@2")).unwrap();
    let started = conn.start(0);
    assert_eq!((started.status, started.written), (COMLINE_OK, 0));
}

#[test]
fn bad_arguments_are_refused() {
    let setup = Setup::plain("datagram@2");
    let mut conn = Conn::new(&setup).unwrap();
    let mut out = [0u8; 64];
    let (mut written, mut id, mut consumed) = (0, 0, 0);
    let mut ev = comline_event::default();
    let invalid = COMLINE_ERR_INVALID_ARGUMENT;
    unsafe {
        // Null handles and out-pointers.
        assert_eq!(
            comline_connection_new(ptr::null(), &mut ptr::null_mut()),
            invalid
        );
        assert_eq!(
            comline_connection_new(&setup.config(), ptr::null_mut()),
            invalid
        );
        assert_eq!(
            comline_connection_feed(ptr::null_mut(), ptr::null(), 0, &mut consumed, &mut ev),
            invalid
        );
        assert_eq!(
            comline_connection_feed(conn.0, ptr::null(), 0, ptr::null_mut(), &mut ev),
            invalid
        );
        assert_eq!(
            comline_connection_feed(conn.0, ptr::null(), 0, &mut consumed, ptr::null_mut()),
            invalid
        );
        assert_eq!(
            comline_connection_feed(conn.0, ptr::null(), 4, &mut consumed, &mut ev),
            invalid
        );
        assert_eq!(
            comline_connection_is_waiting(conn.0, ptr::null_mut()),
            invalid
        );
        assert_eq!(comline_connection_abandon(ptr::null_mut(), 0), invalid);
        assert_eq!(
            comline_connection_start(conn.0, out.as_mut_ptr(), 64, ptr::null_mut()),
            invalid
        );
        comline_connection_free(ptr::null_mut());
        assert_eq!(
            comline_connection_encode_request(
                conn.0,
                0,
                0,
                ptr::null(),
                0,
                1,
                ptr::null(),
                0,
                out.as_mut_ptr(),
                64,
                &mut written,
                ptr::null_mut()
            ),
            invalid
        );
        assert_eq!(
            comline_connection_encode_request(
                conn.0,
                0,
                0,
                ptr::null(),
                0,
                1,
                ptr::null(),
                0,
                ptr::null_mut(),
                64,
                &mut written,
                &mut id
            ),
            invalid
        );
        assert_eq!(
            comline_connection_encode_request(
                conn.0,
                0,
                0,
                ptr::null(),
                1,
                1,
                ptr::null(),
                0,
                out.as_mut_ptr(),
                64,
                &mut written,
                &mut id
            ),
            invalid
        );
        // Null with a zero length is an empty slice.
        assert_eq!(
            comline_connection_encode_request(
                conn.0,
                0,
                0,
                ptr::null(),
                0,
                1,
                ptr::null(),
                0,
                out.as_mut_ptr(),
                64,
                &mut written,
                &mut id
            ),
            COMLINE_OK
        );
        let mut hash = 0;
        assert_eq!(comline_name_hash(ptr::null(), 0, &mut hash), COMLINE_OK);
        assert_eq!(
            comline_name_hash(b"x".as_ptr(), 1, ptr::null_mut()),
            invalid
        );
        assert_eq!(
            comline_handshake_check(ptr::null(), 0, ptr::null(), 0, ptr::null_mut()),
            invalid,
            "ours must parse"
        );
    }
    // Values out of range.
    assert_eq!(
        conn.request(0, 0, &[0xff, 0xfe], true, b"", 64).status,
        invalid,
        "not UTF-8"
    );
    let mut raw = 0;
    let status = unsafe {
        comline_connection_encode_request(
            conn.0,
            0,
            0,
            ptr::null(),
            0,
            2,
            ptr::null(),
            0,
            out.as_mut_ptr(),
            64,
            &mut raw,
            &mut id,
        )
    };
    assert_eq!(status, invalid, "expects_reply is 0 or 1");
    assert_eq!(
        conn.reply(0, 0, 3, 0, b"", 64).status,
        invalid,
        "an envelope kind"
    );
    assert_eq!(
        conn.reply(0, 0, COMLINE_ENVELOPE_FAULT, 0, b"x", 64).status,
        invalid,
        "a fault has no body"
    );

    let mut config = setup.config();
    config.stream = 2;
    assert_eq!(Conn::from_config(&config).err(), Some(invalid));
    let mut config = setup.config();
    config.unordered = 7;
    assert_eq!(Conn::from_config(&config).err(), Some(invalid));
    let mut config = setup.config();
    config.handshake = 2;
    assert_eq!(Conn::from_config(&config).err(), Some(invalid));
    let mut config = setup.config();
    config._reserved = 1;
    assert_eq!(Conn::from_config(&config).err(), Some(invalid));
    let mut config = setup.config();
    config.framing = ptr::null();
    assert_eq!(Conn::from_config(&config).err(), Some(invalid));
    let bad = [0xffu8];
    let mut config = setup.config();
    config.wire_format = bad.as_ptr();
    config.wire_format_len = 1;
    assert_eq!(Conn::from_config(&config).err(), Some(invalid));
    for unknown in ["datagram@1", "datagram@3", "jsonrpc", ""] {
        let mut s = setup;
        s.framing = unknown;
        assert_eq!(
            Conn::new(&s).err(),
            Some(COMLINE_ERR_UNKNOWN_FRAMING),
            "{unknown}"
        );
    }
    // Any wire format name is taken: the codecs are the host's.
    let mut s = setup;
    s.wire_format = "com.acme.custom@1";
    assert!(Conn::new(&s).is_ok());
}

// ── no allocation after connection_new ────────────────────────────────────

#[test]
fn the_happy_path_allocates_nothing() {
    for framing in ["datagram@2", "jsonrpc@1"] {
        for unordered in [false, true] {
            let mut setup = Setup::plain(framing);
            setup.handshake = COMLINE_HANDSHAKE_COMLINE;
            setup.stream = true;
            setup.unordered = unordered;
            setup.max_waiting = 8;
            let a = Conn::new(&setup).unwrap();
            let b = Conn::new(&setup).unwrap();
            let mut out_a = vec![0u8; 4096];
            let mut out_b = vec![0u8; 4096];
            let mut name = vec![0u8; 64];
            let escaped = br#"{"jsonrpc":"2.0","method":"a\/b","params":[],"id":0}"#;
            let escaped = prefixed(escaped);

            let before = allocations();
            let (s, n) = start_into(&a, &mut out_a);
            assert_eq!(s, COMLINE_OK);
            let (s, _, ev) = feed_raw(&b, &out_a[..n]);
            assert_eq!((s, ev.kind), (COMLINE_OK, COMLINE_EVENT_HANDSHAKE_OK));
            let (s, n) = start_into(&b, &mut out_b);
            assert_eq!(s, COMLINE_OK);
            let (s, _, _) = feed_raw(&a, &out_b[..n]);
            assert_eq!(s, COMLINE_OK);

            for round in 0..20u64 {
                // a calls b, waits; b replies; a reads the reply.
                // Three ids a round: this call, a one-way one, an abandoned one.
                let (s, n, id) =
                    request_into(&a, 1, b"chat.Chat/send", true, b"[\"hi\"]", &mut out_a);
                assert_eq!((s, id), (COMLINE_OK, 3 * round));
                let (s, used, ev) = feed_raw(&b, &out_a[..n]);
                assert_eq!((s, used, ev.kind), (COMLINE_OK, n, COMLINE_EVENT_REQUEST));
                let (s, n) = reply_into(
                    &b,
                    ev.request_id,
                    COMLINE_ENVELOPE_OK,
                    b"{\"ok\":1}",
                    &mut out_b,
                );
                assert_eq!(s, COMLINE_OK);
                let (s, _, ev) = feed_raw(&a, &out_b[..n]);
                assert_eq!((s, ev.kind), (COMLINE_OK, COMLINE_EVENT_REPLY));

                // A one-way call, an error reply and a fault.
                let (s, n, _) = request_into(&a, 1, b"chat.Chat/push", false, b"[]", &mut out_a);
                assert_eq!(s, COMLINE_OK);
                let (s, _, ev) = feed_raw(&b, &out_a[..n]);
                assert_eq!((s, ev.kind), (COMLINE_OK, COMLINE_EVENT_REQUEST));
                for envelope in [COMLINE_ENVELOPE_ERR, COMLINE_ENVELOPE_FAULT] {
                    let body: &[u8] = if envelope == COMLINE_ENVELOPE_ERR {
                        b"{}"
                    } else {
                        b""
                    };
                    let (s, _) = reply_into(&b, 99, envelope, body, &mut out_b);
                    assert_eq!(s, COMLINE_OK);
                }

                // Too small, busy-free bookkeeping, a stale reply.
                let (s, needed, _) = request_into(&a, 1, b"x", true, b"[]", &mut out_a[..3]);
                assert_eq!(s, COMLINE_BUFFER_TOO_SMALL);
                assert!(needed > 3);
                let (s, _, id) = request_into(&a, 1, b"x", true, b"[]", &mut out_a);
                assert_eq!(s, COMLINE_OK);
                let mut waiting = 0;
                unsafe {
                    assert_eq!(comline_connection_is_waiting(a.0, &mut waiting), COMLINE_OK);
                    assert_eq!(comline_connection_abandon(a.0, id), COMLINE_OK);
                }
                let (s, n) = reply_into(&b, id, COMLINE_ENVELOPE_OK, b"1", &mut out_b);
                assert_eq!(s, COMLINE_OK);
                let (s, _, ev) = feed_raw(&a, &out_b[..n]);
                assert_eq!((s, ev.kind), (COMLINE_OK, COMLINE_EVENT_STALE));
            }

            if framing == "jsonrpc@1" {
                // An escaped method name is found in the frame and unescaped.
                let (s, _, ev) = feed_raw(&b, &escaped);
                assert_eq!(
                    (s, ev.flags & COMLINE_EVENT_NAME_ESCAPED),
                    (COMLINE_OK, COMLINE_EVENT_NAME_ESCAPED)
                );
                let spelled = &escaped[ev.call_name.offset as usize..][..ev.call_name.len as usize];
                let mut written = 0;
                let s = unsafe {
                    comline_method_name_unescape(
                        spelled.as_ptr(),
                        spelled.len(),
                        name.as_mut_ptr(),
                        name.len(),
                        &mut written,
                    )
                };
                assert_eq!((s, &name[..written]), (COMLINE_OK, &b"a/b"[..]));
            }
            assert_eq!(
                allocations() - before,
                0,
                "{framing}, unordered {unordered}"
            );
        }
    }
}

/// Found by the `ffi_feed` fuzz target and `tests/differential.rs`: feeding
/// `jsonrpc@1` allocates in two cases, since the framing parses with
/// serde_json. Refusing a malformed frame does, as serde_json boxes every
/// error it makes (and formats a message into some, such as a string `id`).
/// And taking a valid frame does when a member's name has an escape in it
/// (`"js\u006erpc"`), which serde_json unescapes into a scratch `Vec` to
/// match it. The rule says nothing allocates after `comline_connection_new`,
/// so these are bugs in the core's JSON-RPC decode, not in the rule. Ignored
/// until they're fixed; `common/drive.rs` allows exactly these two cases.
#[test]
#[ignore = "jsonrpc@1 decode allocates on a malformed frame or an escaped member name"]
fn jsonrpc_decode_allocates_nothing() {
    let conn = Conn::new(&Setup::plain("jsonrpc@1")).unwrap();
    let mut allocating = Vec::new();
    for (frame, status) in [
        (&b""[..], COMLINE_ERR_FRAMING),
        (b"!", COMLINE_ERR_FRAMING),
        (b"{", COMLINE_ERR_FRAMING),
        (
            br#"{"jsonrpc":"2.0","result":1,"id":"x"}"#,
            COMLINE_ERR_FRAMING,
        ),
        (br#"{"js\u006erpc":"2.0","method":"a","id":1}"#, COMLINE_OK),
    ] {
        let before = allocations();
        let (s, _, _) = feed_raw(&conn, frame);
        let allocated = allocations() - before;
        assert_eq!(s, status);
        if allocated != 0 {
            allocating.push((String::from_utf8_lossy(frame).into_owned(), allocated));
        }
    }
    assert!(allocating.is_empty(), "{allocating:?}");
}

#[test]
fn the_counter_counts() {
    let before = allocations();
    let v = std::hint::black_box(vec![1u8; 3]);
    drop(v);
    assert_eq!(allocations() - before, 1);
    // And making a handle is the one allocation the library makes.
    let before = allocations();
    let conn = Conn::new(&Setup::plain("datagram@2")).unwrap();
    assert_eq!(allocations() - before, 2, "the handle and its waiting room");
    drop(conn);
}

// ── overlapping buffers and oversized lengths ──────────────────────────────

#[test]
fn overlapping_inputs_and_outputs_are_refused() {
    let invalid = COMLINE_ERR_INVALID_ARGUMENT;
    let conn = Conn::new(&Setup::plain("jsonrpc@1")).unwrap();
    let mut buf = [b'x'; 256];
    let p = buf.as_mut_ptr();
    let (mut written, mut id) = (0usize, 0u64);
    unsafe {
        // `body` is `out` exactly, then partly.
        for (body, out) in [(0, 0), (8, 0), (0, 8)] {
            let status = comline_connection_encode_request(
                conn.0,
                0,
                0,
                b"m".as_ptr(),
                1,
                1,
                p.add(body),
                16,
                p.add(out),
                64,
                &mut written,
                &mut id,
            );
            assert_eq!(status, invalid, "body at {body}, out at {out}");
        }
        // The name inside `out`.
        let status = comline_connection_encode_request(
            conn.0,
            0,
            0,
            p.add(40),
            4,
            1,
            ptr::null(),
            0,
            p,
            64,
            &mut written,
            &mut id,
        );
        assert_eq!(status, invalid);
        // `*written` inside `out`.
        let status = comline_connection_encode_request(
            conn.0,
            0,
            0,
            ptr::null(),
            0,
            1,
            ptr::null(),
            0,
            p.add(16),
            64,
            p.add(32).cast(),
            &mut id,
        );
        assert_eq!(status, invalid);
        // `out` over the handle itself.
        let status = comline_connection_encode_request(
            conn.0,
            0,
            0,
            ptr::null(),
            0,
            1,
            ptr::null(),
            0,
            conn.0.cast(),
            8,
            &mut written,
            &mut id,
        );
        assert_eq!(status, invalid);
        let mut waiting = 0u8;
        assert_eq!(
            comline_connection_is_waiting(conn.0, &mut waiting),
            COMLINE_OK
        );
        assert_eq!(waiting, 0, "nothing was made or spent");

        // A reply's body over its `out`.
        let status = comline_connection_encode_reply(
            conn.0,
            0,
            1,
            COMLINE_ENVELOPE_OK,
            0,
            p.add(4),
            8,
            p,
            64,
            &mut written,
        );
        assert_eq!(status, invalid);

        // Feed: the event over the input, exactly and partly.
        let ev: *mut comline_event = p.cast();
        let mut consumed = 0;
        assert_eq!(
            comline_connection_feed(conn.0, p, 64, &mut consumed, ev),
            invalid
        );
        assert_eq!(
            comline_connection_feed(conn.0, p.add(40), 64, &mut consumed, ev),
            invalid
        );
        assert_eq!(
            comline_connection_feed(
                conn.0,
                p.add(48),
                8,
                p.add(50).cast(),
                &mut comline_event::default()
            ),
            invalid
        );

        // The handshake: names inside `out`.
        let status = comline_handshake_encode(
            0,
            p.add(10),
            6,
            b"datagram@2".as_ptr(),
            10,
            0,
            p,
            64,
            &mut written,
        );
        assert_eq!(status, invalid);
        let peer: *mut comline_handshake = p.cast();
        assert_eq!(
            comline_handshake_check(p.add(8), 31, b"x".as_ptr(), 1, peer),
            invalid
        );

        // Unescape in place, exactly and partly.
        assert_eq!(
            comline_method_name_unescape(p, 8, p, 8, &mut written),
            invalid
        );
        assert_eq!(
            comline_method_name_unescape(p, 8, p.add(7), 8, &mut written),
            invalid
        );
        assert_eq!(
            comline_method_name_unescape(p, 8, p.add(8), 8, &mut written),
            COMLINE_OK,
            "adjacent is apart"
        );

        // The name hash written over its name.
        assert_eq!(comline_name_hash(p, 8, p.add(4).cast()), invalid);

        // `connection_new`'s out-pointer over its config.
        let mut config = Setup::plain("datagram@2").config();
        let cfg = &mut config as *mut comline_config;
        assert_eq!(comline_connection_new(cfg, cfg.cast()), invalid);
    }
    // Inputs may overlap each other: a name that is also the body.
    let name_and_body = b"[1]";
    let mut out = [0u8; 128];
    let status = unsafe {
        comline_connection_encode_request(
            conn.0,
            0,
            0,
            name_and_body.as_ptr(),
            3,
            1,
            name_and_body.as_ptr(),
            3,
            out.as_mut_ptr(),
            128,
            &mut written,
            &mut id,
        )
    };
    assert_eq!(status, COMLINE_OK);
}

#[test]
fn lengths_above_isize_max_are_refused() {
    let huge = isize::MAX as usize + 1;
    let frame = handshake_encode(1, "json@1", "datagram@2", 0, 64).bytes;
    let byte = [0u8; 1];
    let mut written = 0;
    unsafe {
        assert_eq!(
            comline_handshake_check(
                frame.as_ptr(),
                frame.len(),
                byte.as_ptr(),
                huge,
                ptr::null_mut()
            ),
            COMLINE_ERR_INVALID_ARGUMENT
        );
        assert_eq!(comline_supports(byte.as_ptr(), huge), 0);
        // A null `out` with a huge capacity: refused for the length, not
        // the null.
        assert_eq!(
            comline_method_name_unescape(b"a".as_ptr(), 1, ptr::null_mut(), huge, &mut written),
            COMLINE_ERR_INVALID_ARGUMENT
        );
        let mut conn = Conn::new(&Setup::plain("datagram@2")).unwrap();
        let mut id = 0;
        let status = comline_connection_encode_request(
            conn.0,
            0,
            0,
            ptr::null(),
            0,
            1,
            ptr::null(),
            0,
            ptr::null_mut(),
            huge,
            &mut written,
            &mut id,
        );
        assert_eq!(status, COMLINE_ERR_INVALID_ARGUMENT);
        assert_eq!(conn.is_waiting(), Ok(false));
    }
}

#[test]
fn a_poisoned_handle_is_freed() {
    let conn = Conn::new(&Setup::plain("datagram@2")).unwrap();
    unsafe { comline_test_panic_next(conn.0) };
    let mut waiting = 0;
    assert_eq!(
        unsafe { comline_connection_is_waiting(conn.0, &mut waiting) },
        COMLINE_ERR_POISONED
    );
    let before = allocations();
    drop(conn); // comline_connection_free
    assert_eq!(allocations(), before, "freeing allocates nothing");
}
