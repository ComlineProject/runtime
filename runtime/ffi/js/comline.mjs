// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// The JS glue over the wasm32 build of `comline-runtime-ffi`: the same
// functions `comline.h` declares, for a host that has no pointers. It is a
// thin, faithful wrapper: every call is one ABI call, and returns the
// status with whatever the call wrote; it retries nothing, throws on no
// status, and adds no state. A binding (exceptions, async, codecs) builds on
// it. No dependencies, no wasm-bindgen.
//
//     import { instantiate, Status } from "./comline.mjs";
//     const comline = await instantiate(await readFile("comline_runtime_ffi.wasm"));
//     const { status, connection } = comline.connectionNew({ framing: "datagram@2", handshake: Handshake.NONE, maxWaiting: 16 });
//
// How memory crosses: the module owns two regions, `in` and `out`
// (`comline_wasm_reserve_in/out`). A call copies its inputs (names, bodies,
// the config) into `in`, gives the ABI pointers into `out` for everything it
// writes (bytes, `*written`, the event, the handle), and copies the outputs
// back. The ABI refuses an input that overlaps an output, and the two
// regions never do. Byte outputs are fresh copies; nothing returned points
// into wasm memory.
//
// Numbers: a `u64` is a BigInt both ways; everything else is a Number.
// Names (framings, wire formats, methods) are a string, encoded as UTF-8,
// or a Uint8Array of the bytes as they are. A panic in the module is a trap
// on wasm32, thrown as a `WebAssembly.RuntimeError`: drop the instance.
//
// Where a call's out-pointer is written only on some outcomes, the result
// leaves that field out (or null) when the ABI didn't write it.

/** The ABI version this glue is written against. Before 1.0 a minor counts
 * as a major, so `instantiate` accepts exactly this. */
export const ABI_MAJOR = 0;
export const ABI_MINOR = 1;

/** `comline_status`. Below 16 is an outcome, 16 and up an error. A number
 * not here is a code from a newer library. */
export const Status = Object.freeze({
  OK: 0,
  NEED_MORE: 1,
  BUFFER_TOO_SMALL: 2,
  ERR_FRAMING: 16,
  ERR_HANDSHAKE: 17,
  ERR_FRAME_TOO_LARGE: 18,
  ERR_UNKNOWN_FRAMING: 19,
  ERR_INVALID_ARGUMENT: 20,
  ERR_STATE: 21,
  ERR_POISONED: 22,
  ERR_BUSY: 23,
});

/** `comline_config.handshake`. */
export const Handshake = Object.freeze({ COMLINE: 0, NONE: 1 });

/** `comline_event.kind`. */
export const EventKind = Object.freeze({
  HANDSHAKE_OK: 0,
  REQUEST: 1,
  REPLY: 2,
  STALE: 3,
});

/** `comline_event.flags` bits. */
export const EventFlag = Object.freeze({
  HAS_PROTOCOL: 1 << 0,
  ONE_WAY: 1 << 1,
  CALL_BY_NAME: 1 << 2,
  BODY_ABSENT: 1 << 3,
  NAME_ESCAPED: 1 << 4,
});

/** `comline_event.envelope` and `encodeReply`'s `envelope`. */
export const Envelope = Object.freeze({ OK: 0, ERR: 1, FAULT: 2 });

export const DEFAULT_MAX_FRAME = 16 * 1024 * 1024;
export const MAX_FRAME_CEILING = 4294967291;
export const PREFIX_LEN = 4;
export const HANDSHAKE_LEN = 31;

// The layouts on wasm32 (4-byte pointers and `size_t`), as `src/lib.rs`
// asserts them.
const EVENT_SIZE = 48;
const EVENT = {
  kind: 0, flags: 1, protocol: 2, callId: 4, envelope: 6, errorId: 8,
  callName: 12, body: 20, frame: 28, capabilities: 36, requestId: 40,
};
const CONFIG_SIZE = 48;
const CONFIG = {
  irHash: 0, wireFormat: 8, wireFormatLen: 12, framing: 16, framingLen: 20,
  maxFrame: 24, capabilities: 32, maxWaiting: 36, handshake: 40, stream: 41,
  unordered: 42, reserved: 43,
};
const HANDSHAKE_SIZE = 32;

const utf8 = new TextEncoder();

