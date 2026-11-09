// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The wire names `contract` spells out are the specification's catalog's.
//! They're kept as literals so `comline-runtime` depends on nothing for
//! them; a published wire name never changes, and this holds them to it.

use comline_catalog::{encoding, sets, Kind};
use comline_runtime::contract::{
    FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED, FRAMING_DATAGRAM, FRAMING_DATAGRAM2,
    FRAMING_DATAGRAM3, FRAMING_JSONRPC, HANDSHAKE_COMLINE, HANDSHAKE_COMLINE2, HANDSHAKE_NONE,
};

#[test]
fn every_wire_name_is_the_catalogs() {
    let names = [
        (FORMAT_PACKED, encoding::PACKED_1, Kind::Format),
        (FORMAT_JSON, encoding::JSON_1, Kind::Format),
        (FORMAT_MSGPACK, encoding::MSGPACK_1, Kind::Format),
        (FRAMING_JSONRPC, encoding::JSONRPC_1, Kind::Framing),
        (FRAMING_DATAGRAM, encoding::DATAGRAM_1, Kind::Framing),
        (FRAMING_DATAGRAM2, encoding::DATAGRAM_2, Kind::Framing),
        (FRAMING_DATAGRAM3, encoding::DATAGRAM_3, Kind::Framing),
        (HANDSHAKE_COMLINE, encoding::COMLINE_1, Kind::Handshake),
        (HANDSHAKE_COMLINE2, encoding::COMLINE_2, Kind::Handshake),
        (HANDSHAKE_NONE, encoding::NONE, Kind::Handshake),
    ];
    for (ours, catalog, kind) in names {
        assert_eq!(ours, catalog);
        assert_eq!(comline_catalog::encoding(ours).map(|e| e.kind), Some(kind), "{ours}");
    }
    // And the runtime names every official encoding.
    let mut ours: Vec<&str> = names.iter().map(|(name, ..)| *name).collect();
    let mut official = sets::OFFICIAL.to_vec();
    ours.sort_unstable();
    official.sort_unstable();
    assert_eq!(ours, official);
}
