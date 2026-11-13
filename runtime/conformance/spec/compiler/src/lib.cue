package cases

import (
	"list"
	"strings"
)

// The cases are written here, compactly, and `specs/check.sh` exports them as
// `compiler/cases/<group>.json`, in the shape `specs/compiler.cue` gives
// (`#CompilerCases`). That is the file runners read; this is its source.
//
// A source case differs from its JSON in being easier to write:
//
//   - `packages` is a struct by package name. A package's `config.idp` is
//     `congregation <name>` and `specification_version = 1`, then each line of
//     `config`, then the `dependencies` it declares by path (`../<name>`),
//     each with the unverified protocols it accepts. A schema is written
//     under its namespace, `api` for `src/api.ids`. Every file ends in a
//     newline.
//   - `expect.protocols` is a struct by protocol name, and `acknowledged` one
//     by guarantee. `errors` and `warnings` are codes, and become the
//     `diagnostics`.

// The package setting that lets a protocol say `@unverified`.
_allow: "settings = { annotations = { unverified = { allowed = true } } }"

#Case: {
	about!: string
	packages!: [string]: {
		config: [...string] | *[]
		schemas: [string]: string
		dependencies: [string]: {accept?: [string]: [...string]}
	}
	build!: string
	expect!: {
		builds!: bool
		protocols?: [string]: {
			given_up!: [...string]
			acknowledged?: [string]: {scope!: "protocol" | "package", reason?: string}
		}
		errors?: [...string]
		warnings?: [...string]
		error_contains?: [...string]
	}
}

// Each group file declares `groups: <group>: <case name>: #Case`.
groups: [string]: [string]: #Case

// A package's `dependencies` block, which declares each by path and lists
// the unverified protocols it accepts.
_dependencies: {
	deps: [string]: {accept?: [string]: [...string]}
	text: strings.Join([for d, v in deps {
		"dependencies = {\n    \(d) = {\n        path = \"../\(d)\"\n\(_accept[d])    }\n}\n"
	}], "")
	_accept: {
		for d, v in deps {
			(d): strings.Join([if v.accept != _|_ {
				"        accept_unverified = { " + strings.Join([for proto, ids in v.accept {
					"\(proto) = [" + strings.Join([for id in ids {"\"\(id)\""}], ", ") + "]"
				}], " ") + " }\n"
			}], "")
		}
	}
}

// What a case is as JSON.
out: {
	for G, _ in groups {(G): {
		$schema: "../../specs/gen/compiler.schema.json"
		group:   G
		cases: [for n, c in groups[G] {
			id:          "\(G)/\(n)"
			description: c.about
			packages: [for name, p in c.packages {
				let d = _dependencies & {deps: p.dependencies}
				"name": name
				config: strings.Join(list.Concat([["congregation \(name)", "specification_version = 1"], p.config]), "\n") + "\n" + d.text
				schemas: [for path, text in p.schemas {"path": "src/\(path).ids", "text": text + "\n"}]
				dependencies: [for dep, v in p.dependencies {
					name: dep
					if v.accept != _|_ {accept_unverified: v.accept}
				}]
			}]
			build: c.build
			expect: {
				builds: c.expect.builds
				if c.expect.protocols != _|_ {
					protocols: [for name, p in c.expect.protocols {
						"name":   name
						given_up: p.given_up
						if p.acknowledged != _|_ {
							acknowledged: [for g, a in p.acknowledged {
								guarantee: g
								scope:     a.scope
								if a.reason != _|_ {reason: a.reason}
							}]
						}
					}]
				}
				diagnostics: list.Concat([
					if c.expect.errors != _|_ {[for e in c.expect.errors {code: e, severity: "error"}]},
					if c.expect.warnings != _|_ {[for w in c.expect.warnings {code: w, severity: "warning"}]},
				])
				if c.expect.error_contains != _|_ {error_contains: c.expect.error_contains}
			}
		}]
	}}
}
