// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The input of the `ffi_feed` and `ffi_vs_core` targets: a connection's
//! config, then operations on it, read one field at a time from the front
//! ([`read`]), and written back the same way for the seed corpus
//! ([`write`]). What the operations are, and what is checked as they run,
//! is [`drive`]: `runtime/ffi/tests/common/drive.rs`, which
//! `tests/differential.rs` runs on proptest's sequences.
//!
//! Integers are little-endian, and an exhausted input reads as zeros.
//!
//! **The config:**
//!
//! | field | bytes | reads as |
//! |---|---|---|
//! | framing | 1 (+) | `% 8`: 0–2 `datagram@2`, 3–5 `jsonrpc@1`, 6 `datagram@1`, 7 a length byte (`% 32`) and that many raw bytes |
//! | wire format | 1 (+) | `% 4`: `packed@1`, `json@1`, `msgpack@1`, or raw as above |
//! | flags | 1 (+4) | bit 0 handshake `none`, bit 1 stream, bit 2 unordered; bit 3: four raw bytes follow for `handshake`, `stream`, `unordered`, `_reserved` |
//! | `max_frame` | 1 (+) | `% 8`: 0 (the default), a `u16`, the ceiling, the ceiling + 1, a `u64`, 31, 30, a `u8` |
//! | `max_waiting` | 1 | `% 17` |
//! | `capabilities` | 4 | |
//! | `ir_hash` | 8 | |
//!
//! **Then operations**, until the input runs out, each a tag byte `% 16`:
//!
//! | tag | operation | then |
//! |---|---|---|
//! | 0 | `Start` | a cap |
//! | 1, 2 | `Call` | protocol `u16`, call id `u16`, a name, `expects_reply` (a byte `% 8`: 0–5 one, 6 zero, 7 the next byte as it is), a body, a cap |
//! | 3 | `Answer` | protocol `u16`, theirs, envelope (a byte `% 16`: below 12 that `% 3`, else the next byte as it is), error id `u16`, a body, a cap |
//! | 4 | `Abandon` | ours |
//! | 5 | `IsWaiting` | |
//! | 6 | `PeerHello` | a byte `% 6` (agree, ir hash, wire format, framing, short + a byte, trailing + a byte), capabilities `u32` |
//! | 7, 8 | `PeerRequest` | protocol `u16`, call id `u16`, a name, request id `u64`, one-way (bit 0), a body |
//! | 9, 10 | `PeerReply` | ours, protocol `u16`, a byte `% 3` (ok, err + `u16`, fault), a body |
//! | 11 | `PeerRaw` | framed (bit 0), a length byte, that many bytes |
//! | 12–14 | `Deliver` | a byte: how many (0: all) |
//! | 15 | `Corrupt` | at `u16`, xor `u8` |
//!
//! - **A name:** a byte; below 224 [`drive::METHODS`]` [b % 7]`, else a
//!   length byte (`% 64`) and that many raw bytes.
//! - **A body:** a byte; below 128 [`drive::JSON_TEXTS`]` [b % 6]`, else
//!   `b - 128` raw bytes.
//! - **A cap:** a byte `% 4`: plenty, exact, short + a byte, fixed + a `u16`.
//! - **Ours:** a byte `% 5`: 0–1 waiting + a byte, 2 spent + a byte, 3 never
//!   + a `u16`, 4 raw + a `u64`.
//! - **Theirs:** a byte `% 4`: 0–2 received + a byte, 3 raw + a `u64`.

#[path = "../../ffi/tests/common/drive.rs"]
pub mod drive;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use comline_runtime_ffi::{COMLINE_HANDSHAKE_NONE, COMLINE_MAX_FRAME_CEILING};
use drive::{
    Cap, Config, Hello, Op, Ours, PeerEnvelope, Theirs, JSON_TEXTS, METHODS, WIRE_FORMATS,
};

/// The system allocator, counting this thread's allocations, for a target
/// to install (`#[global_allocator]`) and pass [`allocations`] to
/// [`drive::run`]: the rule that nothing allocates after
/// `comline_connection_new`. libFuzzer's own allocations don't go through
/// it.
pub struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

// SAFETY: it forwards to `System`, and counting allocates nothing.
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

/// How many allocations this thread has made through [`Counting`].
pub fn allocations() -> usize {
    ALLOCATIONS.with(Cell::get)
}

