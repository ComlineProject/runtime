// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// The wasm32 poisoning rule: a trap poisons the whole instance. Against the
// `test-hooks` build (`env.mjs`), whose `comline_test_panic_next` makes the
// next call on a handle panic inside its guard, as `tests/rules.rs` does
// natively. On wasm32 that panic is a trap the guard can't catch, so the
// glue must mark the instance dead: the call that trapped and every later
// call on the instance and on each of its connections return
// `ERR_POISONED`, without entering the module, while a fresh instance
// works.
//
//     node --test runtime/ffi/js/test/poison.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { Comline, Envelope, EventKind, Handshake, Status, instantiateSync } from "../comline.mjs";
import { WASM_TEST_HOOKS } from "./env.mjs";

const bytes = readFileSync(WASM_TEST_HOOKS);
const module = new WebAssembly.Module(bytes);
const POISONED = Status.ERR_POISONED;

/** A request frame for `datagram@2`: protocol 0, call 1, request id 7. */
const REQUEST = Uint8Array.of(0, 0, 1, 0, 7, 0, 0, 0, 0, 0, 0, 0);

/**
 * An instance whose exports are wrapped, to count the calls that enter the
 * module and to replace one where a test says so (`swap[name]`). The glue
 * sees an ordinary exports object.
 */
function counted() {
  const raw = new WebAssembly.Instance(module, {}).exports;
  const counter = { calls: 0, swap: {} };
  const exports = {};
  for (const [name, value] of Object.entries(raw)) {
    exports[name] =
      typeof value === "function"
        ? (...args) => {
            counter.calls++;
            return (counter.swap[name] ?? value)(...args);
          }
        : value;
  }
  return { raw, counter, comline: new Comline(exports) };
}

const setup = { framing: "datagram@2", handshake: Handshake.NONE, maxWaiting: 4 };

function open(comline) {
  const { status, connection } = comline.connectionNew(setup);
  assert.equal(status, Status.OK);
  return connection;
}

/** Every call there is, on `comline` and on `conns`, each of which must say
 * poisoned. */
function everyCallPoisoned(comline, conns) {
  for (const conn of conns) {
    assert.deepEqual(conn.start(64), { status: POISONED, written: 0 });
    const fed = conn.feed(REQUEST);
    assert.equal(fed.status, POISONED);
    assert.equal(fed.consumed, 0);
    assert.equal(fed.event.kind, 0);
    assert.equal(fed.event.requestId, 0n);
    assert.deepEqual(conn.encodeRequest(0, 1, "", true, "body", 64), { status: POISONED, written: 0 });
    assert.deepEqual(conn.encodeReply(0, 7n, Envelope.OK, 0, "body", 64), {
      status: POISONED,
      written: 0,
    });
    assert.equal(conn.abandon(7n), POISONED);
    assert.deepEqual(conn.isWaiting(), { status: POISONED });
  }
  assert.deepEqual(comline.nameHash("json@1"), { status: POISONED });
  assert.deepEqual(comline.handshakeEncode(1n, "json@1", "datagram@2", 0, 64), {
    status: POISONED,
    written: 0,
  });
  assert.deepEqual(comline.handshakeCheck(new Uint8Array(31), new Uint8Array(31)), {
    status: POISONED,
    peer: null,
  });
  assert.deepEqual(comline.methodNameUnescape("a", 8), { status: POISONED, written: 0 });
  assert.deepEqual(comline.connectionNew(setup), { status: POISONED, connection: null });
  assert.equal(comline.supports("datagram@2"), false);
  assert.equal(comline.poisoned, true);
}

test("the hook's panic is a trap on wasm32, not ERR_POISONED", () => {
  const x = new WebAssembly.Instance(module, {}).exports;
  assert.equal(typeof x.comline_test_panic_next, "function");
  // A raw `comline_connection_abandon` on a handle the glue made: the
  // module traps instead of returning a status.
  const comline = new Comline(x);
  const conn = open(comline);
  x.comline_test_panic_next(conn.handle);
  assert.throws(() => x.comline_connection_abandon(conn.handle, 0n), WebAssembly.RuntimeError);
});

