// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A thin owner over the `extern "C"` functions, so tests read as calls.
//! Everything goes through the C ABI; nothing here touches
//! `comline-runtime`.

#![allow(dead_code)]

pub mod cases;

use std::ptr;

use comline_runtime_ffi::*;

/// A connection's setup, in Rust terms.
#[derive(Debug, Clone, Copy)]
pub struct Setup {
    pub framing: &'static str,
    pub wire_format: &'static str,
    pub ir_hash: u64,
    pub capabilities: u32,
    pub handshake: u8,
    pub stream: bool,
    pub unordered: bool,
    pub max_waiting: u32,
    pub max_frame: u64,
}

impl Setup {
    /// No handshake, message mode, room for 16 waiting calls.
    pub fn plain(framing: &'static str) -> Self {
        Self {
            framing,
            wire_format: "json@1",
            ir_hash: 0x0102_0304_0506_0708,
            capabilities: 0,
            handshake: COMLINE_HANDSHAKE_NONE,
            stream: false,
            unordered: false,
            max_waiting: 16,
            max_frame: 0,
        }
    }

    pub fn config(&self) -> comline_config {
        comline_config {
            ir_hash: self.ir_hash,
            wire_format: self.wire_format.as_ptr(),
            wire_format_len: self.wire_format.len(),
            framing: self.framing.as_ptr(),
            framing_len: self.framing.len(),
            max_frame: self.max_frame,
            capabilities: self.capabilities,
            max_waiting: self.max_waiting,
            handshake: self.handshake,
            stream: u8::from(self.stream),
            unordered: u8::from(self.unordered),
            _reserved: 0,
        }
    }
}

/// What one `encode_*` or `start` call gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub status: comline_status,
    /// `out`, cut to `*written` on `COMLINE_OK`.
    pub bytes: Vec<u8>,
    pub written: usize,
    pub request_id: u64,
}

/// What one `feed` call gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fed {
    pub status: comline_status,
    pub consumed: usize,
    pub ev: comline_event,
}

pub struct Conn(pub *mut comline_connection);

impl Conn {
    pub fn new(setup: &Setup) -> Result<Self, comline_status> {
        Self::from_config(&setup.config())
    }

    pub fn from_config(config: &comline_config) -> Result<Self, comline_status> {
        let mut conn = ptr::null_mut();
        let status = unsafe { comline_connection_new(config, &mut conn) };
        if status == COMLINE_OK {
            assert!(!conn.is_null());
            Ok(Self(conn))
        } else {
            assert!(conn.is_null(), "an error leaves *out null");
            Err(status)
        }
    }

    pub fn start(&mut self, cap: usize) -> Encoded {
        let mut out = vec![0u8; cap];
        let mut written = usize::MAX;
        let status =
            unsafe { comline_connection_start(self.0, out.as_mut_ptr(), cap, &mut written) };
        finish(status, out, written, 0)
    }

