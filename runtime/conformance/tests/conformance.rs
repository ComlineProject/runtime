// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `vectors_are_current`: `vectors/` holds what `comline-runtime` produces,
//! apart from the `sizes-*` features, whose values and bytes the specification
//! writes and this runtime proves (`sizes_are_proven`, `rust_support_is_proven`).
//! `rust_support_is_proven`: every claim in `support/rust.json` holds against
//! the vectors, and `reports/rust.json` records which cases prove it.
//!
//! `CONFORMANCE_BLESS=1 cargo test -p comline-runtime-conformance` rewrites
//! the vectors and the report.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use comline_runtime::contract::{
    FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED, FRAMING_DATAGRAM, FRAMING_DATAGRAM2,
    FRAMING_DATAGRAM3, FRAMING_JSONRPC, HANDSHAKE_COMLINE, HANDSHAKE_COMLINE2,
};
use comline_runtime_conformance::{
    fact_shown, reference_cases, render, run, Case, Fact, Facts, Features, Op, Outcome, Output,
    Payload, Profile, ProvenClaim,
    Report, State, Support, Vectors,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// The vendored specification (`vendor-spec.sh`).
fn spec() -> PathBuf {
    root().join("spec")
}

fn bless() -> bool {
    std::env::var_os("CONFORMANCE_BLESS").is_some()
}

fn read<T: for<'de> serde::Deserialize<'de>>(path: &Path) -> T {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Compares `wanted` with the file, or writes it when blessing.
fn check_file(path: &Path, wanted: &str, problems: &mut Vec<String>) {
    let current = fs::read_to_string(path).unwrap_or_default();
    if current == wanted {
        return;
    }
    if bless() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, wanted).unwrap();
    } else {
        problems.push(format!("{} is out of date", path.display()));
    }
}

fn features() -> Features {
    read(&spec().join("features.json"))
}

/// Whether the specification writes `feature`'s vectors and this runtime only
/// proves them: every case is a `size` case, whose witnesses (values and the
/// bytes they encode to) come from a model of the encoders, not from here.
/// `run` is what holds them to the real encoders.
fn proven_not_produced(feature: &str) -> bool {
    let path = spec().join("vectors").join(format!("{feature}.json"));
    path.exists() && {
        let vectors: Vectors = read(&path);
        !vectors.cases.is_empty() && vectors.cases.iter().all(|c| c.op() == Op::Size)
    }
}

#[test]
fn vectors_are_current() {
    let features = features();
    let reference = reference_cases();
    let mut problems = Vec::new();

    for feature in &features.features {
        let produced = reference.iter().any(|v| v.feature == feature.id);
        if !produced && !proven_not_produced(&feature.id) {
            problems.push(format!("feature `{}` has no vectors", feature.id));
        }
    }
    for vectors in &reference {
        if !features.features.iter().any(|f| f.id == vectors.feature) {
            problems.push(format!("vectors for unknown feature `{}`", vectors.feature));
        }
        let path = spec()
            .join("vectors")
            .join(format!("{}.json", vectors.feature));
        check_file(&path, &render(vectors), &mut problems);
    }

    assert!(
        problems.is_empty(),
        "{}\nIf the wire change is intended: CONFORMANCE_BLESS=1 cargo test -p comline-runtime-conformance",
        problems.join("\n")
    );
}