test("a trap poisons the instance and every connection on it", () => {
  const { raw, counter, comline } = counted();
  const a = open(comline);
  const b = open(comline);
  // Both work before the trap.
  const sent = a.encodeRequest(0, 1, "", true, "x", 64);
  assert.equal(sent.status, Status.OK);
  assert.equal(b.feed(REQUEST).event.kind, EventKind.REQUEST);
  assert.equal(comline.poisoned, false);

  raw.comline_test_panic_next(a.handle);
  const trapped = a.feed(REQUEST);
  assert.equal(trapped.status, POISONED, "the call that trapped");
  assert.equal(comline.poisoned, true);

  // Nothing after it enters the module.
  const before = counter.calls;
  everyCallPoisoned(comline, [a, b]);
  a.free();
  b.free();
  assert.equal(counter.calls, before, "a poisoned instance was entered");
  // Freed connections are still a caller's mistake, as before.
  assert.throws(() => a.feed(REQUEST), /freed/);

  // A fresh instance works.
  const fresh = instantiateSync(bytes);
  const conn = open(fresh);
  const request = conn.encodeRequest(0, 1, "", true, "x", 64);
  assert.equal(request.status, Status.OK);
  const reply = conn.encodeReply(0, request.requestId, Envelope.OK, 0, "y", 64);
  assert.equal(reply.status, Status.OK);
  const fed = conn.feed(reply.bytes);
  assert.equal(fed.status, Status.OK);
  assert.equal(fed.event.kind, EventKind.REPLY);
  assert.equal(fed.event.requestId, request.requestId);
  conn.free();
  assert.equal(fresh.poisoned, false);
});

test("every connection call that traps poisons", () => {
  const calls = {
    start: (c) => c.start(64),
    feed: (c) => c.feed(REQUEST),
    encodeRequest: (c) => c.encodeRequest(0, 1, "", true, "x", 64),
    encodeReply: (c) => c.encodeReply(0, 7n, Envelope.OK, 0, "x", 64),
    abandon: (c) => ({ status: c.abandon(7n) }),
    isWaiting: (c) => c.isWaiting(),
  };
  for (const [name, call] of Object.entries(calls)) {
    const { raw, comline } = counted();
    const a = open(comline);
    const b = open(comline);
    raw.comline_test_panic_next(a.handle);
    assert.equal(call(a).status, POISONED, `${name} that trapped`);
    everyCallPoisoned(comline, [a, b]);
  }
});

test("a RangeError from inside the module is a trap too", () => {
  // The engine reports stack exhaustion in wasm as a `RangeError`, which
  // stops the module mid-call just as a trap does.
  const { counter, comline } = counted();
  const conn = open(comline);
  counter.swap.comline_name_hash = () => {
    throw new RangeError("Maximum call stack size exceeded");
  };
  assert.deepEqual(comline.nameHash("json@1"), { status: POISONED });
  everyCallPoisoned(comline, [conn]);
});

test("other exceptions are thrown as before, and poison nothing", () => {
  const { counter, comline } = counted();
  const conn = open(comline);
  // Caught in the glue, before the module.
  assert.throws(() => conn.encodeRequest(0, 1, "", true, 42, 64), TypeError);
  assert.throws(() => comline.nameHash({}), TypeError);
  assert.throws(() => conn.encodeReply(0, 1.5, Envelope.OK, 0, "x", 64), RangeError);
  // Thrown by the engine at the boundary, as it converts an argument: a
  // BigInt where a `u16` goes.
  assert.throws(() => conn.encodeRequest(1n, 1, "", true, "x", 64), TypeError);
  // Memory that can't grow: the module is as it was.
  counter.swap.comline_wasm_reserve_in = () => 0;
  assert.throws(() => comline.nameHash("x".repeat(1 << 20)), /can't grow/);
  delete counter.swap.comline_wasm_reserve_in;
  assert.equal(comline.poisoned, false);
  // Still working.
  assert.equal(conn.feed(REQUEST).status, Status.OK);
  assert.equal(comline.nameHash("json@1").status, Status.OK);
  conn.free();
});
