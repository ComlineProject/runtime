// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// `encodeBound` against what the ABI actually writes, on random encodes:
// names with every control character, quotes, backslashes and non-ASCII
// (as strings, lone surrogates included, and as bytes), protocols and ids
// across their ranges (reply ids up to `u64::MAX`), bodies of 0 to 70000
// bytes, both framings, both kinds and every envelope, in message and
// stream mode.
//
// Each encode is checked twice. Given everything the frame holds, the bound
// must be exactly `*written`. From lengths alone (`nameLen`, no protocol,
// ids or error id), it must be no smaller; the encode is made with that as
// `cap`, so it must succeed the first time. The slack of the loose bound is
// reported per framing and kind. Then the checker is run on a bound with an
// off-by-one planted in it, and must catch it, so a check that can't fail
// isn't mistaken for a proof.
//
// Request ids come from the core, from 0, so a request is encoded at the id
// a fresh connection gives after a few are spent; the digits of every id up
// to `u64::MAX` go through the same count on replies, whose id the caller
// gives.
//
//     node --test runtime/ffi/js/test/bound.test.mjs
//
// `COMLINE_WASM` is the module; it defaults to the `wasm` profile's wasm32
// build under `CARGO_TARGET_DIR` (or the repository's `target`).
//
// `COMLINE_BOUND_RUNS` (default 4000) and `COMLINE_BOUND_SEED` choose the
// run; a failure prints its seed and case.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Envelope, Handshake, Status, encodeBound, instantiateSync } from "../comline.mjs";

const ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const TARGET = process.env.CARGO_TARGET_DIR || join(ROOT, "target");
const WASM =
  process.env.COMLINE_WASM ||
  join(TARGET, "wasm32-unknown-unknown", "wasm", "comline_runtime_ffi.wasm");

const comline = instantiateSync(readFileSync(WASM));
const RUNS = Number(process.env.COMLINE_BOUND_RUNS ?? 4000);
const SEED = Number(process.env.COMLINE_BOUND_SEED ?? 0x5eed);

/** A small seeded generator (mulberry32), so a failure can be replayed. */
function rng(seed) {
  let a = seed >>> 0;
  const next = () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const int = (n) => Math.floor(next() * n);
  const pick = (xs) => xs[int(xs.length)];
  return { next, int, pick };
}

/** A random method name, as a string: control characters, quotes,
 * backslashes, DEL, ASCII, and 2-, 3- and 4-byte UTF-8, lone surrogates
 * too; sometimes nothing but `\u00XX` escapes, the most a byte can take. */
function nameOf(r) {
  const len = r.pick([0, 1, 2, r.int(8), r.int(40), r.int(300)]);
  const onlyControls = r.int(8) === 0;
  let s = "";
  for (let i = 0; i < len; i++) {
    if (onlyControls) {
      s += String.fromCharCode(r.pick([0x00, 0x01, 0x07, 0x0b, 0x0e, 0x1f]));
      continue;
    }
    switch (r.int(9)) {
      case 0: s += String.fromCharCode(r.int(0x20)); break;
      case 1: s += r.pick(['"', "\\", "\n", "\r", "\t", "\b", "\f", "/"]); break;
      case 2: s += "\x7f"; break;
      case 3: s += String.fromCharCode(0x80 + r.int(0x780)); break;
      case 4: s += String.fromCharCode(0x800 + r.int(0xd000 - 0x800)); break;
      case 5: s += String.fromCodePoint(0x10000 + r.int(0x100000)); break;
      case 6: s += String.fromCharCode(0xd800 + r.int(0x800)); break; // lone
      default: s += String.fromCharCode(0x20 + r.int(0x5f));
    }
  }
  return s;
}

const U64_MAX = (1n << 64n) - 1n;

