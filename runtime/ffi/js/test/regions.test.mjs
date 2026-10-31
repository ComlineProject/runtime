// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// The memory regions: the module keeps its side of the promise the glue's
// cached regions rest on (`mod wasm` in `src/lib.rs`), and the glue then
// enters the module once per call.
//
//     node --test runtime/ffi/js/test/regions.test.mjs
//
// `COMLINE_WASM` is the module; it defaults to the `wasm` profile's wasm32
// build under `CARGO_TARGET_DIR` (or the repository's `target`).

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Comline, Envelope, EventKind, Handshake, Status } from "../comline.mjs";

const ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const TARGET = process.env.CARGO_TARGET_DIR || join(ROOT, "target");
const WASM =
  process.env.COMLINE_WASM ||
  join(TARGET, "wasm32-unknown-unknown", "wasm", "comline_runtime_ffi.wasm");

const module = new WebAssembly.Module(readFileSync(WASM));
const utf8 = new TextEncoder();

test("a region moves or changes only when a reserve asks it for more", () => {
  const x = new WebAssembly.Instance(module, {}).exports;
  const IN = 4096;
  const OUT = 1 << 16;
  const inPtr = x.comline_wasm_reserve_in(IN) >>> 0;
  const outPtr = x.comline_wasm_reserve_out(OUT) >>> 0;
  assert.ok(inPtr !== 0 && outPtr !== 0);
  const pattern = (i) => (i * 131 + 7) & 0xff;
  new Uint8Array(x.memory.buffer, inPtr, IN).forEach((_, i, a) => (a[i] = pattern(i)));
  const pagesBefore = x.memory.buffer.byteLength;

  // Work that allocates, frees and grows memory, all through `out`: inputs
  // in its first half, outputs in its second.
  const put = (at, bytes) => {
    new Uint8Array(x.memory.buffer).set(bytes, outPtr + at);
    return outPtr + at;
  };
  const OUTPUTS = outPtr + OUT / 2;
  const handleAt = OUTPUTS;
  const newConn = (framing, maxWaiting) => {
    const f = utf8.encode(framing);
    const fPtr = put(64, f);
    const cfg = new DataView(x.memory.buffer, put(0, new Uint8Array(48)), 48);
    cfg.setUint32(16, fPtr, true);
    cfg.setUint32(20, f.length, true);
    cfg.setUint32(36, maxWaiting, true);
    cfg.setUint8(40, Handshake.NONE);
    assert.equal(x.comline_connection_new(cfg.byteOffset, handleAt), Status.OK);
    return new DataView(x.memory.buffer).getUint32(handleAt, true);
  };
  for (let round = 0; round < 50; round++) {
    const conn = newConn("jsonrpc@1", 16);
    // `jsonrpc@1`'s decode allocates (runtime#58).
    const frame = utf8.encode(`{"jsonrpc":"2.0","method":"a\\u0062","params":[[[${round}]]],"id":${round}}`);
    const status = x.comline_connection_feed(conn, put(128, frame), frame.length, OUTPUTS, OUTPUTS + 8);
    assert.equal(status, Status.OK);
    x.comline_connection_free(conn);
    // A reserve for no more than a region holds hands back the same one.
    assert.equal(x.comline_wasm_reserve_out(OUT) >>> 0, outPtr);
    assert.equal(x.comline_wasm_reserve_out(1) >>> 0, outPtr);
  }
  // A connection big enough to grow memory.
  const big = newConn("datagram@2", 4_000_000);
  assert.ok(x.memory.buffer.byteLength > pagesBefore, "memory grew");
  x.comline_connection_free(big);

  // `in` is where it was, holding what it held.
  assert.equal(x.comline_wasm_reserve_in(IN) >>> 0, inPtr);
  assert.equal(x.comline_wasm_reserve_in(0) >>> 0, inPtr);
  const now = new Uint8Array(x.memory.buffer, inPtr, IN);
  assert.ok(now.every((b, i) => b === pattern(i)), "the in region changed");

  // Asking for more may move it, and keeps what it held.
  const moved = x.comline_wasm_reserve_in(IN * 64) >>> 0;
  assert.notEqual(moved, 0);
  const kept = new Uint8Array(x.memory.buffer, moved, IN);
  assert.ok(kept.every((b, i) => b === pattern(i)), "growing lost the contents");
  assert.equal(x.comline_wasm_reserve_out(OUT) >>> 0, outPtr, "growing in moved out");
});

/** The glue over exports that count every call into the module, and every
 * reserve. */
function counted() {
  const raw = new WebAssembly.Instance(module, {}).exports;
  const count = { calls: 0, reserves: 0 };
  const exports = {};
  for (const [name, value] of Object.entries(raw)) {
    exports[name] =
      typeof value === "function"
        ? (...args) => {
            count.calls++;
            if (name.startsWith("comline_wasm_reserve_")) count.reserves++;
            return value(...args);
          }
        : value;
  }
  return { count, comline: new Comline(exports), memory: raw.memory };
}

