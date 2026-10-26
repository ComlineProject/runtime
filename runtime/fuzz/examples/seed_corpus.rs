// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Writes every conformance vector, and every mirror's samples, as seed
//! inputs for the fuzz targets: `corpus/<target>/<name>`, under the
//! directory given (the fuzz crate's own by default). The vectors come from
//! `reference_cases()`, so the seeds match the reference as built.
//!
//! Each target's input starts with what it reads first (a format byte, a
//! type in `comline_runtime_fuzz`'s grammar, a mirror byte), then the
//! vector's bytes.
//!
//! The C ABI's targets (`ffi_feed`, `ffi_vs_core`) read a config and
//! operations (`src/ffi.rs`). Their seeds are the `framing-datagram2` and
//! `framing-jsonrpc` vectors' frames, fed to a connection of that framing
//! (stream and message mode, after a call so a reply can answer it); the
//! `handshake` vectors' frames, fed to a connection whose own frame they
//! match; and a whole exchange (`drive::exchange`) under every framing,
//! mode, handshake and ordering, so the fuzzer starts past the handshake.

#[path = "../../conformance/tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use comline_runtime::contract::FORMAT_PACKED;
use comline_runtime_conformance::{reference_cases, Op, Type, Value};
use comline_runtime_fuzz::ffi::drive::{self, Cap, Config, Op as FfiOp, Ours};
use comline_runtime_fuzz::ffi::{read as read_ffi, write as write_ffi};
use comline_runtime_fuzz::{type_of, write_format, write_type};

/// Both C ABI targets read the same input.
const FFI_TARGETS: [&str; 2] = ["ffi_feed", "ffi_vs_core"];