function idOf(r) {
  switch (r.int(4)) {
    case 0: return r.pick([0n, 9n, 10n, 99n, 100n, U64_MAX, U64_MAX - 1n, 1n << 63n, 10n ** 19n - 1n, 10n ** 19n]);
    case 1: return BigInt(r.int(1000));
    case 2: return 10n ** BigInt(r.int(20));
    default: {
      const hi = BigInt(r.int(2 ** 32));
      const lo = BigInt(r.int(2 ** 32));
      return (hi << 32n) | lo;
    }
  }
}

const u16Of = (r) => r.pick([0, 1, 127, 128, 16383, 16384, 65535, r.int(65536)]);

function bodyLenOf(r) {
  return r.pick([0, 1, 2, 3, 4, r.int(64), r.int(1024), 65535, 65536, 70000, r.int(70001)]);
}

/** One random encode: its arguments, and the bound's two views of it. */
function caseOf(r) {
  const framing = r.pick(["datagram@2", "jsonrpc@1"]);
  const stream = r.int(2) === 1;
  const kind = r.pick(["request", "reply"]);
  const bodyLen = bodyLenOf(r);
  const protocol = u16Of(r);
  if (kind === "request") {
    const string = nameOf(r);
    const name = r.int(2) ? string : new TextEncoder().encode(string);
    return {
      framing, stream, kind, bodyLen, protocol, name,
      callId: u16Of(r),
      expectsReply: r.int(2) === 1,
      spend: r.int(12),
    };
  }
  return {
    framing, stream, kind, bodyLen, protocol,
    envelope: r.pick([Envelope.OK, Envelope.ERR, Envelope.FAULT]),
    errorId: u16Of(r),
    requestId: idOf(r),
  };
}

/** The UTF-8 length of a name as the encode sends it. */
const utf8Len = (name) => (typeof name === "string" ? new TextEncoder().encode(name).length : name.length);

const bodies = new Map();
const bodyOf = (len) => {
  if (!bodies.has(len)) bodies.set(len, Uint8Array.from({ length: len }, (_, i) => (i * 37 + 11) & 0xff));
  return bodies.get(len);
};

/**
 * Encodes `c` through the ABI, sized by `bound`, and returns what's wrong:
 * nothing when the exact bound is `*written` and the loose one is no
 * smaller (and its encode succeeds first time). Records the loose slack.
 */
function checkCase(c, bound, slack) {
  const { connection: conn } = comline.connectionNew({
    framing: c.framing,
    stream: c.stream,
    handshake: Handshake.NONE,
    maxWaiting: 16,
    unordered: true,
  });
  try {
    const body = c.envelope === Envelope.FAULT ? new Uint8Array(0) : bodyOf(c.bodyLen);
    const shape = { framing: c.framing, stream: c.stream, kind: c.kind, bodyLen: body.length };
    let exact;
    let loose;
    let encode;
    if (c.kind === "request") {
      for (let i = 0; i < c.spend; i++) conn.encodeRequest(0, 0, "", false, "", 64);
      exact = bound({
        ...shape, protocol: c.protocol, name: c.name, expectsReply: c.expectsReply,
        requestId: BigInt(c.spend),
      });
      loose = bound({ ...shape, nameLen: utf8Len(c.name) });
      encode = (cap) => conn.encodeRequest(c.protocol, c.callId, c.name, c.expectsReply, body, cap);
    } else {
      exact = bound({ ...shape, protocol: c.protocol, envelope: c.envelope, errorId: c.errorId, requestId: c.requestId });
      loose = bound({ ...shape, envelope: c.envelope });
      encode = (cap) => conn.encodeReply(c.protocol, c.requestId, c.envelope, c.errorId, body, cap);
    }
    // At the loose bound: it must fit the first time.
    const encoded = encode(loose);
    if (encoded.status !== Status.OK) return `at the loose bound ${loose}: status ${encoded.status}`;
    const written = encoded.written;
    if (c.kind === "request" && c.expectsReply && encoded.requestId !== BigInt(c.spend)) {
      return `request id ${encoded.requestId}, expected ${c.spend}`;
    }
    if (exact !== written) return `the exact bound is ${exact}, ${written} written`;
    if (loose < written) return `the loose bound ${loose} is under ${written}`;
    const key = `${c.framing} ${c.kind}`;
    const s = (slack[key] ??= { n: 0, max: 0, sum: 0, maxRel: 0 });
    s.n++;
    s.sum += loose - written;
    s.max = Math.max(s.max, loose - written);
    s.maxRel = Math.max(s.maxRel, (loose - written) / written);
    return null;
  } finally {
    conn.free();
  }
}