const FRAMINGS: [&str; 3] = ["datagram@2", "jsonrpc@1", "datagram@1"];

/// A cursor over the input; past its end every byte is 0.
struct Reader<'a> {
    bytes: &'a [u8],
}

impl Reader<'_> {
    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    fn u8(&mut self) -> u8 {
        match self.bytes.split_first() {
            Some((&b, rest)) => {
                self.bytes = rest;
                b
            }
            None => 0,
        }
    }

    fn take(&mut self, n: usize) -> Vec<u8> {
        let n = n.min(self.bytes.len());
        let (taken, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        taken.to_vec()
    }

    fn le<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0; N];
        let taken = self.take(N);
        out[..taken.len()].copy_from_slice(&taken);
        out
    }

    fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.le())
    }

    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.le())
    }

    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.le())
    }

    fn name(&mut self) -> Vec<u8> {
        let tag = self.u8();
        if tag < 224 {
            METHODS[tag as usize % METHODS.len()].as_bytes().to_vec()
        } else {
            let len = self.u8() as usize % 64;
            self.take(len)
        }
    }

    fn body(&mut self) -> Vec<u8> {
        let tag = self.u8();
        if tag < 128 {
            JSON_TEXTS[tag as usize % JSON_TEXTS.len()]
                .as_bytes()
                .to_vec()
        } else {
            self.take(tag as usize - 128)
        }
    }

    fn cap(&mut self) -> Cap {
        match self.u8() % 4 {
            0 => Cap::Plenty,
            1 => Cap::Exact,
            2 => Cap::Short(u16::from(self.u8())),
            _ => Cap::Fixed(self.u16()),
        }
    }

    fn ours(&mut self) -> Ours {
        match self.u8() % 5 {
            0 | 1 => Ours::Waiting(u16::from(self.u8())),
            2 => Ours::Spent(u16::from(self.u8())),
            3 => Ours::Never(u64::from(self.u16())),
            _ => Ours::Raw(self.u64()),
        }
    }

    fn theirs(&mut self) -> Theirs {
        match self.u8() % 4 {
            3 => Theirs::Raw(self.u64()),
            _ => Theirs::Received(u16::from(self.u8())),
        }
    }

    fn config(&mut self) -> Config {
        let framing = match self.u8() % 8 {
            0..=2 => FRAMINGS[0].as_bytes().to_vec(),
            3..=5 => FRAMINGS[1].as_bytes().to_vec(),
            6 => FRAMINGS[2].as_bytes().to_vec(),
            _ => {
                let len = self.u8() as usize % 32;
                self.take(len)
            }
        };
        let wire_format = match self.u8() % 4 {
            3 => {
                let len = self.u8() as usize % 32;
                self.take(len)
            }
            i => WIRE_FORMATS[i as usize].as_bytes().to_vec(),
        };
        let flags = self.u8();
        let (mut handshake, mut stream, mut unordered, mut reserved) =
            (flags & 1, (flags >> 1) & 1, (flags >> 2) & 1, 0);
        if flags & 8 != 0 {
            [handshake, stream, unordered, reserved] = self.le();
        }
        let max_frame = match self.u8() % 8 {
            0 => 0,
            1 => u64::from(self.u16()),
            2 => COMLINE_MAX_FRAME_CEILING,
            3 => COMLINE_MAX_FRAME_CEILING + 1,
            4 => self.u64(),
            5 => 31,
            6 => 30,
            _ => u64::from(self.u8()),
        };
        Config {
            framing,
            wire_format,
            max_waiting: u32::from(self.u8() % 17),
            capabilities: self.u32(),
            ir_hash: self.u64(),
            max_frame,
            handshake,
            stream,
            unordered,
            reserved,
        }
    }

    fn op(&mut self) -> Op {
        match self.u8() % 16 {
            0 => Op::Start(self.cap()),
            1 | 2 => Op::Call {
                protocol: self.u16(),
                call_id: self.u16(),
                name: self.name(),
                expects_reply: match self.u8() % 8 {
                    0..=5 => 1,
                    6 => 0,
                    _ => self.u8(),
                },
                body: self.body(),
                cap: self.cap(),
            },
            3 => Op::Answer {
                protocol: self.u16(),
                to: self.theirs(),
                envelope: match self.u8() % 16 {
                    e @ 0..=11 => e % 3,
                    _ => self.u8(),
                },
                error_id: self.u16(),
                body: self.body(),
                cap: self.cap(),
            },
            4 => Op::Abandon(self.ours()),
            5 => Op::IsWaiting,
            6 => Op::PeerHello {
                hello: match self.u8() % 6 {
                    0 => Hello::Agree,
                    1 => Hello::IrHash,
                    2 => Hello::WireFormat,
                    3 => Hello::Framing,
                    4 => Hello::Short(self.u8()),
                    _ => Hello::Trailing(self.u8()),
                },
                capabilities: self.u32(),
            },
            7 | 8 => Op::PeerRequest {
                protocol: self.u16(),
                call_id: self.u16(),
                name: String::from_utf8_lossy(&self.name()).into_owned(),
                request_id: self.u64(),
                one_way: self.u8() & 1 == 1,
                body: self.body(),
            },
            9 | 10 => Op::PeerReply {
                to: self.ours(),
                protocol: self.u16(),
                envelope: match self.u8() % 3 {
                    0 => PeerEnvelope::Ok,
                    1 => PeerEnvelope::Err(self.u16()),
                    _ => PeerEnvelope::Fault,
                },
                body: self.body(),
            },
            11 => Op::PeerRaw {
                framed: self.u8() & 1 == 1,
                bytes: {
                    let len = self.u8() as usize;
                    self.take(len)
                },
            },
            12..=14 => Op::Deliver(u16::from(self.u8())),
            _ => Op::Corrupt {
                at: self.u16(),
                xor: self.u8(),
            },
        }
    }
}

