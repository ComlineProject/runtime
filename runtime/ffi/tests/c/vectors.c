/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

/* The conformance vectors, replayed from C through `comline.h`: the cases
 * `examples/export_vectors.rs` writes out, with the checks
 * `tests/vectors.rs` makes from Rust, against the staticlib or the cdylib
 * (whichever this is linked with). The export's module doc gives the line
 * format and what each op does; each `case_*` below follows the function of
 * the same name in `tests/vectors.rs`.
 *
 *     vectors <dir>
 *
 * reads `<dir>/<feature>.txt` for every feature, prints each file's count,
 * and exits non-zero on any mismatch, on a malformed line, or when a file
 * holds a number of cases other than its header's. Plain C11, no POSIX. */

#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "comline.h"

/* ── layouts ─────────────────────────────────────────────────────────────
 * The numbers `src/lib.rs` holds the Rust types to, from this side. */

_Static_assert(sizeof(comline_span) == 8, "comline_span size");
_Static_assert(offsetof(comline_span, offset) == 0, "comline_span.offset");
_Static_assert(offsetof(comline_span, len) == 4, "comline_span.len");

_Static_assert(sizeof(comline_event) == 48, "comline_event size");
_Static_assert(_Alignof(comline_event) == 8, "comline_event align");
_Static_assert(offsetof(comline_event, kind) == 0, "comline_event.kind");
_Static_assert(offsetof(comline_event, flags) == 1, "comline_event.flags");
_Static_assert(offsetof(comline_event, protocol) == 2, "comline_event.protocol");
_Static_assert(offsetof(comline_event, call_id) == 4, "comline_event.call_id");
_Static_assert(offsetof(comline_event, envelope) == 6, "comline_event.envelope");
_Static_assert(offsetof(comline_event, _reserved) == 7, "comline_event._reserved");
_Static_assert(offsetof(comline_event, error_id) == 8, "comline_event.error_id");
_Static_assert(offsetof(comline_event, _reserved2) == 10, "comline_event._reserved2");
_Static_assert(offsetof(comline_event, call_name) == 12, "comline_event.call_name");
_Static_assert(offsetof(comline_event, body) == 20, "comline_event.body");
_Static_assert(offsetof(comline_event, frame) == 28, "comline_event.frame");
_Static_assert(offsetof(comline_event, capabilities) == 36, "comline_event.capabilities");
_Static_assert(offsetof(comline_event, request_id) == 40, "comline_event.request_id");

_Static_assert(sizeof(comline_handshake) == 32, "comline_handshake size");
_Static_assert(offsetof(comline_handshake, ir_hash) == 0, "comline_handshake.ir_hash");
_Static_assert(offsetof(comline_handshake, wire_format) == 8, "comline_handshake.wire_format");
_Static_assert(offsetof(comline_handshake, framing) == 16, "comline_handshake.framing");
_Static_assert(offsetof(comline_handshake, capabilities) == 24, "comline_handshake.capabilities");

/* `comline_config` holds pointers, so its layout follows the pointer
 * width. */
#define P sizeof(void *)
_Static_assert(sizeof(size_t) == P, "size_t is pointer-sized");
_Static_assert(offsetof(comline_config, ir_hash) == 0, "comline_config.ir_hash");
_Static_assert(offsetof(comline_config, wire_format) == 8, "comline_config.wire_format");
_Static_assert(offsetof(comline_config, wire_format_len) == 8 + P, "comline_config.wire_format_len");
_Static_assert(offsetof(comline_config, framing) == 8 + 2 * P, "comline_config.framing");
_Static_assert(offsetof(comline_config, framing_len) == 8 + 3 * P, "comline_config.framing_len");
_Static_assert(offsetof(comline_config, max_frame) == 8 + 4 * P, "comline_config.max_frame");
_Static_assert(offsetof(comline_config, capabilities) == 16 + 4 * P, "comline_config.capabilities");
_Static_assert(offsetof(comline_config, max_waiting) == 20 + 4 * P, "comline_config.max_waiting");
_Static_assert(offsetof(comline_config, handshake) == 24 + 4 * P, "comline_config.handshake");
_Static_assert(offsetof(comline_config, stream) == 25 + 4 * P, "comline_config.stream");
_Static_assert(offsetof(comline_config, unordered) == 26 + 4 * P, "comline_config.unordered");
_Static_assert(offsetof(comline_config, _reserved) == 27 + 4 * P, "comline_config._reserved");
_Static_assert(sizeof(comline_config) == 32 + 4 * P, "comline_config size");
_Static_assert(_Alignof(comline_config) == 8, "comline_config align");
#undef P

_Static_assert(COMLINE_PREFIX_LEN == 4, "the stream prefix is a u32");
_Static_assert(COMLINE_MAX_FRAME_CEILING == 4294967295u - COMLINE_PREFIX_LEN, "the frame ceiling");
_Static_assert(COMLINE_DEFAULT_MAX_FRAME <= COMLINE_MAX_FRAME_CEILING, "the default frame limit");
_Static_assert(COMLINE_HANDSHAKE_LEN == 31, "the comline@1 frame");