/** Runs the property on `runs` cases; the failures, and the slack. */
function run(bound, runs, seed) {
  const r = rng(seed);
  const failures = [];
  const slack = {};
  for (let i = 0; i < runs; i++) {
    const c = caseOf(r);
    const why = checkCase(c, bound, slack);
    if (why) failures.push({ i, why, c });
  }
  return { failures, slack };
}

const show = (c) => JSON.stringify(c, (_, v) =>
  typeof v === "bigint" ? `${v}n` : v instanceof Uint8Array ? `bytes:${Buffer.from(v).toString("hex")}` : v);

test(`encodeBound is exact, and never under, on ${RUNS} random encodes (seed ${SEED})`, () => {
  const { failures, slack } = run(encodeBound, RUNS, SEED);
  for (const [key, s] of Object.entries(slack).sort()) {
    console.log(
      `${key}: ${s.n} cases; the loose bound's slack: mean ${(s.sum / s.n).toFixed(1)} B, ` +
        `max ${s.max} B, max ${(100 * s.maxRel).toFixed(0)}% of the frame`,
    );
  }
  assert.deepEqual(
    failures.slice(0, 5).map(({ i, why, c }) => `case ${i}: ${why}: ${show(c)}`),
    [],
  );
});

test("a planted off-by-one fails the check", () => {
  // One byte short, everywhere: the exact bound is then always under.
  const short = (args) => encodeBound(args) - 1;
  const { failures } = run(short, 200, SEED + 1);
  assert.equal(failures.length, 200, "every case must catch a bound one byte short");
  // One byte short only on one path that's easy to miss: the `\u00XX`
  // escapes of a name counted at 5 bytes a byte when only its length is
  // given.
  const fiveEscape = (args) =>
    args.nameLen != null && args.framing === "jsonrpc@1"
      ? encodeBound(args) - args.nameLen
      : encodeBound(args);
  const { failures: caught } = run(fiveEscape, 2000, SEED + 2);
  assert.ok(caught.length > 0, "a 5-byte escape went unnoticed");
});

test("the loose bound is tight for names of `\\u00XX` escapes alone", () => {
  // A one-way request has no id, so only the name is loose; a name of
  // control characters without a short escape takes exactly 6 bytes a byte.
  const { connection: conn } = comline.connectionNew({ framing: "jsonrpc@1", handshake: Handshake.NONE });
  for (const len of [0, 1, 2, 17, 255]) {
    const name = "\x01".repeat(len);
    const bound = encodeBound({ framing: "jsonrpc@1", kind: "request", nameLen: len, expectsReply: false, bodyLen: 3 });
    const sent = conn.encodeRequest(0, 0, name, false, "[1]", bound);
    assert.equal(sent.status, Status.OK);
    assert.equal(sent.written, bound, `a ${len}-byte name`);
  }
  conn.free();
});

test("encodeBound refuses what it can't size", () => {
  assert.throws(() => encodeBound({ framing: "datagram@1", kind: "request", bodyLen: 0 }), TypeError);
  assert.throws(() => encodeBound({ framing: "datagram@2", kind: "call", bodyLen: 0 }), TypeError);
  assert.throws(() => encodeBound({ framing: "datagram@2", kind: "reply", bodyLen: 0 }), TypeError);
  assert.throws(() => encodeBound({ framing: "datagram@2", kind: "request", bodyLen: -1 }), TypeError);
  assert.throws(() => encodeBound({ framing: "jsonrpc@1", kind: "request", bodyLen: 0 }), TypeError);
});
