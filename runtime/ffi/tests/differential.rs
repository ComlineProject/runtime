// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Random sequences through a `comline_connection` and through `State` plus
//! the framings directly, which must give identical frames, events and
//! refusals; see `common/drive.rs` for what runs and what is compared. The
//! `ffi_vs_core` fuzz target runs the same comparison on fuzzer input.
//!
//! The steps mirror the R6 model test's generator
//! (`core/tests/state_model.rs`), with the same weights: local calls (made
//! with room to spare, with exactly the room reported, or short of it),
//! requests from the peer, replies to a waiting, spent or never-issued id,
//! abandons and malformed frames. On top of those: the handshake (agreeing,
//! or not), stream and message mode, frames delivered in chunks or several
//! at once, flipped bytes, replies from this side, and arguments the ABI
//! refuses.
//!
//! `PROPTEST_CASES` raises the number of sequences per property; the nightly
//! fuzz workflow runs 100000. Under Miri a few run, to check the boundary
//! for undefined behaviour on random paths.

#[path = "common/drive.rs"]
mod drive;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use comline_runtime_ffi::*;
use drive::{Cap, Config, Hello, Mode, Op, Ours, PeerEnvelope, Theirs};
use proptest::prelude::*;

// ── a counting allocator, as in `rules.rs` ─────────────────────────────────

/// The system allocator, counting this thread's allocations.
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

// ── the generator ──────────────────────────────────────────────────────────

/// A body: raw bytes under `datagram@2`, mostly a JSON text under
/// `jsonrpc@1` (and sometimes not, which it frames all the same).
fn body(json: bool) -> BoxedStrategy<Vec<u8>> {
    let raw = prop::collection::vec(any::<u8>(), 0..12);
    if json {
        prop_oneof![
            6 => prop::sample::select(&drive::JSON_TEXTS[..]).prop_map(|t| t.as_bytes().to_vec()),
            1 => Just(Vec::new()),
            1 => raw,
        ]
        .boxed()
    } else {
        raw.boxed()
    }
}

fn method() -> impl Strategy<Value = String> {
    prop::sample::select(&drive::METHODS[..]).prop_map(str::to_owned)
}

/// A name as `encode_request` takes it: mostly a method, sometimes not
/// UTF-8.
fn name() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        16 => method().prop_map(String::into_bytes),
        1 => Just(vec![b'a', 0xff]),
    ]
}

fn protocol() -> impl Strategy<Value = u16> {
    prop_oneof![0..4u16, any::<u16>()]
}

/// As `state_model.rs`'s `How`: room to spare, exactly the room needed, or
/// short of it; and a fixed room, which may be either.
fn cap() -> impl Strategy<Value = Cap> {
    prop_oneof![
        8 => Just(Cap::Plenty),
        8 => Just(Cap::Exact),
        2 => any::<u16>().prop_map(Cap::Short),
        1 => (0..64u16).prop_map(Cap::Fixed),
    ]
}

/// As `state_model.rs`'s `ReplyTo`.
fn ours() -> impl Strategy<Value = Ours> {
    prop_oneof![
        4 => any::<u16>().prop_map(Ours::Waiting),
        2 => any::<u16>().prop_map(Ours::Spent),
        1 => prop_oneof![0..1000u64, Just(u64::MAX / 2)].prop_map(Ours::Never),
    ]
}

fn envelope() -> impl Strategy<Value = PeerEnvelope> {
    prop_oneof![
        Just(PeerEnvelope::Ok),
        any::<u16>().prop_map(PeerEnvelope::Err),
        Just(PeerEnvelope::Fault),
    ]
}

/// Hand what the peer sent to `feed`: all of it (most often), a chunk, or
/// nothing yet, so frames queue up.
fn deliver() -> impl Strategy<Value = Option<Op>> {
    prop_oneof![
        6 => Just(Some(Op::Deliver(0))),
        2 => (1..40u16).prop_map(|n| Some(Op::Deliver(n))),
        1 => Just(None),
    ]
}