/* ── the files ─────────────────────────────────────────────────────────── */

static const char *const FILES[] = {
    "name-hash",         "handshake",         "envelope",
    "framing-datagram",  "framing-datagram2", "framing-jsonrpc",
};
#define NFILES (sizeof FILES / sizeof FILES[0])

/* The export's lines are under 4096 bytes. */
#define LINE_MAX_LEN 8192
#define MAX_FIELDS 16

/* How a case went through the ABI, as `tests/vectors.rs` counts it. 0 is
 * a failure. */
enum ran { FAILED = 0, FULL, ID_PATCHED, REJECTED_AS_OTHER_KIND, NOT_MAPPED };

/* The case being replayed, for messages. */
static const char *case_id = "";

#define CHECK(cond)                                                            \
    do {                                                                       \
        if (!(cond)) {                                                         \
            fprintf(stderr, "%s: failed: %s (vectors.c:%d)\n", case_id, #cond, \
                    __LINE__);                                                 \
            return FAILED;                                                     \
        }                                                                      \
    } while (0)

/* ── fields ────────────────────────────────────────────────────────────── */

/* Bytes from a field, owned. `p` is never null, so an empty input is a
 * real (zero-length) buffer. */
typedef struct {
    uint8_t *p;
    size_t n;
} bytes;

static void bytes_free(bytes *b) {
    free(b->p);
    b->p = NULL;
    b->n = 0;
}

static int hex_digit(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    return -1;
}

/* A hex field (`-` is empty) into `*out`. */
static int parse_bytes(const char *field, bytes *out) {
    size_t len = strcmp(field, "-") == 0 ? 0 : strlen(field);
    if (len % 2 != 0) return 0;
    out->n = len / 2;
    out->p = malloc(out->n ? out->n : 1);
    if (!out->p) return 0;
    for (size_t i = 0; i < out->n; i++) {
        int hi = hex_digit(field[2 * i]), lo = hex_digit(field[2 * i + 1]);
        if (hi < 0 || lo < 0) {
            bytes_free(out);
            return 0;
        }
        out->p[i] = (uint8_t)(hi << 4 | lo);
    }
    return 1;
}

/* A decimal field up to `max`. */
static int parse_u64(const char *field, uint64_t max, uint64_t *out) {
    uint64_t v = 0;
    if (!*field) return 0;
    for (const char *c = field; *c; c++) {
        if (*c < '0' || *c > '9') return 0;
        unsigned d = (unsigned)(*c - '0');
        if (v > (UINT64_MAX - d) / 10) return 0;
        v = v * 10 + d;
    }
    if (v > max) return 0;
    *out = v;
    return 1;
}

static void print_hex(const char *label, const uint8_t *p, size_t n) {
    fprintf(stderr, "  %s: ", label);
    for (size_t i = 0; i < n; i++) fprintf(stderr, "%02x", p[i]);
    if (n == 0) fputc('-', stderr);
    fputc('\n', stderr);
}

/* `got` is `want`, byte for byte; both are printed when not. */
static int same(const uint8_t *got, size_t got_len, const bytes *want) {
    if (got_len == want->n && memcmp(got, want->p, got_len) == 0) return 1;
    fprintf(stderr, "%s: bytes differ\n", case_id);
    print_hex("got ", got, got_len);
    print_hex("want", want->p, want->n);
    return 0;
}

/* The bytes a span names in `input`, when it lies inside it. */
static const uint8_t *at(const bytes *input, comline_span span) {
    if ((uint64_t)span.offset + span.len > input->n) return NULL;
    return input->p + span.offset;
}

/* The span names exactly `want` in `input`. */
static int span_is(const bytes *input, comline_span span, const uint8_t *want, size_t want_len) {
    const uint8_t *got = at(input, span);
    if (!got) {
        fprintf(stderr, "%s: span %u+%u outside a %zu-byte input\n", case_id, span.offset,
                span.len, input->n);
        return 0;
    }
    if (span.len == want_len && memcmp(got, want, want_len) == 0) return 1;
    fprintf(stderr, "%s: span differs\n", case_id);
    print_hex("got ", got, span.len);
    print_hex("want", want, want_len);
    return 0;
}

/* ── the ABI, wrapped as tests/common/mod.rs wraps it ──────────────────── */

/* A connection's setup: `Setup::plain` is no handshake, message mode, room
 * for 16 waiting calls. */
typedef struct {
    const char *framing;
    const char *wire_format;
    uint64_t ir_hash;
    uint32_t capabilities;
    uint8_t handshake;
} setup;