#[test]
fn rust_support_is_proven() {
    let features = features();
    let support: Support = read(&root().join("support/rust.json"));
    let mut problems = Vec::new();

    for id in support.support.keys() {
        if !features.features.iter().any(|f| &f.id == id) {
            problems.push(format!("support claims unknown feature `{id}`"));
        }
    }

    let mut proven = BTreeMap::new();
    for feature in &features.features {
        let Some(claim) = support.support.get(&feature.id) else {
            problems.push(format!(
                "`{}` has no claim in support/rust.json",
                feature.id
            ));
            continue;
        };
        // Blessing rewrites the vectors alongside, so prove against what
        // they're being rewritten to.
        let path = spec().join("vectors").join(format!("{}.json", feature.id));
        let blessed = if bless() {
            reference_cases()
                .into_iter()
                .find(|v| v.feature == feature.id)
        } else {
            None
        };
        let vectors: Vectors = if let Some(vectors) = blessed {
            vectors
        } else if bless() && !proven_not_produced(&feature.id) {
            continue; // `vectors_are_current` reports it
        } else if path.exists() {
            read(&path)
        } else {
            problems.push(format!(
                "`{}` has no vectors ({})",
                feature.id,
                path.display()
            ));
            continue;
        };
        if vectors.cases.is_empty() {
            problems.push(format!("`{}` has no cases", feature.id));
        }

        let mut proven_by = Vec::new();
        for case in &vectors.cases {
            match (claim.state, run(case)) {
                (State::Supported, Outcome::Pass) => proven_by.push(case.id.clone()),
                (State::Supported, Outcome::Fail(why)) => {
                    problems.push(format!("{}: {why}", case.id))
                }
                (State::Supported, Outcome::NotImplemented(what)) => problems.push(format!(
                    "{}: `{}` is claimed supported, but {what} isn't implemented",
                    case.id, feature.id
                )),
                (State::Partial, outcome) => {
                    let known = claim.failing.contains(&case.id);
                    match outcome {
                        Outcome::Pass if known => problems.push(format!(
                            "{}: listed as failing in `{}`'s partial claim, but passes - drop it from `failing`",
                            case.id, feature.id
                        )),
                        Outcome::Pass => proven_by.push(case.id.clone()),
                        _ if known => {}
                        Outcome::Fail(why) | Outcome::NotImplemented(why) => {
                            problems.push(format!("{}: {why}", case.id))
                        }
                    }
                }
                (State::Unsupported, Outcome::NotImplemented(_)) => {}
                (State::Unsupported, _) => problems.push(format!(
                    "{}: `{}` is claimed unsupported, but the runtime implements it - claim it",
                    case.id, feature.id
                )),
            }
        }
        for id in &claim.failing {
            if claim.state != State::Partial {
                problems.push(format!(
                    "`{}` lists failing cases but isn't partial",
                    feature.id
                ));
                break;
            }
            if !vectors.cases.iter().any(|c| &c.id == id) {
                problems.push(format!(
                    "`{}` lists `{id}` as failing, but has no such case",
                    feature.id
                ));
            }
        }
        proven.insert(
            feature.id.clone(),
            ProvenClaim {
                claim: claim.clone(),
                proven_by,
            },
        );
    }

    let report = Report {
        schema: Some("../spec/specs/gen/report.schema.json".into()),
        runtime: support.runtime.clone(),
        support: proven,
    };
    if problems.is_empty() {
        check_file(
            &root().join("reports/rust.json"),
            &render(&report),
            &mut problems,
        );
    }
    assert!(
        problems.is_empty(),
        "{}\nIf only the report is stale: CONFORMANCE_BLESS=1 cargo test -p comline-runtime-conformance",
        problems.join("\n")
    );
}