/** Bytes of a name or body: a string as UTF-8, bytes as they are. */
function bytesOf(value) {
  if (typeof value === "string") return utf8.encode(value);
  if (value instanceof Uint8Array) return value;
  if (value == null) return new Uint8Array(0);
  throw new TypeError("expected a string or a Uint8Array");
}

const align8 = (n) => (n + 7) & ~7;

/**
 * Instantiate the wasm32 build and check it: its ABI version must be
 * `ABI_MAJOR.ABI_MINOR`, and one fed frame must land in `comline_event` at
 * the offsets above, in 48 bytes.
 *
 * @param {BufferSource | WebAssembly.Module} source the `.wasm` bytes or a
 *   compiled module.
 */
export async function instantiate(source) {
  const module = source instanceof WebAssembly.Module ? source : await WebAssembly.compile(source);
  const instance = await WebAssembly.instantiate(module, {});
  return new Comline(instance.exports);
}

/** The library: its free functions, and `connectionNew`. */
export class Comline {
  #x;

  /** Prefer `instantiate`. Checks the version and the layout, as it says. */
  constructor(exports) {
    this.#x = exports;
    const version = this.abiVersion();
    if (version !== ((ABI_MAJOR << 16) | ABI_MINOR)) {
      throw new Error(
        `comline: the module is ABI ${version >>> 16}.${version & 0xffff}, this glue ${ABI_MAJOR}.${ABI_MINOR}`,
      );
    }
    this.#checkLayout();
  }