fn main() {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus"));
    let mut seeds: BTreeMap<&str, Vec<(String, Vec<u8>)>> = BTreeMap::new();
    let mut add = |target: &'static str, name: String, input: Vec<u8>| {
        seeds.entry(target).or_default().push((name, input));
    };
    let mirrors = common::mirrors();

    let mut add_ffi = |name: String, config: &Config, ops: &[FfiOp]| {
        let input = write_ffi(config, ops);
        // The grammar reads back what was written: a seed is the sequence
        // it was made from.
        assert_eq!(read_ffi(&input), (config.clone(), ops.to_vec()), "{name}");
        for target in FFI_TARGETS {
            add(target, name.clone(), input.clone());
        }
    };
    let plain = |framing: &str, stream: u8| Config {
        handshake: comline_runtime_ffi::COMLINE_HANDSHAKE_NONE,
        unordered: 1,
        max_waiting: 16,
        ..drive::exchange_config(framing, stream)
    };
    for framing in ["datagram@2", "jsonrpc@1"] {
        for stream in [0, 1] {
            for handshake in [0, 1] {
                for unordered in [0, 1] {
                    let config = Config {
                        handshake,
                        unordered,
                        ..drive::exchange_config(framing, stream)
                    };
                    let name = format!(
                        "exchange_{}_stream{stream}_handshake{handshake}_unordered{unordered}",
                        framing.replace('@', "-")
                    );
                    add_ffi(name, &config, &drive::exchange());
                }
            }
        }
    }
    for vectors in reference_cases() {
        for case in &vectors.cases {
            let name = case.id.replace('/', "_");
            let frame = case.bytes.0.clone();
            match vectors.feature.as_str() {
                feature @ ("framing-datagram2" | "framing-jsonrpc") => {
                    let framing = if feature == "framing-jsonrpc" {
                        "jsonrpc@1"
                    } else {
                        "datagram@2"
                    };
                    // A call first, so a reply has something to answer.
                    let ops = [
                        FfiOp::Call {
                            protocol: 0,
                            call_id: 3,
                            name: b"greet".to_vec(),
                            expects_reply: 1,
                            body: b"[1]".to_vec(),
                            cap: Cap::Plenty,
                        },
                        FfiOp::PeerRaw {
                            framed: true,
                            bytes: frame,
                        },
                        FfiOp::Deliver(0),
                        FfiOp::Abandon(Ours::Waiting(0)),
                    ];
                    for stream in [0, 1] {
                        add_ffi(
                            format!("{name}_stream{stream}"),
                            &plain(framing, stream),
                            &ops,
                        );
                    }
                }
                "handshake" => {
                    // A frame names its own config; a refused one has none,
                    // and goes to a connection's first frame as it is.
                    let input = &case.input;
                    let mut config = Config {
                        handshake: comline_runtime_ffi::COMLINE_HANDSHAKE_COMLINE,
                        ..drive::exchange_config("datagram@2", 1)
                    };
                    if let (Some(framing), Some(wire_format), Some(ir_hash), Some(capabilities)) = (
                        input["framing"].as_str(),
                        input["wire_format"].as_str(),
                        input["ir_hash"].as_str().and_then(|n| n.parse().ok()),
                        input["capabilities"].as_u64(),
                    ) {
                        config.framing = framing.into();
                        config.wire_format = wire_format.into();
                        config.ir_hash = ir_hash;
                        config.capabilities = capabilities as u32;
                    }
                    let ops = [
                        FfiOp::Start(Cap::Plenty),
                        FfiOp::PeerRaw {
                            framed: true,
                            bytes: frame,
                        },
                        FfiOp::Deliver(0),
                        FfiOp::Call {
                            protocol: 0,
                            call_id: 0,
                            name: b"ping".to_vec(),
                            expects_reply: 1,
                            body: Vec::new(),
                            cap: Cap::Exact,
                        },
                    ];
                    add_ffi(name, &config, &ops);
                }
                _ => {}
            }
        }
    }

    for vectors in reference_cases() {
        for case in vectors.cases {
            let name = case.id.replace('/', "_");
            let bytes = case.bytes.0;
            let typed = match case.op {
                Op::Encode => {
                    let value: Value =
                        serde_json::from_value(case.input["value"].clone()).expect("a value");
                    Some(type_of(&value))
                }
                Op::Decode => Some(
                    serde_json::from_value::<Type>(case.input["type"].clone()).expect("a type"),
                ),
                _ => None,
            };
            if matches!(case.op, Op::StreamPrefix | Op::StreamSplit) {
                // `stream_split` reads its limit first.
                let max = case.input["max"].as_u64().expect("a limit");
                let mut input = u32::try_from(max)
                    .unwrap_or(u32::MAX)
                    .to_le_bytes()
                    .to_vec();
                input.extend_from_slice(&bytes);
                add("stream_split", name, input);
                continue;
            }
            let Some(ty) = typed else {
                add("framing_decode", name, bytes);
                continue;
            };
            let format = case.input["format"].as_str().expect("a format").to_owned();

            let mut input = Vec::new();
            write_format(&format, &mut input);
            write_type(&ty, &mut input);
            input.extend_from_slice(&bytes);
            add("decode_no_panic", name.clone(), input);

            if format == FORMAT_PACKED {
                let mut input = Vec::new();
                write_type(&ty, &mut input);
                input.extend_from_slice(&bytes);
                add("packed_canonical", name.clone(), input);
            }

            for (i, m) in mirrors.iter().enumerate() {
                let mut input = Vec::new();
                write_format(&format, &mut input);
                input.push(i as u8);
                input.extend_from_slice(&bytes);
                add("seed_vs_derive", format!("{name}_{}", m.name), input);
            }
        }
    }

    for (i, m) in mirrors.iter().enumerate() {
        for format in common::FORMATS {
            for (n, bytes) in m.valid(format).into_iter().enumerate() {
                let name = format!("mirror_{}_{}_{n}", m.name, format.replace('@', "-"));
                let mut input = Vec::new();
                write_format(format, &mut input);
                input.push(i as u8);
                input.extend_from_slice(&bytes);
                add("seed_vs_derive", name.clone(), input);

                let mut input = Vec::new();
                write_format(format, &mut input);
                write_type(&m.ty, &mut input);
                input.extend_from_slice(&bytes);
                add("decode_no_panic", name.clone(), input);

                if format == FORMAT_PACKED {
                    let mut input = Vec::new();
                    write_type(&m.ty, &mut input);
                    input.extend_from_slice(&bytes);
                    add("packed_canonical", name, input);
                }
            }
        }
    }

    for (target, inputs) in seeds {
        let dir = root.join(target);
        std::fs::create_dir_all(&dir).expect("a corpus directory");
        for (name, input) in &inputs {
            std::fs::write(dir.join(name), input).expect("a seed written");
        }
        println!("{}: {} seeds", dir.display(), inputs.len());
    }
}