static setup plain(const char *framing) {
    setup s;
    s.framing = framing;
    s.wire_format = "json@1";
    s.ir_hash = 0x0102030405060708u;
    s.capabilities = 0;
    s.handshake = COMLINE_HANDSHAKE_NONE;
    return s;
}

/* `comline_connection_new`; on an error `*out` must be null. */
static comline_status conn_new(const setup *s, comline_connection **out) {
    comline_config cfg;
    memset(&cfg, 0, sizeof cfg);
    cfg.ir_hash = s->ir_hash;
    cfg.wire_format = (const uint8_t *)s->wire_format;
    cfg.wire_format_len = strlen(s->wire_format);
    cfg.framing = (const uint8_t *)s->framing;
    cfg.framing_len = strlen(s->framing);
    cfg.max_frame = 0;
    cfg.capabilities = s->capabilities;
    cfg.max_waiting = 16;
    cfg.handshake = s->handshake;
    cfg.stream = 0;
    cfg.unordered = 0;
    *out = NULL;
    comline_status status = comline_connection_new(&cfg, out);
    if (status == COMLINE_OK && *out == NULL) {
        fprintf(stderr, "%s: new gave OK and a null handle\n", case_id);
        return -1;
    }
    if (status != COMLINE_OK && *out != NULL) {
        fprintf(stderr, "%s: an error left *out set\n", case_id);
        return -1;
    }
    return status;
}

/* What one `feed` call gave. */
typedef struct {
    comline_status status;
    size_t consumed;
    comline_event ev;
} fed;

static fed feed(comline_connection *conn, const bytes *input) {
    fed f;
    memset(&f.ev, 0, sizeof f.ev);
    f.ev.kind = 0xff;
    f.consumed = SIZE_MAX;
    f.status = comline_connection_feed(conn, input->p, input->n, &f.consumed, &f.ev);
    return f;
}

/* What one `encode_*` or `start` call gave; `out` holds `written` bytes on
 * `COMLINE_OK`. */
#define OUT_CAP 4096
typedef struct {
    comline_status status;
    uint8_t out[OUT_CAP];
    size_t written;
    uint64_t request_id;
} encoded;

static void request(comline_connection *conn, uint16_t protocol, uint16_t call_id,
                    const uint8_t *name, size_t name_len, int expects_reply,
                    const uint8_t *body, size_t body_len, size_t cap, encoded *e) {
    e->written = SIZE_MAX;
    e->request_id = UINT64_MAX;
    e->status = comline_connection_encode_request(conn, protocol, call_id, name, name_len,
                                                  (uint8_t)expects_reply, body, body_len,
                                                  e->out, cap, &e->written, &e->request_id);
}

static void reply(comline_connection *conn, uint16_t protocol, uint64_t request_id,
                  uint8_t envelope, uint16_t error_id, const bytes *body, size_t cap,
                  encoded *e) {
    e->written = SIZE_MAX;
    e->request_id = 0;
    e->status = comline_connection_encode_reply(conn, protocol, request_id, envelope, error_id,
                                                body->p, body->n, e->out, cap, &e->written);
}

/* `*waiting` as 0 or 1, or -1 when the call fails. */
static int is_waiting(comline_connection *conn) {
    uint8_t waiting = 0xff;
    if (comline_connection_is_waiting(conn, &waiting) != COMLINE_OK) return -1;
    return waiting == 1 ? 1 : waiting == 0 ? 0 : -1;
}

/* Spends request ids with one-way calls until the next one is `id`. */
static int spend_ids(comline_connection *conn, uint64_t id) {
    static const uint8_t spend[] = "spend", null[] = "null";
    encoded e;
    for (uint64_t expected = 0; expected < id; expected++) {
        request(conn, 0, 0, spend, 5, 0, null, 4, 256, &e);
        if (e.status != COMLINE_OK || e.request_id != expected) {
            fprintf(stderr, "%s: spending id %llu gave %d, id %llu\n", case_id,
                    (unsigned long long)expected, (int)e.status,
                    (unsigned long long)e.request_id);
            return 0;
        }
    }
    return 1;
}

static uint64_t name_hash(const char *name) {
    uint64_t hash = 0;
    if (comline_name_hash((const uint8_t *)name, strlen(name), &hash) != COMLINE_OK) {
        fprintf(stderr, "%s: name_hash failed\n", case_id);
        exit(1);
    }
    return hash;
}

static int supports(const char *name) {
    return comline_supports((const uint8_t *)name, strlen(name)) == 1;
}

static comline_status handshake_encode(uint64_t ir_hash, const char *wire_format,
                                       const char *framing, uint32_t capabilities, size_t cap,
                                       encoded *e) {
    e->written = SIZE_MAX;
    e->status = comline_handshake_encode(ir_hash, (const uint8_t *)wire_format,
                                         strlen(wire_format), (const uint8_t *)framing,
                                         strlen(framing), capabilities, e->out, cap, &e->written);
    return e->status;
}