  /**
   * The layouts above, proven against the module: a `datagram@2` request
   * fed through a connection (which proves `comline_config` too) must read
   * back field by field, and `feed` must write nothing past 48 bytes.
   */
  #checkLayout() {
    const { status, connection } = this.connectionNew({
      framing: "datagram@2",
      handshake: Handshake.NONE,
    });
    if (status !== Status.OK) {
      throw new Error(`comline: the layout check's connection gave status ${status}`);
    }
    try {
      // Kind 0, protocol 5, call 0x0203, request id 0x0807060504030201, body "x".
      const frame = Uint8Array.of(0, 5, 3, 2, 1, 2, 3, 4, 5, 6, 7, 8, 0x78);
      const want = {
        kind: EventKind.REQUEST, flags: EventFlag.HAS_PROTOCOL, protocol: 5, callId: 0x0203,
        envelope: 0, errorId: 0, callName: { offset: 0, len: 0 }, body: { offset: 12, len: 1 },
        frame: { offset: 0, len: 13 }, capabilities: 0, requestId: 0x0807060504030201n,
      };
      const fed = this._feed(connection.handle, frame, 8);
      const same = JSON.stringify(fed.event, big) === JSON.stringify(want, big);
      if (fed.status !== Status.OK || fed.consumed !== 13 || !same || !fed.tailIntact) {
        throw new Error(`comline: comline_event isn't the ${EVENT_SIZE}-byte layout this glue reads`);
      }
    } finally {
      connection.free();
    }
  }

  // ── memory ──────────────────────────────────────────────────────────────

  /**
   * Reserve `inLen` bytes of `in` and `outLen` of `out`, 8-aligned, and
   * return their addresses with fresh views (a reserve may grow memory,
   * which detaches the old ones).
   */
  #regions(inLen, outLen) {
    const x = this.#x;
    const at = (reserve, len) => {
      const ptr = reserve(len + 8) >>> 0;
      if (ptr === 0) throw new RangeError("comline: wasm memory can't grow");
      return align8(ptr);
    };
    const inPtr = at(x.comline_wasm_reserve_in, inLen);
    const outPtr = at(x.comline_wasm_reserve_out, outLen);
    const buffer = x.memory.buffer;
    return { inPtr, outPtr, u8: new Uint8Array(buffer), dv: new DataView(buffer) };
  }

  /** A view of memory as it is now. */
  #view() {
    return new DataView(this.#x.memory.buffer);
  }

  /** Lay `parts` out in `in` from `base`, each 8-aligned; their addresses. */
  static #place(u8, base, parts) {
    const ptrs = [];
    let at = base;
    for (const part of parts) {
      u8.set(part, at);
      ptrs.push(at);
      at = align8(at + part.length);
    }
    return ptrs;
  }

  static #size(parts) {
    return parts.reduce((n, p) => n + align8(p.length), 0);
  }

  // ── free functions ──────────────────────────────────────────────────────

  /** `comline_abi_version`: `(major << 16) | minor`. */
  abiVersion() {
    return this.#x.comline_abi_version() >>> 0;
  }

  /** `comline_supports`: whether the named framing or handshake works on a
   * connection. */
  supports(name) {
    const n = bytesOf(name);
    const { inPtr, u8 } = this.#regions(n.length, 0);
    u8.set(n, inPtr);
    return this.#x.comline_supports(inPtr, n.length) === 1;
  }

  /** `comline_name_hash`: `{ status, hash }`, `hash` a BigInt. */
  nameHash(name) {
    const n = bytesOf(name);
    const { inPtr, outPtr, u8 } = this.#regions(n.length, 8);
    u8.set(n, inPtr);
    const status = this.#x.comline_name_hash(inPtr, n.length, outPtr);
    const hash = status === Status.OK ? this.#view().getBigUint64(outPtr, true) : undefined;
    return { status, hash };
  }

  /**
   * `comline_handshake_encode`: `{ status, written, bytes }`. `written` is
   * the size needed on `BUFFER_TOO_SMALL`; `bytes` is there on `OK`.
   */
  handshakeEncode(irHash, wireFormat, framing, capabilities, cap) {
    const w = bytesOf(wireFormat);
    const f = bytesOf(framing);
    const { inPtr, outPtr, u8 } = this.#regions(Comline.#size([w, f]), 8 + cap);
    const [wPtr, fPtr] = Comline.#place(u8, inPtr, [w, f]);
    const status = this.#x.comline_handshake_encode(
      BigInt.asUintN(64, BigInt(irHash)), wPtr, w.length, fPtr, f.length,
      capabilities >>> 0, outPtr + 8, cap, outPtr,
    );
    return this.#written(status, outPtr, outPtr + 8);
  }

  /**
   * `comline_handshake_check`: `{ status, peer }`. `peer` is the peer's
   * frame (names hashed) whenever it parses, agreeing or not, and null when
   * the ABI didn't write it.
   */
  handshakeCheck(ours, peer) {
    const o = bytesOf(ours);
    const p = bytesOf(peer);
    const { inPtr, outPtr, u8, dv: before } = this.#regions(Comline.#size([o, p]), HANDSHAKE_SIZE);
    let dv = before;
    const [oPtr, pPtr] = Comline.#place(u8, inPtr, [o, p]);
    // A sentinel in `_reserved`, which a written frame holds as 0.
    u8.fill(0, outPtr, outPtr + HANDSHAKE_SIZE);
    dv.setUint32(outPtr + 28, 0xdead, true);
    const status = this.#x.comline_handshake_check(oPtr, o.length, pPtr, p.length, outPtr);
    dv = this.#view();
    const written = dv.getUint32(outPtr + 28, true) !== 0xdead;
    return {
      status,
      peer: written
        ? {
            irHash: dv.getBigUint64(outPtr, true),
            wireFormat: dv.getBigUint64(outPtr + 8, true),
            framing: dv.getBigUint64(outPtr + 16, true),
            capabilities: dv.getUint32(outPtr + 24, true),
          }
        : null,
    };
  }

  /**
   * `comline_method_name_unescape`: `{ status, written, bytes }`, as
   * `handshakeEncode`. `spelled` is a `callName` span's bytes.
   */
  methodNameUnescape(spelled, cap) {
    const s = bytesOf(spelled);
    const { inPtr, outPtr, u8 } = this.#regions(s.length, 8 + cap);
    u8.set(s, inPtr);
    const status = this.#x.comline_method_name_unescape(inPtr, s.length, outPtr + 8, cap, outPtr);
    return this.#written(status, outPtr, outPtr + 8);
  }

  /**
   * `comline_connection_new`: `{ status, connection }`, `connection` a
   * `Connection` on `OK` and null otherwise. Fields left out are 0, as in a
   * zeroed `comline_config` (so `handshake` defaults to `COMLINE`).
   *
   * @param {import("./comline.d.mts").Config} config
   */
  connectionNew(config) {
    const w = bytesOf(config.wireFormat ?? "");
    const f = bytesOf(config.framing ?? "");
    const { inPtr, outPtr, u8, dv } = this.#regions(CONFIG_SIZE + Comline.#size([w, f]), 4);
    const [wPtr, fPtr] = Comline.#place(u8, inPtr + CONFIG_SIZE, [w, f]);
    const c = inPtr;
    u8.fill(0, c, c + CONFIG_SIZE);
    dv.setBigUint64(c + CONFIG.irHash, BigInt.asUintN(64, BigInt(config.irHash ?? 0)), true);
    dv.setUint32(c + CONFIG.wireFormat, wPtr, true);
    dv.setUint32(c + CONFIG.wireFormatLen, w.length, true);
    dv.setUint32(c + CONFIG.framing, fPtr, true);
    dv.setUint32(c + CONFIG.framingLen, f.length, true);
    dv.setBigUint64(c + CONFIG.maxFrame, BigInt.asUintN(64, BigInt(config.maxFrame ?? 0)), true);
    dv.setUint32(c + CONFIG.capabilities, (config.capabilities ?? 0) >>> 0, true);
    dv.setUint32(c + CONFIG.maxWaiting, (config.maxWaiting ?? 0) >>> 0, true);
    dv.setUint8(c + CONFIG.handshake, config.handshake ?? 0);
    dv.setUint8(c + CONFIG.stream, config.stream ? 1 : 0);
    dv.setUint8(c + CONFIG.unordered, config.unordered ? 1 : 0);
    dv.setUint32(outPtr, 0, true);
    const status = this.#x.comline_connection_new(c, outPtr);
    // `new` allocates the handle, which may grow memory: read through a
    // fresh view.
    const handle = this.#view().getUint32(outPtr, true);
    return {
      status,
      connection: status === Status.OK ? new Connection(this, this.#x, handle) : null,
    };
  }

  // ── for Connection ──────────────────────────────────────────────────────

  /** `{ status, written, bytes }` from a `*written` at `writtenPtr` and
   * bytes at `bytesPtr`. */
  #written(status, writtenPtr, bytesPtr) {
    const written = this.#view().getUint32(writtenPtr, true);
    const result = { status, written };
    if (status === Status.OK) {
      result.bytes = new Uint8Array(this.#x.memory.buffer, bytesPtr, written).slice();
    }
    return result;
  }

  /** @internal */
  _start(handle, cap) {
    const { outPtr } = this.#regions(0, 8 + cap);
    const status = this.#x.comline_connection_start(handle, outPtr + 8, cap, outPtr);
    return this.#written(status, outPtr, outPtr + 8);
  }

  /** @internal `watch` bytes after the event are checked untouched
   * (`tailIntact`), for the layout check. */
  _feed(handle, input, watch = 0) {
    const i = bytesOf(input);
    const { inPtr, outPtr, u8 } = this.#regions(i.length, 8 + EVENT_SIZE + watch);
    u8.set(i, inPtr);
    const evPtr = outPtr + 8;
    u8.fill(0, evPtr, evPtr + EVENT_SIZE);
    u8.fill(0xa5, evPtr + EVENT_SIZE, evPtr + EVENT_SIZE + watch);
    const status = this.#x.comline_connection_feed(handle, inPtr, i.length, outPtr, evPtr);
    const view = this.#view();
    const after = new Uint8Array(this.#x.memory.buffer, evPtr + EVENT_SIZE, watch);
    return {
      status,
      consumed: view.getUint32(outPtr, true),
      event: readEvent(view, evPtr),
      ...(watch ? { tailIntact: after.every((b) => b === 0xa5) } : {}),
    };
  }

  /** @internal */
  _encodeRequest(handle, protocol, callId, name, expectsReply, body, cap) {
    const n = bytesOf(name);
    const b = bytesOf(body);
    const { inPtr, outPtr, u8, dv } = this.#regions(Comline.#size([n, b]), 16 + cap);
    const [nPtr, bPtr] = Comline.#place(u8, inPtr, [n, b]);
    dv.setBigUint64(outPtr + 8, 0n, true);
    const reply = typeof expectsReply === "boolean" ? (expectsReply ? 1 : 0) : expectsReply;
    const status = this.#x.comline_connection_encode_request(
      handle, protocol, callId, nPtr, n.length, reply, bPtr, b.length,
      outPtr + 16, cap, outPtr, outPtr + 8,
    );
    const result = this.#written(status, outPtr, outPtr + 16);
    if (status === Status.OK) {
      result.requestId = this.#view().getBigUint64(outPtr + 8, true);
    }
    return result;
  }

  /** @internal */
  _encodeReply(handle, protocol, requestId, envelope, errorId, body, cap) {
    const b = bytesOf(body);
    const { inPtr, outPtr, u8 } = this.#regions(b.length, 8 + cap);
    u8.set(b, inPtr);
    const status = this.#x.comline_connection_encode_reply(
      handle, protocol, BigInt.asUintN(64, BigInt(requestId)), envelope, errorId,
      inPtr, b.length, outPtr + 8, cap, outPtr,
    );
    return this.#written(status, outPtr, outPtr + 8);
  }

  /** @internal */
  _isWaiting(handle) {
    const { outPtr, dv } = this.#regions(0, 1);
    dv.setUint8(outPtr, 0xff);
    const status = this.#x.comline_connection_is_waiting(handle, outPtr);
    const waiting = this.#view().getUint8(outPtr);
    return { status, waiting: status === Status.OK ? waiting === 1 : undefined };
  }
}