/// A frame no framing reads, as `state_model.rs`'s `Malformed`, written
/// raw: empty, a bad first byte, or a reply with a bad envelope or no id.
fn malformed(json: bool) -> impl Strategy<Value = Vec<u8>> {
    let bad_start =
        (any::<u8>(), prop::collection::vec(any::<u8>(), 0..12)).prop_map(move |(byte, rest)| {
            let mut frame = if json {
                // `!` starts no JSON value.
                vec![b'!', byte]
            } else {
                vec![2u8.saturating_add(byte % 254)]
            };
            frame.extend_from_slice(&rest);
            frame
        });
    let bad_reply = any::<u64>().prop_map(move |id| {
        if json {
            format!(r#"{{"jsonrpc":"2.0","result":{id}}}"#).into_bytes()
        } else {
            let mut frame = vec![1, 0];
            frame.extend_from_slice(&id.to_le_bytes());
            frame.push(0xee);
            frame
        }
    });
    prop_oneof![Just(Vec::new()), bad_start, bad_reply]
}

/// One step, as one or more ops (a frame from the peer, then its delivery).
fn step(json: bool) -> impl Strategy<Value = Vec<Op>> {
    let call = (
        protocol(),
        any::<u16>(),
        name(),
        prop::bool::weighted(0.25),
        body(json),
        cap(),
    )
        .prop_map(|(protocol, call_id, name, one_way, body, cap)| {
            vec![Op::Call {
                protocol,
                call_id,
                name,
                expects_reply: u8::from(!one_way),
                body,
                cap,
            }]
        });
    let request = (
        protocol(),
        any::<u16>(),
        method(),
        any::<u64>(),
        any::<bool>(),
        body(json),
        deliver(),
    )
        .prop_map(
            |(protocol, call_id, name, request_id, one_way, body, deliver)| {
                let mut ops = vec![Op::PeerRequest {
                    protocol,
                    call_id,
                    name,
                    request_id,
                    one_way,
                    body,
                }];
                ops.extend(deliver);
                ops
            },
        );
    let reply = (ours(), protocol(), envelope(), body(json), deliver()).prop_map(
        |(to, protocol, envelope, body, deliver)| {
            let body = if envelope == PeerEnvelope::Fault {
                Vec::new()
            } else {
                body
            };
            let mut ops = vec![Op::PeerReply {
                to,
                protocol,
                envelope,
                body,
            }];
            ops.extend(deliver);
            ops
        },
    );
    let answer = (
        protocol(),
        prop_oneof![
            4 => any::<u16>().prop_map(Theirs::Received),
            1 => any::<u64>().prop_map(Theirs::Raw),
        ],
        prop_oneof![
            8 => 0..3u8,
            1 => any::<u8>(),
        ],
        any::<u16>(),
        body(json),
        cap(),
    )
        .prop_map(|(protocol, to, envelope, error_id, body, cap)| {
            // A fault takes no body; one now and then is refused.
            let body = if envelope == COMLINE_ENVELOPE_FAULT && body.len() % 4 != 0 {
                Vec::new()
            } else {
                body
            };
            vec![Op::Answer {
                protocol,
                to,
                envelope,
                error_id,
                body,
                cap,
            }]
        });
    let malformed = (malformed(json), deliver()).prop_map(|(bytes, deliver)| {
        let mut ops = vec![Op::PeerRaw {
            framed: true,
            bytes,
        }];
        ops.extend(deliver);
        ops
    });
    prop_oneof![
        6 => call,
        2 => request,
        6 => reply,
        2 => ours().prop_map(|to| vec![Op::Abandon(to)]),
        1 => malformed,
        2 => answer,
        1 => Just(vec![Op::IsWaiting]),
        1 => Just(vec![Op::Deliver(0)]),
        1 => (any::<u16>(), any::<u8>()).prop_map(|(at, xor)| vec![Op::Corrupt { at, xor }]),
        1 => prop::collection::vec(any::<u8>(), 0..8)
            .prop_map(|bytes| vec![Op::PeerRaw { framed: false, bytes }]),
        1 => (cap(), any::<u8>()).prop_map(|(cap, n)| vec![Op::Start(cap), Op::Deliver(u16::from(n))]),
    ]
}

/// The handshake up front: both frames (in either order), most often
/// agreeing; or one of them, or none, so calls meet a connection that isn't
/// open.
fn opening() -> impl Strategy<Value = Vec<Op>> {
    let hello = prop_oneof![
        12 => Just(Hello::Agree),
        1 => Just(Hello::IrHash),
        1 => Just(Hello::WireFormat),
        1 => Just(Hello::Framing),
        1 => any::<u8>().prop_map(Hello::Short),
        1 => any::<u8>().prop_map(Hello::Trailing),
    ];
    (hello, any::<u32>(), cap(), any::<bool>(), 0..8u8).prop_map(
        |(hello, capabilities, cap, peer_first, which)| {
            let start = Op::Start(cap);
            let peer = [
                Op::PeerHello {
                    hello,
                    capabilities,
                },
                Op::Deliver(0),
            ];
            match which {
                0 => vec![],
                1 => vec![start],
                2 => peer.to_vec(),
                _ if peer_first => [&peer[..], &[start]].concat(),
                _ => [&[start][..], &peer[..]].concat(),
            }
        },
    )
}

/// A config: mostly one a connection is made from, now and then one that
/// `comline_connection_new` refuses.
fn config() -> impl Strategy<Value = Config> {
    let framing = prop_oneof![
        16 => Just(b"datagram@2".to_vec()),
        16 => Just(b"jsonrpc@1".to_vec()),
        1 => Just(b"datagram@1".to_vec()),
        1 => Just(vec![b'j', 0xff]),
    ];
    let max_frame = prop_oneof![
        8 => Just(0u64),
        2 => 31..200u64,
        1 => 0..31u64,
        1 => Just(COMLINE_MAX_FRAME_CEILING),
        1 => Just(COMLINE_MAX_FRAME_CEILING + 1),
    ];
    let max_waiting = prop_oneof![1 => 0..4u32, 2 => Just(16u32)];
    let handshake = prop_oneof![
        12 => Just(COMLINE_HANDSHAKE_COMLINE),
        8 => Just(COMLINE_HANDSHAKE_NONE),
        1 => 2..=u8::MAX,
    ];
    // Flag fields: 0 or 1, and once in a while another value or a reserved
    // byte set.
    let odd = prop_oneof![30 => Just(None), 1 => (2..=u8::MAX).prop_map(Some)];
    (
        framing,
        prop::sample::select(&drive::WIRE_FORMATS[..]),
        any::<u64>(),
        any::<u32>(),
        max_frame,
        max_waiting,
        handshake,
        any::<bool>(),
        any::<bool>(),
        odd,
    )
        .prop_map(
            |(
                framing,
                wire_format,
                ir_hash,
                capabilities,
                max_frame,
                max_waiting,
                handshake,
                stream,
                unordered,
                odd,
            )| {
                let (mut stream, mut unordered, mut reserved) =
                    (u8::from(stream), u8::from(unordered), 0);
                match odd.map(|v| (v, v % 3)) {
                    Some((v, 0)) => stream = v,
                    Some((v, 1)) => unordered = v,
                    Some((v, _)) => reserved = v,
                    None => {}
                }
                Config {
                    framing,
                    wire_format: wire_format.as_bytes().to_vec(),
                    ir_hash,
                    capabilities,
                    max_frame,
                    max_waiting,
                    handshake,
                    stream,
                    unordered,
                    reserved,
                }
            },
        )
}

/// A config and a sequence for it.
fn sequence() -> impl Strategy<Value = (Config, Vec<Op>)> {
    config().prop_flat_map(|config| {
        let json = config.framing == b"jsonrpc@1";
        let steps = prop::collection::vec(step(json), 0..64);
        (Just(config), opening(), steps).prop_map(|(config, opening, steps)| {
            let mut ops = opening;
            ops.extend(steps.into_iter().flatten());
            (config, ops)
        })
    })
}

fn proptest_config() -> ProptestConfig {
    if cfg!(miri) {
        // Miri has no file system under isolation, and is slow.
        ProptestConfig {
            cases: 4,
            failure_persistence: None,
            ..ProptestConfig::default()
        }
    } else {
        ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(proptest_config())]

    #[test]
    fn the_abi_agrees_with_the_core((config, ops) in sequence()) {
        drive::run(&config, &ops, Mode::Differential, Some(allocations))
            .map_err(TestCaseError::fail)?;
    }
}

/// The driver on [`drive::exchange`], under both framings and both modes,
/// apart from the generator.
#[test]
fn a_whole_exchange_runs() {
    for framing in ["datagram@2", "jsonrpc@1"] {
        for stream in [0, 1] {
            let config = drive::exchange_config(framing, stream);
            drive::run(
                &config,
                &drive::exchange(),
                Mode::Differential,
                Some(allocations),
            )
            .unwrap_or_else(|e| panic!("{framing}, stream {stream}: {e}"));
        }
    }
}
