package conformance

import "github.com/comlineproject/specification/catalog"

// `compiler/cases/<group>.json`: compiler conformance cases. Each is a schema
// graph as data, with the outcome any compiler must reproduce for it: the
// way runtimes reproduce the vectors' bytes. `compiler/readme.md` says what
// a case holds and how a compiler runs one.
#CompilerCases: close({
	$schema?: string
	// The file's name: every case's id is `<group>/<name>`.
	group!: =~"^[a-z0-9-]+$"
	cases!: [...#CompilerCase]
})

#CompilerCase: close({
	id!: #CaseId
	// One line.
	description!: =~"^[^\n]+$"
	// The case's packages: one to build, and the ones it depends on, directly
	// or not. Each is a directory named after it, side by side.
	packages!: [#CompilerPackage, ...#CompilerPackage]
	// The package to build: one of `packages`.
	build!: #CompilerPackageName
	expect!: #CompilerExpect
})

// A package's name is its congregation's, its directory's and the name
// its dependants use it by in `use`.
#CompilerPackageName: =~"^[a-z][a-z0-9_]*$"

#CompilerPackage: close({
	name!: #CompilerPackageName
	// `config.idp`, whole.
	config!: string
	// The schema files, `src/<namespace>.ids`.
	schemas!: [...close({
		path!: =~"^src/([a-z][a-z0-9_]*/)*[a-z][a-z0-9_]*\\.ids$"
		text!: string
	})]
	// The packages this one depends on by path (`../<name>`), as `config`
	// declares them. Restated here so a runner can lay the packages out, and
	// a check can hold them to `config`, without reading `config.idp`.
	dependencies!: [...close({
		name!: #CompilerPackageName
		// What `config` accepts of the dependency's unverified protocols: a
		// protocol's name and the guarantees it gives up, which have to be
		// exactly its set.
		accept_unverified?: [string]: [...#CompilerGuarantee]
	})]
})

// A guarantee's id in the catalog.
#CompilerGuarantee: or([for id, _ in catalog.guarantees {id}])

// What building the package gives. A build that succeeds says what each of
// its protocols gives up, and its warnings; one that fails, its errors, as
// codes where they have one and else as text.
#CompilerExpect: #CompilerBuilds | #CompilerFails

#CompilerBuilds: close({
	builds!: true
	// Every protocol of the package built, and no others.
	protocols!: [...#CompilerProtocol]
	// The warnings, and no others.
	diagnostics!: [...#CompilerWarning]
})

#CompilerFails: close({
	builds!: false
	// The errors, and no others, of those that have a code.
	diagnostics!: [...#CompilerError]
	// For an error without a code: text its message contains. Provisional,
	// until such errors get codes.
	error_contains?: [string, ...string]
})

#CompilerProtocol: close({
	name!: =~"^[A-Z][A-Za-z0-9]*$"
	// The guarantees it gives up, as a set: what is frozen with it.
	given_up!: [...#CompilerGuarantee]
	// Who acknowledged some of what it gives up, and the reason shown for it,
	// where the case asserts it: guarantees it doesn't list aren't asserted.
	acknowledged?: [...close({
		guarantee!: #CompilerGuarantee
		// Where the acknowledgement was written: on the protocol, or in
		// `config.idp` for the package.
		scope!: catalog.#Scope & ("package" | "protocol")
		reason?: string
	})]
})

// A diagnostic is matched on its code and severity, not its message. The
// vocabulary is closed; a code not here has no case yet.
#CompilerDiagnostic: #CompilerError | #CompilerWarning

#CompilerError: or([
	close({code!: "unverified::missing-marker", severity!: "error"}),
	close({code!: "unverified::missing-reason", severity!: "error"}),
	close({code!: "unverified::unknown-guarantee", severity!: "error"}),
	close({code!: "unverified::duplicate-guarantee", severity!: "error"}),
	close({code!: "unverified::missing-guarantee", severity!: "error"}),
	close({code!: "unverified::wrong-scope", severity!: "error"}),
	close({code!: "bounds::unbounded", severity!: "error"}),
	close({code!: "bounds::unbounded-depth", severity!: "error"}),
	close({code!: "settings::forbidden", severity!: "error"}),
])

#CompilerWarning: or([
	close({code!: "unverified::not-given-up", severity!: "warning"}),
	close({code!: "unverified::nothing-given-up", severity!: "warning"}),
	close({code!: "unverified::schema-agreement", severity!: "warning"}),
	close({code!: "unverified::encoding-agreement", severity!: "warning"}),
	close({code!: "unverified::encoding-conformance", severity!: "warning"}),
	close({code!: "unverified::bounded-size", severity!: "warning"}),
	close({code!: "unverified::version-honesty", severity!: "warning"}),
])