/* A method name span, unescaped through `comline_method_name_unescape`
 * when the event says it's escaped, is `want`. */
static int method_is(const bytes *input, const comline_event *ev, const bytes *want) {
    const uint8_t *spelled = at(input, ev->call_name);
    if (!spelled) return span_is(input, ev->call_name, want->p, want->n);
    if (!(ev->flags & COMLINE_EVENT_NAME_ESCAPED))
        return span_is(input, ev->call_name, want->p, want->n);
    size_t needed = SIZE_MAX;
    comline_status status =
        comline_method_name_unescape(spelled, ev->call_name.len, NULL, 0, &needed);
    if (status == COMLINE_OK) needed = 0;
    else if (status != COMLINE_BUFFER_TOO_SMALL) {
        fprintf(stderr, "%s: unescape gave %d\n", case_id, (int)status);
        return 0;
    }
    uint8_t *out = malloc(needed ? needed : 1);
    size_t written = SIZE_MAX;
    if (!out) return 0;
    status = comline_method_name_unescape(spelled, ev->call_name.len, out, needed, &written);
    int ok = status == COMLINE_OK && written == needed;
    if (!ok) fprintf(stderr, "%s: unescape gave %d, %zu bytes\n", case_id, (int)status, written);
    else {
        bytes got = {out, written};
        ok = same(got.p, got.n, want);
    }
    free(out);
    return ok;
}

/* A reply event holds `(envelope, error_id, body)`. */
static int reply_is(const bytes *input, const fed *f, uint8_t envelope, uint16_t error_id,
                    const bytes *body) {
    if (f->status != COMLINE_OK || f->ev.kind != COMLINE_EVENT_REPLY ||
        f->ev.envelope != envelope || f->ev.error_id != error_id ||
        (f->ev.flags & COMLINE_EVENT_BODY_ABSENT)) {
        fprintf(stderr, "%s: feed gave status %d, kind %u, envelope %u, error %u, flags %#x\n",
                case_id, (int)f->status, f->ev.kind, f->ev.envelope, f->ev.error_id,
                (unsigned)f->ev.flags);
        return 0;
    }
    return span_is(input, f->ev.body, body->p, body->n);
}

/* ── the cases ─────────────────────────────────────────────────────────── */

/* A case line's fields, after `<op> <id> <how>`. */
typedef struct {
    char *how;
    char **f;
    size_t n;
} line;

/* Field `i` as bytes, u64, u16 and so on; a malformed field fails the
 * case. */
#define FIELD_BYTES(var, i)                                                    \
    bytes var = {NULL, 0};                                                     \
    CHECK(parse_bytes(l->f[i], &var))
#define FIELD_U64(var, i, max)                                                 \
    uint64_t var = 0;                                                          \
    CHECK(parse_u64(l->f[i], (max), &var))

static enum ran case_name_hash(const line *l) {
    CHECK(l->n == 2 && strcmp(l->how, "exact") == 0);
    FIELD_BYTES(name, 0);
    FIELD_BYTES(want, 1);
    uint64_t hash = 0;
    comline_status status = comline_name_hash(name.p, name.n, &hash);
    uint8_t le[8];
    for (int i = 0; i < 8; i++) le[i] = (uint8_t)(hash >> (8 * i));
    int ok = status == COMLINE_OK && same(le, 8, &want);
    bytes_free(&name);
    bytes_free(&want);
    CHECK(ok);
    return FULL;
}

static enum ran handshake_reject(const bytes *frame) {
    encoded some;
    CHECK(handshake_encode(1, "json@1", "datagram@2", 0, 64, &some) == COMLINE_OK);
    comline_handshake unset, peer;
    memset(&unset, 0, sizeof unset);
    unset._reserved = 0xdead;
    peer = unset;
    comline_status status =
        comline_handshake_check(some.out, some.written, frame->p, frame->n, &peer);
    CHECK(status == COMLINE_ERR_HANDSHAKE);
    /* A frame that doesn't parse gives no peer. */
    CHECK(memcmp(&peer, &unset, sizeof peer) == 0);

    /* And on a connection, as the first frame fed. */
    setup s = plain("datagram@2");
    s.handshake = COMLINE_HANDSHAKE_COMLINE;
    comline_connection *conn;
    CHECK(conn_new(&s, &conn) == COMLINE_OK);
    fed first = feed(conn, frame);
    fed again = feed(conn, frame);
    comline_connection_free(conn);
    CHECK(first.status == COMLINE_ERR_HANDSHAKE);
    /* A refused handshake closes the connection. */
    CHECK(again.status == COMLINE_ERR_STATE);
    return FULL;
}

/* One end's frame against its peer's: `OK` when they agree, else
 * `ERR_HANDSHAKE`. */