/** One end of a connection: a `comline_connection *`. Free it with
 * `free()`; nothing frees it for you. */
export class Connection {
  #lib;
  #x;
  #handle;

  /** @internal Made by `Comline.connectionNew`. */
  constructor(lib, exports, handle) {
    this.#lib = lib;
    this.#x = exports;
    this.#handle = handle;
  }

  /** @internal The `comline_connection *`, 0 once freed. */
  get handle() {
    return this.#handle;
  }

  /** `comline_connection_start`: `{ status, written, bytes }`. */
  start(cap) {
    return this.#lib._start(this.#live(), cap);
  }

  /**
   * `comline_connection_feed`: `{ status, consumed, event }`. Spans in
   * `event` are offsets into `input`.
   */
  feed(input) {
    return this.#lib._feed(this.#live(), input);
  }

  /**
   * `comline_connection_encode_request`: `{ status, written, bytes,
   * requestId }`; `requestId` (a BigInt) and `bytes` on `OK`.
   */
  encodeRequest(protocol, callId, name, expectsReply, body, cap) {
    return this.#lib._encodeRequest(this.#live(), protocol, callId, name, expectsReply, body, cap);
  }

  /** `comline_connection_encode_reply`: `{ status, written, bytes }`. */
  encodeReply(protocol, requestId, envelope, errorId, body, cap) {
    return this.#lib._encodeReply(this.#live(), protocol, requestId, envelope, errorId, body, cap);
  }

