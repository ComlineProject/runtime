// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `vectors_are_current`: `vectors/` holds what `comline-runtime` produces.
//! `rust_support_is_proven`: every claim in `support/rust.json` holds against
//! the vectors, and `reports/rust.json` records which cases prove it.
//!
//! `CONFORMANCE_BLESS=1 cargo test -p comline-runtime-conformance` rewrites
//! the vectors and the report.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use comline_runtime_conformance::{
    reference_cases, render, run, Features, Outcome, ProvenClaim, Report, State, Support, Vectors,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
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
    read(&root().join("features.json"))
}

#[test]
fn vectors_are_current() {
    let features = features();
    let reference = reference_cases();
    let mut problems = Vec::new();

    for feature in &features.features {
        if !reference.iter().any(|v| v.feature == feature.id) {
            problems.push(format!("feature `{}` has no vectors", feature.id));
        }
    }
    for vectors in &reference {
        if !features.features.iter().any(|f| f.id == vectors.feature) {
            problems.push(format!("vectors for unknown feature `{}`", vectors.feature));
        }
        let path = root()
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
        let path = root().join("vectors").join(format!("{}.json", feature.id));
        let vectors: Vectors = if bless() {
            match reference_cases()
                .into_iter()
                .find(|v| v.feature == feature.id)
            {
                Some(vectors) => vectors,
                None => continue, // `vectors_are_current` reports it
            }
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