static enum ran case_handshake_check(const line *l) {
    CHECK(l->n == 2);
    int reject = strcmp(l->how, "reject") == 0;
    CHECK(reject || strcmp(l->how, "exact") == 0);
    FIELD_BYTES(ours, 0);
    FIELD_BYTES(peer, 1);
    comline_status status = comline_handshake_check(ours.p, ours.n, peer.p, peer.n, NULL);
    bytes_free(&ours);
    bytes_free(&peer);
    CHECK(status == (reject ? COMLINE_ERR_HANDSHAKE : COMLINE_OK));
    return FULL;
}

static enum ran case_handshake(const line *l) {
    if (strcmp(l->how, "reject") == 0) {
        CHECK(l->n == 1);
        FIELD_BYTES(frame, 0);
        enum ran r = handshake_reject(&frame);
        bytes_free(&frame);
        return r;
    }
    CHECK(l->n == 6 && strcmp(l->how, "exact") == 0);
    FIELD_U64(ir_hash, 0, UINT64_MAX);
    const char *wire_format = l->f[1], *framing = l->f[2];
    FIELD_U64(capabilities, 3, UINT32_MAX);
    FIELD_BYTES(input, 4);
    FIELD_BYTES(want, 5);
    enum ran r = FAILED;
    comline_connection *conn = NULL;
    encoded e;
    uint32_t caps = (uint32_t)capabilities;

    /* Encode. */
    handshake_encode(ir_hash, wire_format, framing, caps, 30, &e);
    if (e.status != COMLINE_BUFFER_TOO_SMALL || e.written != 31) {
        fprintf(stderr, "%s: 30 bytes of room gave %d, %zu\n", case_id, (int)e.status, e.written);
        goto out;
    }
    if (handshake_encode(ir_hash, wire_format, framing, caps, 64, &e) != COMLINE_OK) {
        fprintf(stderr, "%s: encode gave %d\n", case_id, (int)e.status);
        goto out;
    }
    if (!same(e.out, e.written, &want)) goto out;

    /* Decode: the peer's frame read back holds the input. */
    comline_handshake peer;
    memset(&peer, 0xa5, sizeof peer);
    comline_status status = comline_handshake_check(e.out, e.written, input.p, input.n, &peer);
    if (status != COMLINE_OK || peer.ir_hash != ir_hash ||
        peer.wire_format != name_hash(wire_format) || peer.framing != name_hash(framing) ||
        peer.capabilities != caps || peer._reserved != 0) {
        fprintf(stderr, "%s: check gave %d and another peer\n", case_id, (int)status);
        goto out;
    }

    /* On a connection, when the framing is a connection framing. */
    if (supports(framing)) {
        setup s = plain(framing);
        s.handshake = COMLINE_HANDSHAKE_COMLINE;
        s.ir_hash = ir_hash;
        s.wire_format = wire_format;
        s.capabilities = caps;
        if (conn_new(&s, &conn) != COMLINE_OK) {
            fprintf(stderr, "%s: new failed\n", case_id);
            goto out;
        }
        e.written = SIZE_MAX;
        e.status = comline_connection_start(conn, e.out, 64, &e.written);
        if (e.status != COMLINE_OK) {
            fprintf(stderr, "%s: start gave %d\n", case_id, (int)e.status);
            goto out;
        }
        if (!same(e.out, e.written, &input)) goto out;
        fed f = feed(conn, &input);
        if (f.status != COMLINE_OK || f.ev.kind != COMLINE_EVENT_HANDSHAKE_OK ||
            f.ev.capabilities != caps || f.consumed != input.n) {
            fprintf(stderr, "%s: feed gave %d, kind %u\n", case_id, (int)f.status, f.ev.kind);
            goto out;
        }
    }
    r = FULL;
out:
    comline_connection_free(conn);
    bytes_free(&input);
    bytes_free(&want);
    return r;
}

/* A `datagram@2` connection waiting on request 0. */
static comline_connection *waiting_on_zero(void) {
    setup s = plain("datagram@2");
    comline_connection *conn;
    if (conn_new(&s, &conn) != COMLINE_OK) return NULL;
    encoded e;
    request(conn, 0, 0, (const uint8_t *)"", 0, 1, (const uint8_t *)"", 0, 64, &e);
    if (e.status != COMLINE_OK || e.request_id != 0) {
        fprintf(stderr, "%s: request gave %d, id %llu\n", case_id, (int)e.status,
                (unsigned long long)e.request_id);
        comline_connection_free(conn);
        return NULL;
    }
    return conn;
}

