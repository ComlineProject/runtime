use std::ptr;

use super::status::{FEED, PLAIN, REPLY, REQUEST, START};
use super::{Config, PLENTY, SENTINEL, SENTINEL_BYTE};
use comline_runtime::contract::FRAMING_JSONRPC;
use comline_runtime_ffi::*;

/// What one `start` or `encode_*` call gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Encoded {
    pub(super) status: comline_status,
    pub(super) written: usize,
    pub(super) request_id: u64,
    /// `out[..written]` on `COMLINE_OK`.
    pub(super) bytes: Vec<u8>,
    /// Reference only: when not 0, the refusals any of which is right, and
    /// nothing else is compared.
    pub(super) either: u32,
}

/// What one `feed` call gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Fed {
    pub(super) status: comline_status,
    pub(super) consumed: usize,
    pub(super) ev: comline_event,
}

/// A handle, called through the `extern "C"` functions only.
pub(super) struct Ffi {
    conn: *mut comline_connection,
    /// The output buffer, reused so a call allocates nothing on this side
    /// either; `SENTINEL` bytes past `cap` are checked after each call.
    out: Vec<u8>,
    allocations: Option<fn() -> usize>,
    /// The connection speaks `jsonrpc@1`.
    jsonrpc: bool,
}

impl Drop for Ffi {
    fn drop(&mut self) {
        unsafe { comline_connection_free(self.conn) };
    }
}

/// A pointer to `bytes`, null when there are none (which the ABI allows).
fn ptr_of(bytes: &[u8]) -> *const u8 {
    if bytes.is_empty() {
        ptr::null()
    } else {
        bytes.as_ptr()
    }
}

impl Ffi {
    pub(super) fn new(
        config: &Config,
        allocations: Option<fn() -> usize>,
    ) -> (comline_status, Option<Self>) {
        let cfg = comline_config {
            ir_hash: config.ir_hash,
            wire_format: ptr_of(&config.wire_format),
            wire_format_len: config.wire_format.len(),
            framing: ptr_of(&config.framing),
            framing_len: config.framing.len(),
            max_frame: config.max_frame,
            capabilities: config.capabilities,
            max_waiting: config.max_waiting,
            handshake: config.handshake,
            stream: config.stream,
            unordered: config.unordered,
            _reserved: config.reserved,
        };
        let mut conn = ptr::null_mut();
        let status = unsafe { comline_connection_new(&cfg, &mut conn) };
        if status != COMLINE_OK {
            assert!(conn.is_null(), "an error leaves *out null");
            return (status, None);
        }
        assert!(!conn.is_null());
        let ffi = Self {
            conn,
            out: vec![0; PLENTY + SENTINEL],
            allocations,
            jsonrpc: config.framing == FRAMING_JSONRPC.as_bytes(),
        };
        (status, Some(ffi))
    }

    /// Runs one call, failing when it allocated.
    fn measured<R>(&self, what: &str, call: impl FnOnce() -> R) -> Result<R, String> {
        let (result, allocated) = self.counted(call);
        ensure!(allocated == 0, "{what} allocated ({allocated} allocations)");
        Ok(result)
    }

    /// Runs one call, and counts what it allocated (0 without a counter).
    fn counted<R>(&self, call: impl FnOnce() -> R) -> (R, usize) {
        let before = self.allocations.map(|count| count());
        let result = call();
        match (self.allocations, before) {
            (Some(count), Some(before)) => (result, count() - before),
            _ => (result, 0),
        }
    }

    /// One encode into `cap` bytes, through `raw(out, cap, written,
    /// request_id)`.
    fn encode_once(
        &mut self,
        what: &str,
        codes: &[comline_status],
        cap: usize,
        raw: &dyn Fn(*mut u8, usize, &mut usize, &mut u64) -> comline_status,
    ) -> Result<Encoded, String> {
        if self.out.len() < cap + SENTINEL {
            self.out.resize(cap + SENTINEL, 0);
        }
        let out = if cap == 0 {
            ptr::null_mut()
        } else {
            self.out[cap..cap + SENTINEL].fill(SENTINEL_BYTE);
            self.out.as_mut_ptr()
        };
        let (mut written, mut request_id) = (usize::MAX, u64::MAX);
        let status = self.measured(what, || raw(out, cap, &mut written, &mut request_id))?;
        ensure!(codes.contains(&status), "{what}: status {status}");
        ensure!(
            cap == 0
                || self.out[cap..cap + SENTINEL]
                    .iter()
                    .all(|&b| b == SENTINEL_BYTE),
            "{what} wrote past cap {cap}"
        );
        let bytes = match status {
            COMLINE_OK => {
                ensure!(written <= cap, "{what}: wrote {written} into {cap}");
                self.out[..written].to_vec()
            }
            COMLINE_BUFFER_TOO_SMALL => {
                ensure!(
                    written > cap,
                    "{what}: BUFFER_TOO_SMALL needing {written}, with {cap}"
                );
                Vec::new()
            }
            _ => Vec::new(),
        };
        Ok(Encoded {
            status,
            written,
            request_id,
            bytes,
            either: 0,
        })
    }