test("a call that fits the regions is one crossing", () => {
  const { count, comline } = counted();
  const { connection: conn } = comline.connectionNew({
    framing: "jsonrpc@1",
    handshake: Handshake.NONE,
    maxWaiting: 64,
  });
  const once = (what, call) => {
    const before = { ...count };
    const result = call();
    assert.equal(count.calls - before.calls, 1, `${what}: crossings`);
    assert.equal(count.reserves, before.reserves, `${what}: reserved`);
    return result;
  };
  for (let i = 0; i < 20; i++) {
    const sent = once("encodeRequest", () => conn.encodeRequest(0, 0, "app.Chat/send", true, "[1]", 256));
    assert.equal(sent.status, Status.OK);
    const reply = `{"jsonrpc":"2.0","result":${i},"id":${sent.requestId}}`;
    const fed = once("feed", () => conn.feed(reply));
    assert.equal(fed.event.kind, EventKind.REPLY);
    once("encodeReply", () => conn.encodeReply(0, 9n, Envelope.OK, 0, "1", 256));
    once("isWaiting", () => conn.isWaiting());
    once("abandon", () => conn.abandon(99n));
    once("nameHash", () => comline.nameHash("json@1"));
    once("supports", () => comline.supports("jsonrpc@1"));
    once("handshakeEncode", () => comline.handshakeEncode(1n, "json@1", "jsonrpc@1", 0, 64));
  }

  // A call that needs more reserves once, then fits.
  const body = "1".repeat(100_000);
  const before = count.reserves;
  const big = conn.encodeReply(0, 9n, Envelope.OK, 0, body, 100_100);
  assert.equal(big.status, Status.OK);
  assert.equal(count.reserves - before, 2, "in and out, once each");
  once("encodeReply, large again", () => conn.encodeReply(0, 9n, Envelope.OK, 0, body, 100_100));
  conn.free();
});

test("memory growing between calls doesn't disturb the glue", () => {
  const { comline } = counted();
  const { connection: conn } = comline.connectionNew({
    framing: "datagram@2",
    handshake: Handshake.NONE,
    maxWaiting: 4,
  });
  const first = conn.encodeRequest(3, 4, "", true, "abc", 64);
  // Grow memory through another connection's allocation.
  const { status, connection: big } = comline.connectionNew({
    framing: "datagram@2",
    handshake: Handshake.NONE,
    maxWaiting: 4_000_000,
  });
  assert.equal(status, Status.OK);
  const second = conn.encodeRequest(3, 4, "", true, "abc", 64);
  assert.equal(second.status, Status.OK);
  assert.equal(second.requestId, first.requestId + 1n);
  assert.deepEqual(second.bytes.subarray(12), first.bytes.subarray(12));
  const fed = conn.feed(second.bytes);
  assert.equal(fed.event.protocol, 3);
  assert.equal(fed.event.callId, 4);
  big.free();
  conn.free();
});

test("a name from the glue's cache encodes as its bytes do", () => {
  const comline = new Comline(new WebAssembly.Instance(module, {}).exports);
  const { status, connection } = comline.connectionNew({
    framing: "jsonrpc@1",
    handshake: Handshake.NONE,
  });
  assert.equal(status, Status.OK);
  try {
    // Past the cache's bound and back, twice over, so it fills and empties.
    for (let round = 0; round < 2; round++) {
      for (let i = 0; i < 600; i++) {
        const name = `method.${i % 300}.é`;
        const viaString = connection.encodeRequest(0, 0, name, false, "1", 256);
        const viaBytes = connection.encodeRequest(0, 0, utf8.encode(name), false, "1", 256);
        assert.equal(viaString.status, Status.OK);
        assert.deepEqual(viaString.bytes, viaBytes.bytes);
        assert.ok(new TextDecoder().decode(viaString.bytes).includes(`"${name}"`));
      }
    }
  } finally {
    connection.free();
  }
});

test("an encode into a caller's array is one crossing, and copies a frame the default encode would", () => {
  const { count, comline } = counted();
  const open = () =>
    comline.connectionNew({ framing: "datagram@2", handshake: Handshake.NONE, maxWaiting: 8 }).connection;
  const a = open();
  const b = open();
  const body = Uint8Array.from({ length: 4096 }, (_, i) => (i * 31 + 7) & 0xff);
  const dest = new Uint8Array(4096 + 64);
  for (let i = 0; i < 5; i++) {
    const want = a.encodeRequest(5, 0x0203, "", 0, body, 4096 + 64);
    const before = count.calls;
    const into = b.encodeRequestInto(dest, 5, 0x0203, "", 0, body);
    assert.equal(count.calls - before, 1, "crossings");
    assert.equal(into.status, Status.OK);
    assert.equal(into.written, want.written);
    assert.deepEqual(dest.subarray(0, into.written), want.bytes);
    // Fed straight from the caller's array.
    const fed = a.feed(dest.subarray(0, into.written));
    assert.equal(fed.status, Status.OK);
    assert.equal(fed.event.body.len, 4096);
  }
  a.free();
  b.free();
});