static enum ran case_envelope(const line *l) {
    if (strcmp(l->how, "reject") == 0) {
        CHECK(l->n == 1);
        FIELD_BYTES(frame, 0);
        comline_connection *conn = waiting_on_zero();
        fed f = {0, 0, {0}};
        if (conn) f = feed(conn, &frame);
        comline_connection_free(conn);
        bytes_free(&frame);
        CHECK(conn != NULL);
        CHECK(f.status == COMLINE_ERR_FRAMING);
        return FULL;
    }
    CHECK(l->n == 5 && strcmp(l->how, "exact") == 0);
    FIELD_U64(envelope, 0, 255);
    FIELD_U64(error_id, 1, UINT16_MAX);
    FIELD_BYTES(body, 2);
    FIELD_BYTES(frame, 3);
    FIELD_BYTES(want, 4);
    enum ran r = FAILED;
    comline_connection *conn = NULL, *waiting = NULL;
    encoded e;

    setup s = plain("datagram@2");
    if (conn_new(&s, &conn) != COMLINE_OK) goto out;
    reply(conn, 0, 0, (uint8_t)envelope, (uint16_t)error_id, &body, 256, &e);
    if (e.status != COMLINE_OK) {
        fprintf(stderr, "%s: encode gave %d\n", case_id, (int)e.status);
        goto out;
    }
    if (!same(e.out, e.written, &want)) goto out;

    waiting = waiting_on_zero();
    if (!waiting) goto out;
    fed f = feed(waiting, &frame);
    if (!reply_is(&frame, &f, (uint8_t)envelope, (uint16_t)error_id, &body)) goto out;
    r = FULL;
out:
    comline_connection_free(conn);
    comline_connection_free(waiting);
    bytes_free(&body);
    bytes_free(&frame);
    bytes_free(&want);
    return r;
}

/* A rejected request or response: `feed` refuses it as malformed, or reads
 * it as the other kind of frame. */
static enum ran reject_frame(const char *framing, const bytes *frame, int is_request) {
    setup s = plain(framing);
    comline_connection *conn;
    CHECK(conn_new(&s, &conn) == COMLINE_OK);
    fed f = feed(conn, frame);
    comline_connection_free(conn);
    if (f.status != COMLINE_OK) {
        CHECK(f.status == COMLINE_ERR_FRAMING);
        return FULL;
    }
    if (is_request) {
        CHECK(f.ev.kind != COMLINE_EVENT_REQUEST);
    } else {
        CHECK(f.ev.kind != COMLINE_EVENT_REPLY && f.ev.kind != COMLINE_EVENT_STALE);
    }
    return REJECTED_AS_OTHER_KIND;
}

static enum ran case_request(const line *l) {
    if (strcmp(l->how, "reject") == 0) {
        CHECK(l->n == 2 && supports(l->f[0]));
        FIELD_BYTES(frame, 1);
        enum ran r = reject_frame(l->f[0], &frame, 1);
        bytes_free(&frame);
        return r;
    }
    int patched = strcmp(l->how, "patched") == 0;
    CHECK(l->n == 10 && (patched || strcmp(l->how, "exact") == 0));
    const char *framing = l->f[0];
    int datagram2 = strcmp(framing, "datagram@2") == 0;
    CHECK(datagram2 || strcmp(framing, "jsonrpc@1") == 0);
    FIELD_U64(protocol, 1, UINT16_MAX);
    FIELD_U64(call_id, 2, UINT16_MAX);
    FIELD_U64(one_way, 4, 1);
    FIELD_U64(request_id, 5, UINT64_MAX);
    FIELD_U64(drive, 6, 1000);
    FIELD_BYTES(method, 3);
    FIELD_BYTES(params, 7);
    FIELD_BYTES(input, 8);
    FIELD_BYTES(want, 9);
    enum ran r = FAILED;
    comline_connection *conn = NULL, *peer = NULL;
    encoded e;
    setup s = plain(framing);

    /* Encode, at the vector's id when the connection can be driven to it,
     * else at 0, for which the export's bytes are. */
    if (conn_new(&s, &conn) != COMLINE_OK) goto out;
    if (!spend_ids(conn, drive)) goto out;
    request(conn, (uint16_t)protocol, (uint16_t)call_id, method.p, method.n, !one_way, params.p,
            params.n, OUT_CAP, &e);
    if (e.status != COMLINE_OK) {
        fprintf(stderr, "%s: encode gave %d\n", case_id, (int)e.status);
        goto out;
    }
    if (!one_way && e.request_id != drive) {
        fprintf(stderr, "%s: encoded as request %llu\n", case_id, (unsigned long long)e.request_id);
        goto out;
    }
    if (!same(e.out, e.written, &want)) goto out;
    /* A one-way call doesn't wait, and a call that expects a reply does. */
    if (is_waiting(conn) != !one_way) {
        fprintf(stderr, "%s: waiting is %d\n", case_id, is_waiting(conn));
        goto out;
    }

