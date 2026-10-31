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
  return { count, comline: new Comline(exports) };
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
