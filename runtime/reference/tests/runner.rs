// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The conformance runner's process adapter, [`Lines`], over the built
//! `comline-reference`: every case passes, every official profile is proven,
//! and a wrong byte would show as a diff.

use std::io::BufReader;
use std::path::Path;
use std::process::{Command, Stdio};

use comline_runtime_conformance::runner::{judge, load_corpus, questions, run_all, Lines};
use comline_runtime_conformance::{Answer, Case, Outcome};

#[test]
fn the_reference_process_passes_every_case() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_comline-reference"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn comline-reference");
    let mut reference = Lines::new(
        child.stdin.take().unwrap(),
        BufReader::new(child.stdout.take().unwrap()),
    );
    let version = reference.version().unwrap();
    assert_eq!(version["protocol"], 1);

    let corpus =
        load_corpus(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../conformance")).unwrap();
    let cases: Vec<&Case> = corpus.cases().collect();
    let outcomes = run_all(&cases, &mut reference).unwrap();
    let failed: Vec<_> = outcomes
        .iter()
        .filter(|(_, o)| **o != Outcome::Pass)
        .collect();
    assert!(failed.is_empty(), "{failed:#?}");
    for profile in corpus.profiles.values() {
        for proof in corpus.prove(profile, &outcomes) {
            assert_eq!(proof.result, Ok(()), "{}: {}", profile.name, proof.fact);
        }
    }

    // One byte flipped in an encode answer fails the case, with a diff.
    let case = corpus.case("envelope/err").unwrap();
    let asked = questions(case);
    let mut answers: Vec<Answer> =
        comline_runtime_conformance::runner::Implementation::answer(&mut reference, &asked)
            .unwrap();
    if let Answer::Bytes(bytes) = &mut answers[0] {
        bytes[1] ^= 0xff;
    }
    match judge(case, &answers) {
        Outcome::Fail(why) => assert!(why.contains("byte 1 differs"), "{why}"),
        other => panic!("{other:?}"),
    }

    drop(reference);
    assert!(child.wait().unwrap().success());
}