    /* Decode, on a fresh connection. */
    if (conn_new(&s, &peer) != COMLINE_OK) goto out;
    fed f = feed(peer, &input);
    if (f.status != COMLINE_OK || f.ev.kind != COMLINE_EVENT_REQUEST || f.consumed != input.n ||
        f.ev.request_id != request_id ||
        ((f.ev.flags & COMLINE_EVENT_ONE_WAY) != 0) != (one_way == 1)) {
        fprintf(stderr, "%s: feed gave %d, kind %u, consumed %zu, id %llu, flags %#x\n", case_id,
                (int)f.status, f.ev.kind, f.consumed, (unsigned long long)f.ev.request_id,
                (unsigned)f.ev.flags);
        goto out;
    }
    if (!span_is(&input, f.ev.frame, input.p, input.n)) goto out;
    if (!span_is(&input, f.ev.body, params.p, params.n)) goto out;
    if (datagram2) {
        if (!(f.ev.flags & COMLINE_EVENT_HAS_PROTOCOL) || (f.ev.flags & COMLINE_EVENT_CALL_BY_NAME) ||
            f.ev.protocol != protocol || f.ev.call_id != call_id) {
            fprintf(stderr, "%s: feed gave flags %#x, protocol %u, call %u\n", case_id,
                    (unsigned)f.ev.flags, f.ev.protocol, f.ev.call_id);
            goto out;
        }
    } else {
        if ((f.ev.flags & COMLINE_EVENT_HAS_PROTOCOL) || !(f.ev.flags & COMLINE_EVENT_CALL_BY_NAME)) {
            fprintf(stderr, "%s: feed gave flags %#x\n", case_id, (unsigned)f.ev.flags);
            goto out;
        }
        if (!method_is(&input, &f.ev, &method)) goto out;
    }
    r = patched ? ID_PATCHED : FULL;
out:
    comline_connection_free(conn);
    comline_connection_free(peer);
    bytes_free(&method);
    bytes_free(&params);
    bytes_free(&input);
    bytes_free(&want);
    return r;
}

static enum ran case_response(const line *l) {
    if (strcmp(l->how, "reject") == 0) {
        CHECK(l->n == 2 && supports(l->f[0]));
        FIELD_BYTES(frame, 1);
        enum ran r = reject_frame(l->f[0], &frame, 0);
        bytes_free(&frame);
        return r;
    }
    CHECK(l->n == 8 && strcmp(l->how, "exact") == 0);
    const char *framing = l->f[0];
    CHECK(supports(framing));
    FIELD_U64(protocol, 1, UINT16_MAX);
    /* A reply id too large to wait on isn't exported. */
    FIELD_U64(request_id, 2, 1000);
    FIELD_U64(envelope, 3, 255);
    FIELD_U64(error_id, 4, UINT16_MAX);
    FIELD_BYTES(body, 5);
    FIELD_BYTES(input, 6);
    FIELD_BYTES(want, 7);
    enum ran r = FAILED;
    comline_connection *conn = NULL, *caller = NULL;
    encoded e;
    setup s = plain(framing);

    /* Encode: a reply carries the peer's request id, so nothing to drive. */
    if (conn_new(&s, &conn) != COMLINE_OK) goto out;
    reply(conn, (uint16_t)protocol, request_id, (uint8_t)envelope, (uint16_t)error_id, &body,
          OUT_CAP, &e);
    if (e.status != COMLINE_OK) {
        fprintf(stderr, "%s: encode gave %d\n", case_id, (int)e.status);
        goto out;
    }
    if (!same(e.out, e.written, &want)) goto out;

    /* Decode, on a connection waiting on the vector's request id. */
    if (conn_new(&s, &caller) != COMLINE_OK) goto out;
    if (!spend_ids(caller, request_id)) goto out;
    request(caller, (uint16_t)protocol, 0, (const uint8_t *)"call", 4, 1,
            (const uint8_t *)"null", 4, 256, &e);
    if (e.status != COMLINE_OK || e.request_id != request_id) {
        fprintf(stderr, "%s: request gave %d, id %llu\n", case_id, (int)e.status,
                (unsigned long long)e.request_id);
        goto out;
    }
    fed f = feed(caller, &input);
    if (!reply_is(&input, &f, (uint8_t)envelope, (uint16_t)error_id, &body)) goto out;
    if (f.ev.request_id != request_id) {
        fprintf(stderr, "%s: the reply answers %llu\n", case_id, (unsigned long long)f.ev.request_id);
        goto out;
    }
    if (is_waiting(caller) != 0) {
        fprintf(stderr, "%s: still waiting\n", case_id);
        goto out;
    }
    r = FULL;
out:
    comline_connection_free(conn);
    comline_connection_free(caller);
    bytes_free(&body);
    bytes_free(&input);
    bytes_free(&want);
    return r;
}

/* `datagram@1`: no ABI operation does it, and the ABI refuses it. */
static enum ran case_unmapped(const line *l) {
    CHECK(l->n == 1 && strcmp(l->how, "refused") == 0);
    CHECK(!supports(l->f[0]));
    setup s = plain(l->f[0]);
    comline_connection *conn;
    comline_status status = conn_new(&s, &conn);
    if (status == COMLINE_OK) comline_connection_free(conn);
    CHECK(status == COMLINE_ERR_UNKNOWN_FRAMING);
    return NOT_MAPPED;
}