/// The config and operations at the front of `input`.
pub fn read(input: &[u8]) -> (Config, Vec<Op>) {
    let mut r = Reader { bytes: input };
    let config = r.config();
    let mut ops = Vec::new();
    while !r.is_empty() {
        ops.push(r.op());
    }
    (config, ops)
}

// ── writing, for the seed corpus ───────────────────────────────────────────

fn index_of(pool: &[&str], bytes: &[u8]) -> Option<u8> {
    pool.iter()
        .position(|p| p.as_bytes() == bytes)
        .map(|i| i as u8)
}

fn write_raw(bytes: &[u8], max: usize, out: &mut Vec<u8>) {
    let bytes = &bytes[..bytes.len().min(max - 1)];
    out.push(bytes.len() as u8);
    out.extend_from_slice(bytes);
}

fn write_name(name: &[u8], out: &mut Vec<u8>) {
    match index_of(&METHODS, name) {
        Some(i) => out.push(i),
        None => {
            out.push(224);
            write_raw(name, 64, out);
        }
    }
}

fn write_body(body: &[u8], out: &mut Vec<u8>) {
    match index_of(&JSON_TEXTS, body) {
        Some(i) => out.push(i),
        None => {
            let body = &body[..body.len().min(127)];
            out.push(128 + body.len() as u8);
            out.extend_from_slice(body);
        }
    }
}

fn write_cap(cap: Cap, out: &mut Vec<u8>) {
    match cap {
        Cap::Plenty => out.push(0),
        Cap::Exact => out.push(1),
        Cap::Short(n) => out.extend([2, n.min(255) as u8]),
        Cap::Fixed(n) => {
            out.push(3);
            out.extend(n.to_le_bytes());
        }
    }
}

fn write_ours(to: Ours, out: &mut Vec<u8>) {
    match to {
        Ours::Waiting(i) => out.extend([0, i.min(255) as u8]),
        Ours::Spent(i) => out.extend([2, i.min(255) as u8]),
        Ours::Never(n) => {
            out.push(3);
            out.extend((n.min(u64::from(u16::MAX)) as u16).to_le_bytes());
        }
        Ours::Raw(id) => {
            out.push(4);
            out.extend(id.to_le_bytes());
        }
    }
}