    pub fn feed(&mut self, input: &[u8]) -> Fed {
        let mut consumed = usize::MAX;
        let mut ev = comline_event {
            kind: 0xff,
            ..Default::default()
        };
        let status = unsafe {
            comline_connection_feed(self.0, input.as_ptr(), input.len(), &mut consumed, &mut ev)
        };
        Fed {
            status,
            consumed,
            ev,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn request(
        &mut self,
        protocol: u16,
        call_id: u16,
        name: &[u8],
        expects_reply: bool,
        body: &[u8],
        cap: usize,
    ) -> Encoded {
        let mut out = vec![0u8; cap];
        let mut written = usize::MAX;
        let mut request_id = u64::MAX;
        let status = unsafe {
            comline_connection_encode_request(
                self.0,
                protocol,
                call_id,
                name.as_ptr(),
                name.len(),
                u8::from(expects_reply),
                body.as_ptr(),
                body.len(),
                out.as_mut_ptr(),
                cap,
                &mut written,
                &mut request_id,
            )
        };
        finish(status, out, written, request_id)
    }

    pub fn reply(
        &mut self,
        protocol: u16,
        request_id: u64,
        envelope: u8,
        error_id: u16,
        body: &[u8],
        cap: usize,
    ) -> Encoded {
        let mut out = vec![0u8; cap];
        let mut written = usize::MAX;
        let status = unsafe {
            comline_connection_encode_reply(
                self.0,
                protocol,
                request_id,
                envelope,
                error_id,
                body.as_ptr(),
                body.len(),
                out.as_mut_ptr(),
                cap,
                &mut written,
            )
        };
        finish(status, out, written, 0)
    }

    pub fn abandon(&mut self, request_id: u64) -> comline_status {
        unsafe { comline_connection_abandon(self.0, request_id) }
    }

    pub fn is_waiting(&mut self) -> Result<bool, comline_status> {
        let mut waiting = 0xff;
        match unsafe { comline_connection_is_waiting(self.0, &mut waiting) } {
            COMLINE_OK => Ok(waiting == 1),
            status => Err(status),
        }
    }

    /// Spends request ids with one-way calls until the next one is `id`.
    /// `next` is the id the connection hands out next.
    pub fn spend_ids(&mut self, next: u64, id: u64) {
        for expected in next..id {
            let sent = self.request(0, 0, b"spend", false, b"null", 256);
            assert_eq!(sent.status, COMLINE_OK);
            assert_eq!(sent.request_id, expected);
        }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        unsafe { comline_connection_free(self.0) };
    }
}

fn finish(status: comline_status, mut out: Vec<u8>, written: usize, request_id: u64) -> Encoded {
    if status == COMLINE_OK {
        out.truncate(written);
    }
    Encoded {
        status,
        bytes: out,
        written,
        request_id,
    }
}

/// The bytes a span names in `input`.
pub fn at(input: &[u8], span: comline_span) -> &[u8] {
    let start = span.offset as usize;
    &input[start..start + span.len as usize]
}

/// A method name span, unescaped through `comline_method_name_unescape`
/// when the event says it's escaped.
pub fn method_name(input: &[u8], ev: &comline_event) -> String {
    let spelled = at(input, ev.call_name);
    if ev.flags & COMLINE_EVENT_NAME_ESCAPED == 0 {
        return String::from_utf8(spelled.to_vec()).expect("a plain name is UTF-8");
    }
    unescape(spelled).expect("an escaped name unescapes")
}

/// `comline_method_name_unescape`, sized by its own `BUFFER_TOO_SMALL`.
pub fn unescape(spelled: &[u8]) -> Result<String, comline_status> {
    let mut written = 0;
    let status = unsafe {
        comline_method_name_unescape(
            spelled.as_ptr(),
            spelled.len(),
            ptr::null_mut(),
            0,
            &mut written,
        )
    };
    let needed = match status {
        COMLINE_BUFFER_TOO_SMALL => written,
        COMLINE_OK => 0,
        other => return Err(other),
    };
    let mut out = vec![0u8; needed];
    let status = unsafe {
        comline_method_name_unescape(
            spelled.as_ptr(),
            spelled.len(),
            out.as_mut_ptr(),
            out.len(),
            &mut written,
        )
    };
    assert_eq!(status, COMLINE_OK);
    assert_eq!(written, needed);
    Ok(String::from_utf8(out).expect("unescaped names are UTF-8"))
}

pub fn name_hash(name: &str) -> u64 {
    let mut hash = 0;
    let status = unsafe { comline_name_hash(name.as_ptr(), name.len(), &mut hash) };
    assert_eq!(status, COMLINE_OK);
    hash
}

pub fn supports(name: &str) -> bool {
    unsafe { comline_supports(name.as_ptr(), name.len()) == 1 }
}

pub fn handshake_encode(
    ir_hash: u64,
    wire_format: &str,
    framing: &str,
    capabilities: u32,
    cap: usize,
) -> Encoded {
    let mut out = vec![0u8; cap];
    let mut written = usize::MAX;
    let status = unsafe {
        comline_handshake_encode(
            ir_hash,
            wire_format.as_ptr(),
            wire_format.len(),
            framing.as_ptr(),
            framing.len(),
            capabilities,
            out.as_mut_ptr(),
            cap,
            &mut written,
        )
    };
    finish(status, out, written, 0)
}

pub fn handshake_check(ours: &[u8], peer: &[u8]) -> (comline_status, Option<comline_handshake>) {
    let unset = comline_handshake {
        _reserved: 0xdead,
        ..Default::default()
    };
    let mut out = unset;
    let status = unsafe {
        comline_handshake_check(
            ours.as_ptr(),
            ours.len(),
            peer.as_ptr(),
            peer.len(),
            &mut out,
        )
    };
    (status, (out != unset).then_some(out))
}

/// A frame behind its stream length prefix.
pub fn prefixed(frame: &[u8]) -> Vec<u8> {
    let mut out = (frame.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(frame);
    out
}