/* ── the replay ────────────────────────────────────────────────────────── */

/* Split `text` on single spaces into at most `max` fields. */
static size_t split(char *text, char **fields, size_t max) {
    size_t n = 0;
    char *c = text;
    while (n < max) {
        fields[n++] = c;
        c = strchr(c, ' ');
        if (!c) return n;
        *c++ = '\0';
    }
    return max + 1;
}

/* Read one line into `buf`, newline cut; 0 at the end or on a line too
 * long or unterminated. */
static int read_line(FILE *file, char *buf, size_t cap) {
    if (!fgets(buf, (int)cap, file)) return 0;
    size_t len = strlen(buf);
    if (len == 0 || buf[len - 1] != '\n') return 0;
    buf[len - 1] = '\0';
    return 1;
}

static unsigned long counts[5];

/* Replay one file; the number of failures, or one for a malformed file. */
static unsigned replay_file(const char *dir, const char *feature) {
    static char buf[LINE_MAX_LEN];
    char path[1024];
    snprintf(path, sizeof path, "%s/%s.txt", dir, feature);
    FILE *file = fopen(path, "r");
    if (!file) {
        fprintf(stderr, "%s: can't open\n", path);
        return 1;
    }
    char *header[MAX_FIELDS];
    uint64_t expected = 0;
    if (!read_line(file, buf, sizeof buf) || split(buf, header, MAX_FIELDS) != 4 ||
        strcmp(header[0], "comline-ffi-vectors") != 0 || strcmp(header[1], "1") != 0 ||
        strcmp(header[2], feature) != 0 || !parse_u64(header[3], 1u << 20, &expected)) {
        fprintf(stderr, "%s: not a version 1 export of %s\n", path, feature);
        fclose(file);
        return 1;
    }
    unsigned failures = 0;
    unsigned long replayed = 0, passed = 0;
    while (read_line(file, buf, sizeof buf)) {
        char *fields[MAX_FIELDS];
        size_t n = split(buf, fields, MAX_FIELDS);
        replayed++;
        if (n < 3 || n > MAX_FIELDS) {
            fprintf(stderr, "%s:%lu: malformed line\n", path, replayed + 1);
            failures++;
            continue;
        }
        line l = {fields[2], fields + 3, n - 3};
        case_id = fields[1];
        const char *op = fields[0];
        enum ran r = FAILED;
        if (strcmp(op, "name_hash") == 0) r = case_name_hash(&l);
        else if (strcmp(op, "handshake") == 0) r = case_handshake(&l);
        else if (strcmp(op, "handshake_check") == 0) r = case_handshake_check(&l);
        else if (strcmp(op, "envelope") == 0) r = case_envelope(&l);
        else if (strcmp(op, "request") == 0) r = case_request(&l);
        else if (strcmp(op, "response") == 0) r = case_response(&l);
        else if (strcmp(op, "unmapped") == 0) r = case_unmapped(&l);
        else fprintf(stderr, "%s: no op %s\n", case_id, op);
        if (r == FAILED) failures++;
        else {
            passed++;
            counts[r]++;
        }
    }
    if (!feof(file)) {
        fprintf(stderr, "%s: a line too long or unterminated after %lu cases\n", path, replayed);
        failures++;
    }
    fclose(file);
    if (replayed != expected) {
        fprintf(stderr, "%s: replayed %lu cases, the header says %llu\n", path, replayed,
                (unsigned long long)expected);
        failures++;
    }
    printf("%s: %lu/%llu passed\n", feature, passed, (unsigned long long)expected);
    return failures;
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s <dir with export_vectors' files>\n", argv[0]);
        return 2;
    }
    case_id = "abi";
    uint32_t version = comline_abi_version();
    if (version != ((uint32_t)COMLINE_ABI_MAJOR << 16 | COMLINE_ABI_MINOR)) {
        fprintf(stderr, "the library is ABI %u.%u, the header %u.%u\n", version >> 16,
                version & 0xffff, COMLINE_ABI_MAJOR, COMLINE_ABI_MINOR);
        return 1;
    }
    unsigned failures = 0;
    for (size_t i = 0; i < NFILES; i++) failures += replay_file(argv[1], FILES[i]);
    unsigned long total = counts[FULL] + counts[ID_PATCHED] + counts[REJECTED_AS_OTHER_KIND] +
                          counts[NOT_MAPPED];
    printf("%lu cases: %lu full, %lu id-patched, %lu rejected as the other kind, %lu not mapped\n",
           total, counts[FULL], counts[ID_PATCHED], counts[REJECTED_AS_OTHER_KIND],
           counts[NOT_MAPPED]);
    if (failures) {
        fprintf(stderr, "%u failures\n", failures);
        return 1;
    }
    return 0;
}