/// Every official encoding has a profile (reading one holds it to its kind's
/// facts and their values), and every fact is proven: its cases exist, pass
/// against `comline-runtime`, and show the stated value.
#[test]
fn profiles_are_proven() {
    let vectors: Vec<Vectors> = reference_cases();
    let case =
        |id: &str| -> Option<&Case> { vectors.iter().flat_map(|v| &v.cases).find(|c| c.id == id) };
    let mut problems = Vec::new();

    let names = [
        FORMAT_JSON,
        FORMAT_MSGPACK,
        FORMAT_PACKED,
        FRAMING_DATAGRAM,
        FRAMING_DATAGRAM2,
        FRAMING_DATAGRAM3,
        FRAMING_JSONRPC,
        HANDSHAKE_COMLINE,
        HANDSHAKE_COMLINE2,
    ];
    for name in names {
        let path = spec().join("profiles").join(format!("{name}.json"));
        if !path.exists() {
            problems.push(format!("`{name}` has no profile"));
            continue;
        }
        let profile: Profile = read(&path);
        if profile.name != name {
            problems.push(format!("{} names `{}`", path.display(), profile.name));
        }
        for (fact, proven_by) in profile.facts.stated() {
            let label = fact.name();
            let mut proving = Vec::new();
            for id in proven_by {
                match case(id) {
                    None => problems.push(format!("`{name}`: `{label}` names unknown case `{id}`")),
                    Some(c) => match run(c) {
                        Outcome::Pass => proving.push(c),
                        other => {
                            problems.push(format!("`{name}`: `{label}`'s case `{id}`: {other:?}"))
                        }
                    },
                }
            }
            if proving.len() == proven_by.len() {
                if let Err(e) = fact_shown(fact, &proving) {
                    problems.push(format!("`{name}`: `{label}`: {e}"));
                }
            }
        }
    }

    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Every size rule a format's profile states is proven by cases that exist and
/// pass against `comline-runtime`: the witnesses' bytes are what the real
/// encoder writes (and `encoded_len` agrees), which is what the specification
/// leaves to this run.
#[test]
fn size_rules_are_proven() {
    use comline_runtime_conformance::runner::{load_corpus, run_all, InProcess};

    fn proven_by(value: &serde_json::Value, ids: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(members) => {
                for (key, member) in members {
                    match (key.as_str(), member) {
                        ("proven_by", serde_json::Value::Array(cases)) => {
                            ids.extend(cases.iter().filter_map(|c| c.as_str().map(String::from)))
                        }
                        _ => proven_by(member, ids),
                    }
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|i| proven_by(i, ids)),
            _ => {}
        }
    }

    let corpus = load_corpus(&spec()).unwrap();
    let mut problems = Vec::new();
    for name in [FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED] {
        let profile: serde_json::Value =
            read(&spec().join("profiles").join(format!("{name}.json")));
        let Some(sizes) = profile.get("sizes") else {
            problems.push(format!("`{name}` states no sizes"));
            continue;
        };
        let feature = sizes["feature"].as_str().unwrap_or_default();
        let cases = corpus.feature(feature);
        if cases.is_empty() {
            problems.push(format!("`{name}`: the feature `{feature}` has no cases"));
            continue;
        }
        let outcomes = run_all(&cases, &mut InProcess).unwrap();
        let mut ids = Vec::new();
        proven_by(sizes, &mut ids);
        for id in ids {
            match outcomes.get(&id) {
                Some(Outcome::Pass) => {}
                Some(other) => problems.push(format!("`{name}`: `{id}`: {other:?}")),
                None => problems.push(format!("`{name}` names `{id}`, which isn't in `{feature}`")),
            }
        }
        for (id, outcome) in &outcomes {
            if *outcome != Outcome::Pass {
                problems.push(format!("{id}: {outcome:?}"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Reading a vectors file and writing it back gives the file: the typed
/// inputs hold everything the files do, and what's sent to implementations
/// is what the corpus says.
#[test]
fn vectors_write_back_as_read() {
    let corpus = comline_runtime_conformance::runner::load_corpus(&spec()).unwrap();
    for vectors in &corpus.vectors {
        let path = spec()
            .join("vectors")
            .join(format!("{}.json", vectors.feature));
        assert_eq!(
            render(vectors),
            fs::read_to_string(&path).unwrap(),
            "{}",
            path.display()
        );
    }
}

/// The library runner, with `comline-runtime` in process, passes every case
/// of the corpus on disk and proves every official profile.
#[test]
fn the_runner_passes_the_reference() {
    use comline_runtime_conformance::runner::{load_corpus, run_all, InProcess};

    let corpus = load_corpus(&spec()).unwrap();
    let cases: Vec<&Case> = corpus.cases().collect();
    assert!(!cases.is_empty());
    let outcomes = run_all(&cases, &mut InProcess).unwrap();
    let failed: Vec<_> = outcomes
        .iter()
        .filter(|(_, o)| **o != Outcome::Pass)
        .collect();
    assert!(failed.is_empty(), "{failed:#?}");
    assert_eq!(corpus.profiles.len(), 9);
    for profile in corpus.profiles.values() {
        assert!(!corpus.cases_for(profile).is_empty(), "{}", profile.name);
        for proof in corpus.prove(profile, &outcomes) {
            assert_eq!(proof.result, Ok(()), "{}: {}", profile.name, proof.fact);
        }
    }
}

/// The one rule on a framing and a format, over the official encodings'
/// facts: JSON-RPC takes `json@1` only, and a datagram anything.
#[test]
fn official_pairs_follow_their_facts() {
    let fact = |name: &str| -> Facts { read::<Profile>(&spec().join("profiles").join(format!("{name}.json"))).facts };
    let payload = |name: &str| match fact(name) {
        Facts::Framing(f) => f.payload.value,
        _ => panic!("`{name}` isn't a framing"),
    };
    let output = |name: &str| match fact(name) {
        Facts::Format(f) => f.output.value,
        _ => panic!("`{name}` isn't a format"),
    };
    for framing in [FRAMING_DATAGRAM, FRAMING_DATAGRAM2, FRAMING_DATAGRAM3] {
        for format in [FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED] {
            assert!(payload(framing).carries(output(format)), "{format} on {framing}");
        }
    }
    assert!(payload(FRAMING_JSONRPC).carries(output(FORMAT_JSON)));
    assert!(!payload(FRAMING_JSONRPC).carries(output(FORMAT_MSGPACK)));
    assert!(!payload(FRAMING_JSONRPC).carries(output(FORMAT_PACKED)));
}

/// The rule goes by the fact, so a custom format that writes JSON fits.
#[test]
fn the_rule_names_no_encoding() {
    assert!(Payload::Json.carries(Output::Json));
    assert!(!Payload::Json.carries(Output::Bytes));
    assert!(Payload::Opaque.carries(Output::Json));
    assert!(Payload::Opaque.carries(Output::Bytes));
}

/// Params as JSON-RPC 2.0 has them: an array or an object when given, or
/// `null`, which is what `jsonrpc@1` writes for none.
#[test]
fn json_payload_takes_structured_params() {
    for fits in [&b""[..], b"null", b"[]", b"{}", br#"[7,"x"]"#] {
        assert!(Payload::Json.carries_params(fits), "{}", String::from_utf8_lossy(fits));
    }
    for not in [&b"1"[..], br#""x""#, b"true", b"[", b"[],\"x\":[]", b"\x92\x07"] {
        assert!(!Payload::Json.carries_params(not), "{}", String::from_utf8_lossy(not));
        assert!(Payload::Opaque.carries_params(not));
    }
}

/// `payload` and `output` need cases that show them, not just cases that pass.
#[test]
fn payload_and_output_facts_need_their_cases() {
    let vectors: Vec<Vectors> = reference_cases();
    let case = |id: &str| -> &Case {
        vectors.iter().flat_map(|v| &v.cases).find(|c| c.id == id).unwrap_or_else(|| panic!("{id}"))
    };
    let exact = case("framing-jsonrpc/request");
    let msgpack = case("framing-jsonrpc/reject-request-params-msgpack");
    let scalar = case("framing-jsonrpc/reject-request-scalar-params");
    let unrelated = case("framing-jsonrpc/reject-request-not-json");
    let json = Fact::Payload(Payload::Json);
    assert_eq!(fact_shown(json, &[exact, msgpack]), Ok(()));
    assert!(fact_shown(json, &[exact]).is_err());
    assert!(fact_shown(json, &[msgpack]).is_err());
    assert!(fact_shown(json, &[exact, scalar]).is_err(), "its params are JSON");
    assert!(fact_shown(json, &[exact, unrelated]).is_err(), "not that frame");
    // A datagram has no refusal of that kind to name: its short frame isn't
    // a frame with another payload.
    let datagram = case("framing-datagram/request");
    let short = case("framing-datagram/reject-short-request");
    assert!(fact_shown(json, &[datagram, short]).is_err());

    let opaque = Fact::Payload(Payload::Opaque);
    assert_eq!(fact_shown(opaque, &[case("framing-datagram/request-params-msgpack")]), Ok(()));
    assert!(fact_shown(opaque, &[datagram]).is_err(), "its params are JSON");
    assert!(fact_shown(opaque, &[exact]).is_err());

    let (writes_json, writes_bytes) = (Fact::Output(Output::Json), Fact::Output(Output::Bytes));
    let (text, binary) = (case("format-json/struct"), case("format-msgpack/struct"));
    assert_eq!(fact_shown(writes_json, &[text]), Ok(()));
    assert!(fact_shown(writes_json, &[text, binary]).is_err());
    assert_eq!(fact_shown(writes_bytes, &[binary]), Ok(()));
    assert!(fact_shown(writes_bytes, &[text]).is_err());
    assert!(fact_shown(writes_json, &[exact]).is_err(), "not an encode");
}

/// A handshake's facts are booleans; `false` (or leaving one out) claims
/// nothing, and `true` needs the cases that show the refusal.
#[test]
fn handshake_facts_need_their_refusals() {
    let profile: Profile = serde_json::from_str(
        r#"{"name": "com.acme.hello@1", "kind": "handshake",
            "facts": {"schema_agreement": {"value": false}}}"#,
    )
    .unwrap();
    let stated = profile.facts.stated();
    assert_eq!(stated.len(), 1);
    assert_eq!(stated[0].0, Fact::SchemaAgreement(false));
    assert!(stated[0].1.is_empty());
    assert_eq!(fact_shown(Fact::SchemaAgreement(false), &[]), Ok(()));
    let written = serde_json::to_value(&profile).unwrap();
    assert_eq!(
        written["facts"]["schema_agreement"]["value"],
        serde_json::json!(false)
    );

    let vectors = reference_cases();
    let case = |id: &str| -> &Case {
        vectors
            .iter()
            .flat_map(|v| &v.cases)
            .find(|c| c.id == id)
            .unwrap()
    };
    let agree = case("handshake/check-agree");
    let ir_hash = case("handshake/check-reject-ir-hash");
    let format = case("handshake/check-reject-format");
    let framing = case("handshake/check-reject-framing");
    assert_eq!(
        fact_shown(Fact::SchemaAgreement(true), &[agree, ir_hash]),
        Ok(())
    );
    assert!(fact_shown(Fact::SchemaAgreement(true), &[agree]).is_err());
    assert!(fact_shown(Fact::SchemaAgreement(true), &[agree, format]).is_err());
    assert_eq!(
        fact_shown(Fact::EncodingAgreement(true), &[agree, format, framing]),
        Ok(())
    );
    assert!(fact_shown(Fact::EncodingAgreement(true), &[agree, format]).is_err());

    // comline@2: an accepted negotiation, and a refusal for each thing the
    // fact covers.
    let accept = case("handshake-comline2/offer-accept");
    let line = case("handshake-comline2/offer-line-mismatch");
    let package = case("handshake-comline2/offer-package-mismatch");
    let encoding = case("handshake-comline2/offer-encoding-mismatch");
    let exchange = case("handshake-comline2/offer-exchange-mismatch");
    assert_eq!(fact_shown(Fact::SchemaAgreement(true), &[accept, line, package]), Ok(()));
    assert!(fact_shown(Fact::SchemaAgreement(true), &[accept, line]).is_err());
    assert!(fact_shown(Fact::SchemaAgreement(true), &[line, package]).is_err());
    assert_eq!(fact_shown(Fact::EncodingAgreement(true), &[accept, encoding, exchange]), Ok(()));
    assert!(fact_shown(Fact::EncodingAgreement(true), &[accept, encoding]).is_err());
}
