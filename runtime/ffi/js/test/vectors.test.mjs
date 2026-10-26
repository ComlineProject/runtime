// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// The conformance vectors, replayed through the wasm32 build by way of
// `comline.mjs`: the cases `examples/export_vectors.rs` writes out, with the
// checks `tests/vectors.rs` makes from Rust and `tests/c/vectors.c` makes
// from C. The export's module doc gives the line format and what each op
// does; each `case*` below follows the function of the same name in
// `tests/vectors.rs`.
//
//     node --test runtime/ffi/js/test/vectors.test.mjs
//
// The whole replay runs once per way of loading the module (`LOADERS`), so
// each is proven on every case. Every encode is sized by `encodeBound`: with
// everything the frame holds given, the bound must be the exact size the
// ABI writes, so the encode never needs a retry; with only the lengths, it
// must be no smaller.
//
// `env.mjs` says where the export and the module are. Each file fails on
// any mismatch, a malformed line, or a number of cases other than its
// header's.

import { after, before, describe, test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import {
  instantiate,
  instantiateStreaming,
  instantiateSync,
  encodeBound,
  EventFlag,
  EventKind,
  Handshake,
  Status,
} from "../comline.mjs";
import { VECTORS, WASM } from "./env.mjs";

const FILES = [
  "name-hash",
  "handshake",
  "envelope",
  "framing-datagram",
  "framing-datagram2",
  "framing-jsonrpc",
];

/** How a case went through the ABI, as `tests/vectors.rs` counts it. */
const FULL = "full";
const ID_PATCHED = "id-patched";
const OTHER_KIND = "rejected as the other kind";
const NOT_MAPPED = "not mapped";

const bytes = readFileSync(WASM);
const served = (type) => new Response(bytes, { headers: { "content-type": type } });

/** Every way of loading the module, each run on every case. */
const LOADERS = {
  "instantiate (bytes)": () => instantiate(bytes),
  "instantiate (module)": () => instantiate(new WebAssembly.Module(bytes)),
  "instantiateSync (bytes)": async () => instantiateSync(bytes),
  "instantiateSync (module)": async () => instantiateSync(new WebAssembly.Module(bytes)),
  "instantiateStreaming": () => instantiateStreaming(served("application/wasm")),
  "instantiateStreaming (from bytes)": () =>
    instantiateStreaming(Promise.resolve(served("application/octet-stream"))),
};

/** The instance the cases below run on: each loader's in turn. */
let comline;

/** A failed check: the replay records it against the case and goes on. */
class Mismatch extends Error {}

function check(ok, why) {
  if (!ok) throw new Mismatch(typeof why === "function" ? why() : why);
}

// ── fields ─────────────────────────────────────────────────────────────────

function bytesField(field) {
  if (field === "-") return new Uint8Array(0);
  check(/^(?:[0-9a-f]{2})+$/.test(field), `not hex: ${field}`);
  return Uint8Array.from(field.match(/../g), (h) => parseInt(h, 16));
}

/** A decimal field up to `max` (a BigInt). */
function u64Field(field, max = 0xffff_ffff_ffff_ffffn) {
  check(/^[0-9]+$/.test(field), `not a decimal: ${field}`);
  const v = BigInt(field);
  check(v <= max, `${field} is over ${max}`);
  return v;
}

const numField = (field, max) => Number(u64Field(field, BigInt(max)));

const hex = (b) => (b.length ? Buffer.from(b).toString("hex") : "-");

function sameBytes(got, want, what = "bytes") {
  check(
    got !== undefined && got.length === want.length && got.every((b, i) => b === want[i]),
    () => `${what}: got ${got ? hex(got) : "nothing"}, want ${hex(want)}`,
  );
}

/** The bytes a span names in `input`, when it lies inside it. */
function at(input, span) {
  check(span.offset + span.len <= input.length, () => {
    return `span ${span.offset}+${span.len} outside a ${input.length}-byte input`;
  });
  return input.subarray(span.offset, span.offset + span.len);
}

// ── the ABI, wrapped as tests/common/mod.rs wraps it ───────────────────────

/** No handshake, message mode, room for 16 waiting calls. */
function plain(framing) {
  return {
    framing,
    wireFormat: "json@1",
    irHash: 0x0102030405060708n,
    capabilities: 0,
    handshake: Handshake.NONE,
    maxWaiting: 16,
  };
}

/** A connection, freed when `body` returns. */
function withConn(setup, body) {
  const { status, connection } = comline.connectionNew(setup);
  check(status === Status.OK, `new gave ${status}`);
  try {
    return body(connection);
  } finally {
    connection.free();
  }
}

/** Spends request ids with one-way calls until the next one is `id`. */
function spendIds(conn, id) {
  for (let expected = 0n; expected < id; expected++) {
    const sent = conn.encodeRequest(0, 0, "spend", false, "null", 256);
    check(sent.status === Status.OK && sent.requestId === expected, () => {
      return `spending id ${expected} gave ${sent.status}, id ${sent.requestId}`;
    });
  }
}

/** A method name span, unescaped through `methodNameUnescape` when the
 * event says it's escaped. */
function methodName(input, ev) {
  const spelled = at(input, ev.callName);
  if (!(ev.flags & EventFlag.NAME_ESCAPED)) return spelled;
  const sized = comline.methodNameUnescape(spelled, 0);
  let needed;
  if (sized.status === Status.BUFFER_TOO_SMALL) needed = sized.written;
  else if (sized.status === Status.OK) needed = 0;
  else throw new Mismatch(`unescape gave ${sized.status}`);
  const out = comline.methodNameUnescape(spelled, needed);
  check(out.status === Status.OK && out.written === needed, () => {
    return `unescape gave ${out.status}, ${out.written} bytes`;
  });
  return out.bytes;
}

/** A reply event holds `(envelope, errorId, body)`. */
function checkReply(input, fed, envelope, errorId, body) {
  check(
    fed.status === Status.OK &&
      fed.event.kind === EventKind.REPLY &&
      fed.event.envelope === envelope &&
      fed.event.errorId === errorId &&
      (fed.event.flags & EventFlag.BODY_ABSENT) === 0,
    () => `feed gave ${JSON.stringify(fed, big)}`,
  );
  sameBytes(at(input, fed.event.body), body, "body");
}

const big = (_, v) => (typeof v === "bigint" ? v.toString() : v);

// ── the cases ──────────────────────────────────────────────────────────────

function caseNameHash(how, f) {
  check(how === "exact" && f.length === 2, "malformed");
  const { status, hash } = comline.nameHash(bytesField(f[0]));
  check(status === Status.OK, `name_hash gave ${status}`);
  const le = new Uint8Array(8);
  new DataView(le.buffer).setBigUint64(0, hash, true);
  sameBytes(le, bytesField(f[1]), "hash");
  return FULL;
}

function caseHandshake(how, f) {
  if (how === "reject") {
    check(f.length === 1, "malformed");
    const frame = bytesField(f[0]);
    const some = comline.handshakeEncode(1n, "json@1", "datagram@2", 0, 64);
    check(some.status === Status.OK, "a valid frame");
    const checked = comline.handshakeCheck(some.bytes, frame);
    check(checked.status === Status.ERR_HANDSHAKE, `check gave ${checked.status}`);
    check(checked.peer === null, "a frame that doesn't parse gave a peer");
    // And on a connection, as the first frame fed.
    const setup = { ...plain("datagram@2"), handshake: Handshake.COMLINE };
    withConn(setup, (conn) => {
      const fed = conn.feed(frame);
      check(fed.status === Status.ERR_HANDSHAKE, `feed gave ${fed.status}`);
      check(conn.feed(frame).status === Status.ERR_STATE, "a refused handshake leaves it open");
    });
    return FULL;
  }
  check(how === "exact" && f.length === 6, "malformed");
  const irHash = u64Field(f[0]);
  const [wireFormat, framing] = [f[1], f[2]];
  const capabilities = numField(f[3], 0xffff_ffff);
  const input = bytesField(f[4]);
  const want = bytesField(f[5]);

  // Encode.
  const tooSmall = comline.handshakeEncode(irHash, wireFormat, framing, capabilities, 30);
  check(tooSmall.status === Status.BUFFER_TOO_SMALL && tooSmall.written === 31, () => {
    return `30 bytes of room gave ${tooSmall.status}, ${tooSmall.written}`;
  });
  const encoded = comline.handshakeEncode(irHash, wireFormat, framing, capabilities, 64);
  check(encoded.status === Status.OK, `encode gave ${encoded.status}`);
  sameBytes(encoded.bytes, want, "encoded");

  // Decode: the peer's frame read back holds the input.
  const { status, peer } = comline.handshakeCheck(encoded.bytes, input);
  check(status === Status.OK, `check gave ${status}`);
  const wantPeer = {
    irHash,
    wireFormat: comline.nameHash(wireFormat).hash,
    framing: comline.nameHash(framing).hash,
    capabilities,
  };
  check(JSON.stringify(peer, big) === JSON.stringify(wantPeer, big), () => {
    return `decodes to ${JSON.stringify(peer, big)}`;
  });

  // On a connection, when the framing is a connection framing.
  if (comline.supports(framing)) {
    const setup = {
      ...plain(framing),
      handshake: Handshake.COMLINE,
      irHash,
      wireFormat,
      capabilities,
    };
    withConn(setup, (conn) => {
      const started = conn.start(64);
      check(started.status === Status.OK, `start gave ${started.status}`);
      sameBytes(started.bytes, input, "start");
      const fed = conn.feed(input);
      check(
        fed.status === Status.OK &&
          fed.event.kind === EventKind.HANDSHAKE_OK &&
          fed.event.capabilities === capabilities &&
          fed.consumed === input.length,
        () => `feed gave ${JSON.stringify(fed, big)}`,
      );
    });
  }
  return FULL;
}

/** Encode with `cap` set to `exact`, the bound with everything given,
 * which must be exactly what's written; `loose`, the bound from lengths
 * alone, must be no smaller. */
function sizedEncode(exact, loose, encode) {
  check(loose >= exact, `the loose bound ${loose} is under the exact ${exact}`);
  const encoded = encode(exact);
  check(encoded.status === Status.OK, `encode at the bound ${exact} gave ${encoded.status}`);
  check(encoded.written === exact, `wrote ${encoded.written}, the bound is ${exact}`);
  return encoded;
}

/** A `datagram@2` connection waiting on request 0, freed after `body`. */
function waitingOnZero(body) {
  return withConn(plain("datagram@2"), (conn) => {
    const sent = conn.encodeRequest(0, 0, "", true, new Uint8Array(0), 64);
    check(sent.status === Status.OK && sent.requestId === 0n, () => {
      return `request gave ${sent.status}, id ${sent.requestId}`;
    });
    return body(conn);
  });
}

function caseEnvelope(how, f) {
  if (how === "reject") {
    check(f.length === 1, "malformed");
    const frame = bytesField(f[0]);
    const fed = waitingOnZero((conn) => conn.feed(frame));
    check(fed.status === Status.ERR_FRAMING, () => `feed gave ${JSON.stringify(fed, big)}`);
    return FULL;
  }
  check(how === "exact" && f.length === 5, "malformed");
  const envelope = numField(f[0], 255);
  const errorId = numField(f[1], 0xffff);
  const body = bytesField(f[2]);
  const frame = bytesField(f[3]);
  const want = bytesField(f[4]);
  withConn(plain("datagram@2"), (conn) => {
    const reply = { framing: "datagram@2", kind: "reply", envelope, bodyLen: body.length };
    const encoded = sizedEncode(
      encodeBound({ ...reply, protocol: 0, requestId: 0n, errorId }),
      encodeBound(reply),
      (cap) => conn.encodeReply(0, 0n, envelope, errorId, body, cap),
    );
    sameBytes(encoded.bytes, want, "encoded");
  });
  const fed = waitingOnZero((conn) => conn.feed(frame));
  checkReply(frame, fed, envelope, errorId, body);
  return FULL;
}

/** A rejected request or response: `feed` refuses it as malformed, or reads
 * it as the other kind of frame. */
function rejectFrame(framing, frame, isRequest) {
  const fed = withConn(plain(framing), (conn) => conn.feed(frame));
  if (fed.status !== Status.OK) {
    check(fed.status === Status.ERR_FRAMING, `feed gave ${fed.status}`);
    return FULL;
  }
  if (isRequest) {
    check(fed.event.kind !== EventKind.REQUEST, "read a request it must reject");
  } else {
    check(
      fed.event.kind !== EventKind.REPLY && fed.event.kind !== EventKind.STALE,
      "read a reply it must reject",
    );
  }
  return OTHER_KIND;
}

function caseRequest(how, f) {
  if (how === "reject") {
    check(f.length === 2 && comline.supports(f[0]), "malformed");
    return rejectFrame(f[0], bytesField(f[1]), true);
  }
  const patched = how === "patched";
  check((patched || how === "exact") && f.length === 10, "malformed");
  const framing = f[0];
  const datagram2 = framing === "datagram@2";
  check(datagram2 || framing === "jsonrpc@1", `framing ${framing}`);
  const protocol = numField(f[1], 0xffff);
  const callId = numField(f[2], 0xffff);
  const method = bytesField(f[3]);
  const oneWay = numField(f[4], 1) === 1;
  const requestId = u64Field(f[5]);
  const drive = u64Field(f[6], 1000n);
  const params = bytesField(f[7]);
  const input = bytesField(f[8]);
  const want = bytesField(f[9]);

  // Encode, at the vector's id when the connection can be driven to it,
  // else at 0, for which the export's bytes are.
  withConn(plain(framing), (conn) => {
    spendIds(conn, drive);
    const request = { framing, kind: "request", bodyLen: params.length };
    const encoded = sizedEncode(
      encodeBound({
        ...request, protocol, name: method, expectsReply: !oneWay, requestId: drive,
      }),
      encodeBound({ ...request, nameLen: method.length }),
      (cap) => conn.encodeRequest(protocol, callId, method, !oneWay, params, cap),
    );
    if (!oneWay) {
      check(encoded.requestId === drive, `encoded as request ${encoded.requestId}`);
    }
    sameBytes(encoded.bytes, want, "encoded");
    // A one-way call doesn't wait, and a call that expects a reply does.
    const waiting = conn.isWaiting();
    check(waiting.status === Status.OK && waiting.waiting === !oneWay, "waiting");
  });

  // Decode, on a fresh connection.
  const fed = withConn(plain(framing), (conn) => conn.feed(input));
  const ev = fed.event;
  check(
    fed.status === Status.OK &&
      ev.kind === EventKind.REQUEST &&
      fed.consumed === input.length &&
      ev.requestId === requestId &&
      ((ev.flags & EventFlag.ONE_WAY) !== 0) === oneWay,
    () => `feed gave ${JSON.stringify(fed, big)}`,
  );
  sameBytes(at(input, ev.frame), input, "frame");
  sameBytes(at(input, ev.body), params, "params");
  if (datagram2) {
    check(
      (ev.flags & EventFlag.HAS_PROTOCOL) !== 0 &&
        (ev.flags & EventFlag.CALL_BY_NAME) === 0 &&
        ev.protocol === protocol &&
        ev.callId === callId,
      () => `feed gave ${JSON.stringify(fed, big)}`,
    );
  } else {
    check(
      (ev.flags & EventFlag.HAS_PROTOCOL) === 0 && (ev.flags & EventFlag.CALL_BY_NAME) !== 0,
      () => `feed gave ${JSON.stringify(fed, big)}`,
    );
    sameBytes(methodName(input, ev), method, "method");
  }
  return patched ? ID_PATCHED : FULL;
}

function caseResponse(how, f) {
  if (how === "reject") {
    check(f.length === 2 && comline.supports(f[0]), "malformed");
    return rejectFrame(f[0], bytesField(f[1]), false);
  }
  check(how === "exact" && f.length === 8, "malformed");
  const framing = f[0];
  check(comline.supports(framing), `framing ${framing}`);
  const protocol = numField(f[1], 0xffff);
  // A reply id too large to wait on isn't exported.
  const requestId = u64Field(f[2], 1000n);
  const envelope = numField(f[3], 255);
  const errorId = numField(f[4], 0xffff);
  const body = bytesField(f[5]);
  const input = bytesField(f[6]);
  const want = bytesField(f[7]);

  // Encode: a reply carries the peer's request id, so nothing to drive.
  withConn(plain(framing), (conn) => {
    const reply = { framing, kind: "reply", envelope, bodyLen: body.length };
    const encoded = sizedEncode(
      encodeBound({ ...reply, protocol, requestId, errorId }),
      encodeBound(reply),
      (cap) => conn.encodeReply(protocol, requestId, envelope, errorId, body, cap),
    );
    sameBytes(encoded.bytes, want, "encoded");
  });

  // Decode, on a connection waiting on the vector's request id.
  withConn(plain(framing), (caller) => {
    spendIds(caller, requestId);
    const sent = caller.encodeRequest(protocol, 0, "call", true, "null", 256);
    check(sent.status === Status.OK && sent.requestId === requestId, () => {
      return `request gave ${sent.status}, id ${sent.requestId}`;
    });
    const fed = caller.feed(input);
    checkReply(input, fed, envelope, errorId, body);
    check(fed.event.requestId === requestId, `the reply answers ${fed.event.requestId}`);
    const waiting = caller.isWaiting();
    check(waiting.status === Status.OK && waiting.waiting === false, "still waiting");
  });
  return FULL;
}

/** `datagram@1`: no ABI operation does it, and the ABI refuses it. */
function caseUnmapped(how, f) {
  check(how === "refused" && f.length === 1, "malformed");
  check(!comline.supports(f[0]), `${f[0]} is supported`);
  const { status, connection } = comline.connectionNew(plain(f[0]));
  connection?.free();
  check(status === Status.ERR_UNKNOWN_FRAMING && connection === null, `new gave ${status}`);
  return NOT_MAPPED;
}

const OPS = {
  name_hash: caseNameHash,
  handshake: caseHandshake,
  envelope: caseEnvelope,
  request: caseRequest,
  response: caseResponse,
  unmapped: caseUnmapped,
};

// ── the replay ─────────────────────────────────────────────────────────────

/** Replays one export file on `comline`; its counts go to `counts`. */
function replay(feature, counts) {
  const path = join(VECTORS, `${feature}.txt`);
  const text = readFileSync(path, "latin1");
  assert.ok(text.endsWith("\n"), `${path}: unterminated`);
  const [header, ...lines] = text.slice(0, -1).split("\n");
  const head = header.split(" ");
  assert.ok(
    head.length === 4 &&
      head[0] === "comline-ffi-vectors" &&
      head[1] === "1" &&
      head[2] === feature &&
      /^[0-9]+$/.test(head[3]),
    `${path}: not a version 1 export of ${feature}`,
  );
  const expected = Number(head[3]);
  const failures = [];
  let passed = 0;
  for (const line of lines) {
    const [op, id, how, ...fields] = line.split(" ");
    try {
      assert.ok(how !== undefined && OPS[op], `malformed line: ${line.slice(0, 60)}`);
      counts[OPS[op](how, fields)]++;
      passed++;
    } catch (e) {
      failures.push(`${id}: ${e.message}`);
    }
  }
  if (lines.length !== expected) {
    failures.push(`${path}: replayed ${lines.length} cases, the header says ${expected}`);
  }
  console.log(`${feature}: ${passed}/${expected} passed`);
  assert.deepEqual(failures, []);
}

for (const [loader, load] of Object.entries(LOADERS)) {
  describe(loader, () => {
    const counts = { [FULL]: 0, [ID_PATCHED]: 0, [OTHER_KIND]: 0, [NOT_MAPPED]: 0 };
    let instance;
    before(async () => {
      instance = await load();
    });
    for (const feature of FILES) {
      test(feature, () => {
        comline = instance;
        replay(feature, counts);
      });
    }
    after(() => {
      const total = Object.values(counts).reduce((a, b) => a + b, 0);
      console.log(
        `${loader}: ${total} cases: ${counts[FULL]} full, ${counts[ID_PATCHED]} id-patched, ` +
          `${counts[OTHER_KIND]} rejected as the other kind, ${counts[NOT_MAPPED]} not mapped`,
      );
    });
  });
}