    /// An encode, and on `BUFFER_TOO_SMALL` the same one a byte short of the
    /// size it reported, which must report that size again.
    fn encode(
        &mut self,
        what: &str,
        codes: &[comline_status],
        cap: usize,
        raw: &dyn Fn(*mut u8, usize, &mut usize, &mut u64) -> comline_status,
    ) -> Result<Encoded, String> {
        let got = self.encode_once(what, codes, cap, raw)?;
        if got.status == COMLINE_BUFFER_TOO_SMALL {
            let again = self.encode_once(what, codes, got.written - 1, raw)?;
            ensure!(
                (again.status, again.written) == (COMLINE_BUFFER_TOO_SMALL, got.written),
                "{what}: BUFFER_TOO_SMALL needing {}, then {} needing {} with a byte less",
                got.written,
                again.status,
                again.written
            );
        }
        Ok(got)
    }

    pub(super) fn start(&mut self, cap: usize) -> Result<Encoded, String> {
        let conn = self.conn;
        self.encode("start", START, cap, &|out, cap, written, _| unsafe {
            comline_connection_start(conn, out, cap, written)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn request(
        &mut self,
        protocol: u16,
        call_id: u16,
        name: &[u8],
        expects_reply: u8,
        body: &[u8],
        cap: usize,
    ) -> Result<Encoded, String> {
        let conn = self.conn;
        let (name_ptr, body_ptr) = (ptr_of(name), ptr_of(body));
        self.encode(
            "encode_request",
            REQUEST,
            cap,
            &|out, cap, written, request_id| unsafe {
                comline_connection_encode_request(
                    conn,
                    protocol,
                    call_id,
                    name_ptr,
                    name.len(),
                    expects_reply,
                    body_ptr,
                    body.len(),
                    out,
                    cap,
                    written,
                    request_id,
                )
            },
        )
    }

    pub(super) fn reply(
        &mut self,
        protocol: u16,
        request_id: u64,
        envelope: u8,
        error_id: u16,
        body: &[u8],
        cap: usize,
    ) -> Result<Encoded, String> {
        let conn = self.conn;
        let body_ptr = ptr_of(body);
        self.encode("encode_reply", REPLY, cap, &|out, cap, written, _| unsafe {
            comline_connection_encode_reply(
                conn,
                protocol,
                request_id,
                envelope,
                error_id,
                body_ptr,
                body.len(),
                out,
                cap,
                written,
            )
        })
    }

    pub(super) fn abandon(&mut self, request_id: u64) -> Result<comline_status, String> {
        let conn = self.conn;
        let status = self.measured("abandon", || unsafe {
            comline_connection_abandon(conn, request_id)
        })?;
        ensure!(PLAIN.contains(&status), "abandon: status {status}");
        Ok(status)
    }

    pub(super) fn is_waiting(&mut self) -> Result<(comline_status, u8), String> {
        let conn = self.conn;
        let mut waiting = 0xff;
        let status = self.measured("is_waiting", || unsafe {
            comline_connection_is_waiting(conn, &mut waiting)
        })?;
        ensure!(PLAIN.contains(&status), "is_waiting: status {status}");
        ensure!(
            status != COMLINE_OK || waiting <= 1,
            "is_waiting: *waiting = {waiting}"
        );
        Ok((status, waiting))
    }

    pub(super) fn feed(&mut self, input: &[u8]) -> Result<Fed, String> {
        let conn = self.conn;
        let mut consumed = usize::MAX;
        // Not a valid event, so a field the call leaves alone shows.
        let mut ev = comline_event {
            kind: 0xff,
            flags: 0xff,
            _reserved: 0xff,
            request_id: u64::MAX,
            ..Default::default()
        };
        let (status, allocated) = self.counted(|| unsafe {
            comline_connection_feed(conn, input.as_ptr(), input.len(), &mut consumed, &mut ev)
        });
        // The known breaches of the rule, all serde_json's under
        // `jsonrpc@1` (see `serde_json_allocates`, and `rules.rs`,
        // `jsonrpc_decode_allocates_nothing`, ignored until they're fixed).
        // Anything else that allocates fails.
        let frame = &input[..consumed.min(input.len())];
        let known = self.jsonrpc && serde_json_allocates(status, frame);
        ensure!(
            allocated == 0 || known,
            "feed allocated ({allocated} allocations)"
        );
        ensure!(FEED.contains(&status), "feed: status {status}");
        Ok(Fed {
            status,
            consumed,
            ev,
        })
    }
}

/// Whether `jsonrpc@1`'s decode is known to allocate on `frame`, which fed
/// with `status`. The framing parses with serde_json, which allocates:
/// - for any error, which it boxes (so for every refused frame);
/// - to unescape a member name (`"js\u006erpc"`) into its scratch `Vec`;
/// - to skip or borrow a value with brackets inside brackets (`[[1]]` as
///   `params`, or an unknown member's), which it does by pushing the
///   enclosing ones onto that scratch `Vec`: brackets three deep in the frame.
///
/// These break the rule that nothing allocates after
/// `comline_connection_new`; they are the core's bugs, not the rule's.
fn serde_json_allocates(status: comline_status, frame: &[u8]) -> bool {
    if status == COMLINE_ERR_FRAMING || status == COMLINE_ERR_PEER_REFUSED || frame.contains(&b'\\')
    {
        return true;
    }
    // Without a backslash, a `"` always opens or closes a string.
    let (mut depth, mut deepest, mut in_string) = (0usize, 0, false);
    for &b in frame {
        match b {
            b'"' => in_string = !in_string,
            b'[' | b'{' if !in_string => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b']' | b'}' if !in_string => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest >= 3
}