  /** `comline_connection_abandon`: the status. */
  abandon(requestId) {
    return this.#x.comline_connection_abandon(this.#live(), BigInt.asUintN(64, BigInt(requestId)));
  }

  /** `comline_connection_is_waiting`: `{ status, waiting }`. */
  isWaiting() {
    return this.#lib._isWaiting(this.#live());
  }

  /** `comline_connection_free`. The connection is unusable afterwards;
   * freeing twice is a no-op here. */
  free() {
    if (this.#handle !== 0) {
      this.#x.comline_connection_free(this.#handle);
      this.#handle = 0;
    }
  }

  #live() {
    if (this.#handle === 0) throw new Error("comline: the connection was freed");
    return this.#handle;
  }
}

/** A `comline_event` at `ptr`. */
function readEvent(dv, ptr) {
  const span = (at) => ({
    offset: dv.getUint32(ptr + at, true),
    len: dv.getUint32(ptr + at + 4, true),
  });
  return {
    kind: dv.getUint8(ptr + EVENT.kind),
    flags: dv.getUint8(ptr + EVENT.flags),
    protocol: dv.getUint16(ptr + EVENT.protocol, true),
    callId: dv.getUint16(ptr + EVENT.callId, true),
    envelope: dv.getUint8(ptr + EVENT.envelope),
    errorId: dv.getUint16(ptr + EVENT.errorId, true),
    callName: span(EVENT.callName),
    body: span(EVENT.body),
    frame: span(EVENT.frame),
    capabilities: dv.getUint32(ptr + EVENT.capabilities, true),
    requestId: dv.getBigUint64(ptr + EVENT.requestId, true),
  };
}

const big = (_, v) => (typeof v === "bigint" ? v.toString() : v);
