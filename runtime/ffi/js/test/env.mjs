// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// Where the Node tests find what they run against. Each defaults to its
// place under `CARGO_TARGET_DIR` (or the repository's `target`):
//
// - `COMLINE_WASM`: the wasm32 build, `--profile wasm`;
// - `COMLINE_WASM_TEST_HOOKS`: the same with `--features test-hooks`, built
//   into `<target>/test-hooks` so it doesn't replace the other;
// - `COMLINE_VECTORS`: the vectors `examples/export_vectors.rs` exports.

import { join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const TARGET = process.env.CARGO_TARGET_DIR || join(ROOT, "target");
const BUILD = join("wasm32-unknown-unknown", "wasm", "comline_runtime_ffi.wasm");

export const WASM = process.env.COMLINE_WASM || join(TARGET, BUILD);
export const WASM_TEST_HOOKS =
  process.env.COMLINE_WASM_TEST_HOOKS || join(TARGET, "test-hooks", BUILD);
export const VECTORS = process.env.COMLINE_VECTORS || join(TARGET, "ffi-vectors");