/// The bytes [`read`] reads back as `config` and `ops`, as near as the
/// grammar allows: raw names and bodies past their length limits are cut,
/// and indices past a byte are capped.
pub fn write(config: &Config, ops: &[Op]) -> Vec<u8> {
    let mut out = Vec::new();
    match index_of(&FRAMINGS, &config.framing) {
        Some(i) => out.push(i * 3),
        None => {
            out.push(7);
            write_raw(&config.framing, 32, &mut out);
        }
    }
    match index_of(&WIRE_FORMATS, &config.wire_format) {
        Some(i) => out.push(i),
        None => {
            out.push(3);
            write_raw(&config.wire_format, 32, &mut out);
        }
    }
    let plain = config.reserved == 0
        && config.handshake <= COMLINE_HANDSHAKE_NONE
        && config.stream <= 1
        && config.unordered <= 1;
    if plain {
        out.push(config.handshake | config.stream << 1 | config.unordered << 2);
    } else {
        out.extend([
            8,
            config.handshake,
            config.stream,
            config.unordered,
            config.reserved,
        ]);
    }
    match config.max_frame {
        0 => out.push(0),
        COMLINE_MAX_FRAME_CEILING => out.push(2),
        n if n == COMLINE_MAX_FRAME_CEILING + 1 => out.push(3),
        n => {
            out.push(4);
            out.extend(n.to_le_bytes());
        }
    }
    out.push(config.max_waiting.min(16) as u8);
    out.extend(config.capabilities.to_le_bytes());
    out.extend(config.ir_hash.to_le_bytes());

    for op in ops {
        match op {
            Op::Start(cap) => {
                out.push(0);
                write_cap(*cap, &mut out);
            }
            Op::Call {
                protocol,
                call_id,
                name,
                expects_reply,
                body,
                cap,
            } => {
                out.push(1);
                out.extend(protocol.to_le_bytes());
                out.extend(call_id.to_le_bytes());
                write_name(name, &mut out);
                match expects_reply {
                    1 => out.push(0),
                    0 => out.push(6),
                    other => out.extend([7, *other]),
                }
                write_body(body, &mut out);
                write_cap(*cap, &mut out);
            }
            Op::Answer {
                protocol,
                to,
                envelope,
                error_id,
                body,
                cap,
            } => {
                out.push(3);
                out.extend(protocol.to_le_bytes());
                match to {
                    Theirs::Received(i) => out.extend([0, (*i).min(255) as u8]),
                    Theirs::Raw(id) => {
                        out.push(3);
                        out.extend(id.to_le_bytes());
                    }
                }
                match envelope {
                    e @ 0..=2 => out.push(*e),
                    other => out.extend([12, *other]),
                }
                out.extend(error_id.to_le_bytes());
                write_body(body, &mut out);
                write_cap(*cap, &mut out);
            }
            Op::Abandon(to) => {
                out.push(4);
                write_ours(*to, &mut out);
            }
            Op::IsWaiting => out.push(5),
            Op::PeerHello {
                hello,
                capabilities,
            } => {
                out.push(6);
                match hello {
                    Hello::Agree => out.push(0),
                    Hello::IrHash => out.push(1),
                    Hello::WireFormat => out.push(2),
                    Hello::Framing => out.push(3),
                    Hello::Short(n) => out.extend([4, *n]),
                    Hello::Trailing(n) => out.extend([5, *n]),
                }
                out.extend(capabilities.to_le_bytes());
            }
            Op::PeerRequest {
                protocol,
                call_id,
                name,
                request_id,
                one_way,
                body,
            } => {
                out.push(7);
                out.extend(protocol.to_le_bytes());
                out.extend(call_id.to_le_bytes());
                write_name(name.as_bytes(), &mut out);
                out.extend(request_id.to_le_bytes());
                out.push(u8::from(*one_way));
                write_body(body, &mut out);
            }
            Op::PeerReply {
                to,
                protocol,
                envelope,
                body,
            } => {
                out.push(9);
                write_ours(*to, &mut out);
                out.extend(protocol.to_le_bytes());
                match envelope {
                    PeerEnvelope::Ok => out.push(0),
                    PeerEnvelope::Err(id) => {
                        out.push(1);
                        out.extend(id.to_le_bytes());
                    }
                    PeerEnvelope::Fault => out.push(2),
                }
                write_body(body, &mut out);
            }
            Op::PeerRaw { framed, bytes } => {
                out.extend([11, u8::from(*framed)]);
                write_raw(bytes, 256, &mut out);
            }
            Op::Deliver(n) => out.extend([12, (*n).min(255) as u8]),
            Op::Corrupt { at, xor } => {
                out.push(15);
                out.extend(at.to_le_bytes());
                out.push(*xor);
            }
        }
    }
    out
}
