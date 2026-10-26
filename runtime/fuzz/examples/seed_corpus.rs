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

#[path = "../../conformance/tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use comline_runtime::contract::FORMAT_PACKED;
use comline_runtime_conformance::{reference_cases, Op, Type, Value};
use comline_runtime_fuzz::{type_of, write_format, write_type};

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
