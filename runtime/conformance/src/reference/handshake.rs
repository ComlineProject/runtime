// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use comline_runtime::contract::{
    HandshakeFrame, FORMAT_JSON, FORMAT_MSGPACK, FRAMING_DATAGRAM, FRAMING_DATAGRAM2,
};

use crate::engine::decode;
use crate::{Case, Compare, Expect, HandshakeIn, Hex, Input};

pub(super) fn handshake(
    id: &str,
    ir_hash: u64,
    wire_format: &str,
    framing: &str,
    capabilities: u32,
) -> (String, Input) {
    let input = HandshakeIn {
        ir_hash,
        wire_format: wire_format.into(),
        framing: framing.into(),
        capabilities,
    };
    (id.into(), Input::Handshake(Some(input)))
}

/// One end's `comline@1` declaration against the frame its peer sent:
/// agreeing (capability bits differ, which is allowed), then a peer that
/// differs in the schema fingerprint, the format, or the framing alone. The
/// cases `comline@1`'s profile proves its facts by.
pub(super) fn handshake_check_cases() -> Vec<Case> {
    let ours = HandshakeIn {
        ir_hash: 1,
        wire_format: FORMAT_JSON.into(),
        framing: FRAMING_DATAGRAM.into(),
        capabilities: 0,
    };
    let peer = |ir_hash: u64, format: &str, framing: &str, capabilities: u32| {
        let mut bytes = Vec::new();
        HandshakeFrame::new(ir_hash, format, framing, capabilities).encode(&mut bytes);
        bytes
    };
    let check = |id: &str, bytes: Vec<u8>| {
        let case = Case {
            id: format!("handshake/{id}"),
            input: Input::HandshakeCheck(ours.clone()),
            bytes: Hex(bytes),
            expect: Expect::Exact,
            compare: Compare::Bytes,
        };
        match decode(&case.input, &case.bytes.0) {
            Ok(Some(_)) => case,
            Ok(None) => Case {
                expect: Expect::Reject,
                ..case
            },
            Err(outcome) => panic!("reference case `{id}` doesn't check: {outcome:?}"),
        }
    };
    vec![
        check("check-agree", peer(1, FORMAT_JSON, FRAMING_DATAGRAM, 0b101)),
        check(
            "check-reject-ir-hash",
            peer(2, FORMAT_JSON, FRAMING_DATAGRAM, 0),
        ),
        check(
            "check-reject-format",
            peer(1, FORMAT_MSGPACK, FRAMING_DATAGRAM, 0),
        ),
        check(
            "check-reject-framing",
            peer(1, FORMAT_JSON, FRAMING_DATAGRAM2, 0),
        ),
    ]
}
