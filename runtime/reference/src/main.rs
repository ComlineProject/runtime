// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `comline-reference`: answers encode and decode questions, newline-delimited
//! JSON on stdin and stdout, until stdin ends. See README.md.

fn main() -> std::io::Result<()> {
    comline_reference::serve(std::io::stdin().lock(), std::io::stdout().lock())
}
