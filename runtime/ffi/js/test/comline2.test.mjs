// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// `comline@2` through the glue, end to end: both exchanges, a call by wire
// numbers over each, what the readers give, and refusals. `vectors.test.mjs`
// proves each step against the corpus, `tests/comline2.rs` the ABI's rules.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  instantiateSync,
  Envelope,
  EventFlag,
  EventKind,
  Exchange,
  Handshake,
  NewerConsumer,
  Reason,
  SameVersionDiffers,
  Scope,
  Side,
  Status,
} from "../comline.mjs";

const ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const TARGET = process.env.CARGO_TARGET_DIR || join(ROOT, "target");
const WASM =
  process.env.COMLINE_WASM ||
  join(TARGET, "wasm32-unknown-unknown", "wasm", "comline_runtime_ffi.wasm");
const comline = instantiateSync(readFileSync(WASM));

/** A hello message: package id 16 × 9, the version `0.MINOR.0`, no encodings,
 * and `protocols` as `[number, line minor, shape byte, functions]`, numbers
 * under 128. */
function hello(exchange, minor, protocols) {
  const bytes = [0x43, 0x4f, 2, 1, exchange, ...new Array(16).fill(9), 0, minor, 0, 0, 0, protocols.length];
  for (const [number, line, shape, functions] of protocols) {
    bytes.push(number, 0, line, 0, ...new Array(16).fill(shape), functions.length, ...functions);
  }
  return new Uint8Array(bytes);
}

function connect(facts, side, exchange, parameters = {}) {
  const { status, connection } = comline.connectionNew({
    framing: "datagram@3",
    wireFormat: "packed@1",
    handshake: Handshake.COMLINE2,
    maxWaiting: 4,
    comline2: { hello: facts, side, exchange, ...parameters },
  });
  assert.equal(status, Status.OK);
  return connection;
}

/** Messages between the two until neither has one; the last fed statuses. */
function run(a, b) {
  let last = [Status.OK, Status.OK];
  for (let i = 0; i < 8; i++) {
    const fromA = a.start(4096);
    if (fromA.status === Status.OK && fromA.written) last[1] = b.feed(fromA.bytes).status;
    const fromB = b.start(4096);
    if (fromB.status === Status.OK && fromB.written) last[0] = a.feed(fromB.bytes).status;
  }
  return last;
}

test("it's supported and checked", () => {
  assert.equal(comline.supports("comline@2"), true);
  assert.equal(comline.supports("datagram@3"), true);
  assert.equal(comline.comline2MessageCheck(hello(0, 3, [])).kind, 1);
  assert.deepEqual(comline.comline2MessageCheck(new Uint8Array(0)), {
    status: Status.ERR_HANDSHAKE,
    kind: 0,
    reason: Reason.MALFORMED,
  });
  const notOne = comline.connectionNew({
    framing: "datagram@3",
    handshake: Handshake.COMLINE2,
  });
  assert.equal(notOne.status, Status.ERR_INVALID_ARGUMENT);
  assert.equal(notOne.connection, null);
});

for (const exchange of [Exchange.OFFER, Exchange.SYMMETRIC]) {
  test(`exchange ${exchange} connects and carries a call by numbers`, () => {
    const facts = hello(exchange, 3, [[0, 1, 1, [0, 1]], [5, 2, 2, [7]]]);
    const a = connect(facts, Side.INITIATOR, exchange);
    const b = connect(facts, Side.RESPONDER, exchange);
    assert.equal(a.encodeRequest(5, 7, "", true, "x", 64).status, Status.ERR_STATE);
    assert.deepEqual(run(a, b), [Status.OK, Status.OK]);

    assert.equal(a.checkCall(5, 7), Status.OK);
    const call = a.encodeRequest(5, 7, "", true, "x", 64);
    assert.equal(call.status, Status.OK);
    assert.deepEqual([...call.bytes.subarray(0, 3)], [0, 5, 7]);
    const fed = b.feed(call.bytes);
    assert.equal(fed.event.kind, EventKind.REQUEST);
    assert.deepEqual([fed.event.protocol, fed.event.callId], [5, 7]);
    const reply = b.encodeReply(5, fed.event.requestId, Envelope.OK, 0, "y", 64);
    assert.equal(a.feed(reply.bytes).event.kind, EventKind.REPLY);

    assert.deepEqual(a.protocols(8).protocols, [{ number: 0, reason: 0 }, { number: 5, reason: 0 }]);
    assert.deepEqual(a.warnings(8).numbers, []);
    assert.deepEqual(a.peerVersion(8).version, { major: 0, minor: 3, patch: 0 });
    assert.equal(a.peerVersion(8).pre, "");
    a.free();
    b.free();
  });
}

test("a newer consumer degrades: the call fails locally", () => {
  const a = connect(hello(0, 4, [[0, 1, 3, [0, 1]]]), Side.INITIATOR, Exchange.OFFER);
  const b = connect(hello(0, 3, [[0, 1, 1, [0]]]), Side.RESPONDER, Exchange.OFFER);
  const offer = a.start(4096);
  const fed = b.feed(offer.bytes);
  assert.equal(fed.event.kind, EventKind.HANDSHAKE_CONTINUE);
  assert.notEqual(fed.event.flags & EventFlag.SEND, 0);
  assert.equal(a.feed(b.start(4096).bytes).event.kind, EventKind.HANDSHAKE_OK);
  assert.equal(b.feed(a.start(4096).bytes).event.kind, EventKind.HANDSHAKE_OK);
  assert.equal(a.checkCall(0, 0), Status.OK);
  assert.equal(a.checkCall(0, 1), Status.ERR_UNAVAILABLE);
  assert.deepEqual(a.peerLacks(0, 8).numbers, [1]);
  a.free();
  b.free();
});

test("a refusal gives its reason and side at both ends", () => {
  const parameters = { scope: Scope.PACKAGE, newerConsumer: NewerConsumer.REFUSE };
  const a = connect(hello(0, 4, [[0, 1, 3, [0]]]), Side.INITIATOR, Exchange.OFFER, parameters);
  const b = connect(hello(0, 5, [[0, 2, 4, [0]]]), Side.RESPONDER, Exchange.OFFER, parameters);
  assert.deepEqual(run(a, b), [Status.ERR_REFUSED, Status.ERR_REFUSED]);
  const want = { reason: Reason.LINE_MISMATCH, side: Side.RESPONDER, protocol: 0 };
  assert.deepEqual(a.refusal().refusal, want);
  assert.deepEqual(b.refusal().refusal, want);
  assert.equal(a.encodeRequest(0, 0, "", true, "", 64).status, Status.ERR_STATE);
  a.free();
  b.free();
});

test("same_version_differs = warn connects and reports the protocol", () => {
  const warn = { sameVersionDiffers: SameVersionDiffers.WARN };
  const a = connect(hello(1, 3, [[2, 1, 1, [0]]]), Side.INITIATOR, Exchange.SYMMETRIC, warn);
  const b = connect(hello(1, 3, [[2, 1, 2, [0]]]), Side.RESPONDER, Exchange.SYMMETRIC, warn);
  assert.deepEqual(run(a, b), [Status.OK, Status.OK]);
  assert.deepEqual(a.warnings(8).numbers, [2]);
  assert.deepEqual(b.warnings(8).numbers, [2]);
  const small = a.warnings(0);
  assert.deepEqual([small.status, small.written], [Status.BUFFER_TOO_SMALL, 1]);
  a.free();
  b.free();
});