test("an encode into a caller's array gives the exact size when it's too small, and takes no id", () => {
  const comline = new Comline(new WebAssembly.Instance(module, {}).exports);
  const { connection: conn } = comline.connectionNew({
    framing: "jsonrpc@1",
    handshake: Handshake.NONE,
    maxWaiting: 8,
  });
  const want = conn.encodeRequest(0, 0, "app.Chat/send", true, "[1,2,3]", 256);
  for (const len of [0, 1, want.written - 1]) {
    const dest = new Uint8Array(len).fill(7);
    assert.deepEqual(conn.encodeRequestInto(dest, 0, 0, "app.Chat/send", true, "[1,2,3]"), {
      status: Status.BUFFER_TOO_SMALL,
      written: want.written,
    });
    assert.ok(dest.every((b) => b === 7));
  }
  const dest = new Uint8Array(want.written);
  const into = conn.encodeRequestInto(dest, 0, 0, "app.Chat/send", true, "[1,2,3]");
  assert.equal(into.status, Status.OK);
  assert.equal(into.requestId, want.requestId + 1n, "the refusals took no id");
  const reply = conn.encodeReply(0, 1n, Envelope.OK, 0, "[1]", 256);
  const short = new Uint8Array(reply.written - 1);
  assert.deepEqual(conn.encodeReplyInto(short, 0, 1n, Envelope.OK, 0, "[1]"), {
    status: Status.BUFFER_TOO_SMALL,
    written: reply.written,
  });
  conn.free();
});

test("an encode into a view of wasm memory is refused without entering the module", () => {
  const { count, comline, memory } = counted();
  const { connection: conn } = comline.connectionNew({
    framing: "datagram@2",
    handshake: Handshake.NONE,
    maxWaiting: 8,
  });
  conn.encodeRequest(0, 1, "", true, "warm", 64);
  const views = () => [
    new Uint8Array(memory.buffer),
    new Uint8Array(memory.buffer, 0, 256),
    new Uint8Array(memory.buffer, memory.buffer.byteLength - 256),
  ];
  const refuse = (dest, what) => {
    const before = count.calls;
    const snapshot = dest.slice();
    assert.deepEqual(conn.encodeRequestInto(dest, 0, 1, "", true, "x"), {
      status: Status.ERR_INVALID_ARGUMENT,
      written: 0,
    }, what);
    assert.deepEqual(conn.encodeReplyInto(dest, 0, 1n, Envelope.OK, 0, "x"), {
      status: Status.ERR_INVALID_ARGUMENT,
      written: 0,
    }, what);
    assert.equal(count.calls, before, `${what}: entered the module`);
    assert.deepEqual(dest, snapshot, `${what}: written`);
  };
  for (const view of views()) refuse(view, "a view of memory");
  // Memory grows (through another connection's allocation); a view of the
  // new buffer, taken before the glue has seen it grow, is refused too.
  const { connection: big } = comline.connectionNew({
    framing: "datagram@2",
    handshake: Handshake.NONE,
    maxWaiting: 4_000_000,
  });
  for (const view of views()) refuse(view, "a view of grown memory");
  // A refusal took no id: the next request is the one after `warm`.
  const next = conn.encodeRequest(0, 1, "", true, "x", 64);
  assert.equal(next.requestId, 1n);
  assert.throws(() => conn.encodeRequestInto(new ArrayBuffer(64), 0, 1, "", true, "x"), TypeError);
  assert.throws(() => conn.encodeRequestInto(new DataView(new ArrayBuffer(64)), 0, 1, "", true, "x"), TypeError);
  big.free();
  conn.free();
});

test("a caller's array shares nothing with wasm memory after the encode", () => {
  const comline = new Comline(new WebAssembly.Instance(module, {}).exports);
  const open = () =>
    comline.connectionNew({ framing: "datagram@2", handshake: Handshake.NONE, maxWaiting: 8 }).connection;
  const conn = open();
  const dest = new Uint8Array(256);
  const first = conn.encodeRequestInto(dest, 3, 4, "", false, "abc");
  assert.equal(first.status, Status.OK);
  const frame = dest.slice(0, first.written);
  // The regions are reused, and memory grows: the array keeps its frame.
  conn.encodeRequest(9, 9, "", false, "zzzzzzzz", 256);
  conn.encodeReply(9, 1n, Envelope.OK, 0, new Uint8Array(100_000).fill(0xee), 100_100);
  const big = open();
  comline.connectionNew({ framing: "datagram@2", handshake: Handshake.NONE, maxWaiting: 4_000_000 })
    .connection.free();
  assert.deepEqual(dest.subarray(0, first.written), frame);
  assert.equal(dest.byteLength, 256, "not detached by the grow");
  // Writing into the array changes nothing the module sees: the next encode
  // into the same array is the same frame but the request id (bytes 4-11).
  dest.fill(0xff);
  const second = conn.encodeRequestInto(dest, 3, 4, "", false, "abc");
  assert.equal(second.status, Status.OK);
  assert.equal(second.written, first.written);
  assert.deepEqual(dest.subarray(0, 4), frame.subarray(0, 4));
  assert.deepEqual(dest.subarray(12, second.written), frame.subarray(12));
  big.free();
  conn.free();
});
