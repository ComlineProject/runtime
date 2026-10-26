// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// Types for `comline.mjs`, the JS glue over the wasm32 build of
// `comline-runtime-ffi` (`.d.mts`, the name TypeScript looks for beside a
// `.mjs`). Each method is one `comline.h` function; see there for what it
// does and when it fails.

export declare const ABI_MAJOR: 0;
export declare const ABI_MINOR: 1;

/** `comline_status`. Below 16 is an outcome, 16 and up an error; a number
 * not listed is a code from a newer library. */
export declare const Status: Readonly<{
  OK: 0;
  NEED_MORE: 1;
  BUFFER_TOO_SMALL: 2;
  ERR_FRAMING: 16;
  ERR_HANDSHAKE: 17;
  ERR_FRAME_TOO_LARGE: 18;
  ERR_UNKNOWN_FRAMING: 19;
  ERR_INVALID_ARGUMENT: 20;
  ERR_STATE: 21;
  ERR_POISONED: 22;
  ERR_BUSY: 23;
}>;
export type Status = number;

export declare const Handshake: Readonly<{ COMLINE: 0; NONE: 1 }>;
export declare const EventKind: Readonly<{ HANDSHAKE_OK: 0; REQUEST: 1; REPLY: 2; STALE: 3 }>;
export declare const EventFlag: Readonly<{
  HAS_PROTOCOL: 1;
  ONE_WAY: 2;
  CALL_BY_NAME: 4;
  BODY_ABSENT: 8;
  NAME_ESCAPED: 16;
}>;
export declare const Envelope: Readonly<{ OK: 0; ERR: 1; FAULT: 2 }>;

export declare const DEFAULT_MAX_FRAME: number;
export declare const MAX_FRAME_CEILING: number;
export declare const PREFIX_LEN: 4;
export declare const HANDSHAKE_LEN: 31;

/** A name or a body: a string is encoded as UTF-8, bytes go as they are. */
export type Bytes = string | Uint8Array;

/** `comline_config`. A field left out is 0, as in a zeroed struct. */
export interface Config {
  irHash?: bigint | number;
  wireFormat?: Bytes;
  framing?: Bytes;
  maxFrame?: bigint | number;
  capabilities?: number;
  maxWaiting?: number;
  /** `Handshake.COMLINE` (0, the default) or `Handshake.NONE`. */
  handshake?: number;
  stream?: boolean;
  unordered?: boolean;
}

/** `comline_span`: a part of the input fed. */
export interface Span {
  offset: number;
  len: number;
}

/** `comline_event`. */
export interface Event {
  kind: number;
  flags: number;
  protocol: number;
  callId: number;
  envelope: number;
  errorId: number;
  callName: Span;
  body: Span;
  frame: Span;
  capabilities: number;
  requestId: bigint;
}

/** A decoded `comline@1` frame, names hashed. */
export interface Peer {
  irHash: bigint;
  wireFormat: bigint;
  framing: bigint;
  capabilities: number;
}

/** An `out` / `cap` / `*written` call. `written` is the size needed on
 * `BUFFER_TOO_SMALL`; `bytes`, a copy, is there on `OK`. */
export interface Written {
  status: Status;
  written: number;
  bytes?: Uint8Array;
}

export interface Fed {
  status: Status;
  consumed: number;
  event: Event;
}

/** Compile (unless given a module) and instantiate the wasm32 build, then
 * check its ABI version and the `comline_event` layout. */
export declare function instantiate(source: BufferSource | WebAssembly.Module): Promise<Comline>;

export declare class Comline {
  private constructor();
  abiVersion(): number;
  supports(name: Bytes): boolean;
  nameHash(name: Bytes): { status: Status; hash?: bigint };
  handshakeEncode(
    irHash: bigint | number,
    wireFormat: Bytes,
    framing: Bytes,
    capabilities: number,
    cap: number,
  ): Written;
  /** `peer` is null when the ABI didn't write it (the frame didn't parse). */
  handshakeCheck(ours: Uint8Array, peer: Uint8Array): { status: Status; peer: Peer | null };
  methodNameUnescape(spelled: Uint8Array, cap: number): Written;
  connectionNew(config: Config): { status: Status; connection: Connection | null };
}

/** A `comline_connection *`. Free it with `free()`; nothing frees it for
 * you. */
export declare class Connection {
  private constructor();
  start(cap: number): Written;
  feed(input: Uint8Array): Fed;
  encodeRequest(
    protocol: number,
    callId: number,
    name: Bytes,
    expectsReply: boolean | number,
    body: Bytes,
    cap: number,
  ): Written & { requestId?: bigint };
  encodeReply(
    protocol: number,
    requestId: bigint | number,
    envelope: number,
    errorId: number,
    body: Bytes,
    cap: number,
  ): Written;
  abandon(requestId: bigint | number): Status;
  isWaiting(): { status: Status; waiting?: boolean };
  free(): void;
}
