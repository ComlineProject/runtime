package cases

// Where a `use` path starts. Within a package, its own schemas are reached
// only through a relative path: `package::` (from the package's root),
// `parent::` (one level up) or `self::` (the current namespace). An absolute
// first segment always means `std` or a dependency, so a bare path to one of
// the package's own schemas is an error, with a fix that writes `package::`
// in front of it (`import::bare-own-schema-path`). A path that matches
// nothing is `import::unresolved`, as it was. Rule and rollout:
// https://comlineproject.github.io/docs/proposals/use-paths-and-groups/relative-use/
//
// A package's own schema named like a dependency or `std` is refused outright
// (the shadowing guard), and `std`, `package`, `parent` and `self` can't be a
// dependency's name. Neither error has a code: the cases say what the message
// contains, as the acceptance cases do.
//
// Every error is reported, not only the first, among those of the schemas: a
// schema's bare name that nothing brought into scope is
// `reference::not-in-scope` besides any other error of the schema (see the
// `references` group). A config error, as the reserved dependency names,
// stops the build before the schemas are checked, as in Rust, so those cases
// hold no reference of their own.

_impModels: """
	struct Blob {
	    size: u32
	}

	struct Tag {
	    id: u32
	}
	"""

// A schema that holds no reference, for the cases a config error stops.
_impPlain: """
	struct Upload {
	    size: u32
	}
	"""

// A schema using `Blob`, whichever way `use` brought it in.
_impUsesBlob: """
	struct Upload {
	    blob: Blob
	}
	"""

groups: imports: {
	// A bare path to the package's own schema, in each form of a `use`.
	"bare-own-item": {
		about: "a bare path to an item of the package's own schema is an error"
		packages: app: schemas: {models: _impModels, api: "use models::Blob\n\n" + _impUsesBlob}
		build: "app"
		expect: {builds: false, errors: ["import::bare-own-schema-path"]}
	}
	"bare-own-namespace": {
		about: "a bare path to the package's own schema as a whole is an error, and so is the bare Blob after it, as a whole-namespace use brings in only its name: every error is reported"
		packages: app: schemas: {models: _impModels, api: "use models\n\n" + _impUsesBlob}
		build: "app"
		expect: {builds: false, errors: ["import::bare-own-schema-path", "reference::not-in-scope"]}
	}
	"bare-own-multi": {
		about: "a bare path with {...} to the package's own schema is an error"
		packages: app: schemas: {models: _impModels, api: "use models::{Blob, Tag}\n\n" + _impUsesBlob}
		build: "app"
		expect: {builds: false, errors: ["import::bare-own-schema-path"]}
	}
	"bare-own-glob": {
		about: "a bare path with * to the package's own schema is an error"
		packages: app: schemas: {models: _impModels, api: "use models::*\n\n" + _impUsesBlob}
		build: "app"
		expect: {builds: false, errors: ["import::bare-own-schema-path"]}
	}
	"bare-own-nested": {
		about: "a bare path to a schema in a directory of the package is an error, whatever the importing schema's own place"
		packages: app: schemas: {
			"shapes/models": _impModels
			"shapes/api":    "use shapes::models::Blob\n\n" + _impUsesBlob
		}
		build: "app"
		expect: {builds: false, errors: ["import::bare-own-schema-path"]}
	}

	// The same schemas through the relative forms, which build.
	"package-item": {
		about: "package:: reaches an item of the package's own schema"
		packages: app: schemas: {models: _impModels, api: "use package::models::Blob\n\n" + _impUsesBlob}
		build: "app"
		expect: {builds: true, protocols: {}}
	}
	"package-namespace": {
		about: "package:: reaches the package's own schema as a whole"
		packages: app: schemas: {models: _impModels, api: "use package::models\n\nstruct Upload {\n    blob: models::Blob\n}"}
		build: "app"
		expect: {builds: true, protocols: {}}
	}
	"package-multi": {
		about: "package:: with {...} reaches several items of the package's own schema"
		packages: app: schemas: {models: _impModels, api: "use package::models::{Blob, Tag}\n\nstruct Upload {\n    blob: Blob\n    tag: Tag\n}"}
		build: "app"
		expect: {builds: true, protocols: {}}
	}
	"package-glob": {
		about: "package:: with * reaches every item of the package's own schema"
		packages: app: schemas: {models: _impModels, api: "use package::models::*\n\n" + _impUsesBlob}
		build: "app"
		expect: {builds: true, protocols: {}}
	}
	"package-nested": {
		about: "package:: reaches a schema in a directory of the package from one beside it"
		packages: app: schemas: {
			"shapes/models": _impModels
			"shapes/api":    "use package::shapes::models::Blob\n\n" + _impUsesBlob
		}
		build: "app"
		expect: {builds: true, protocols: {}}
	}
	"parent-item": {
		about: "parent:: reaches a schema one level up from the importing schema's namespace"
		packages: app: schemas: {
			"shapes/models": _impModels
			"shapes/api":    "use parent::models::Blob\n\n" + _impUsesBlob
		}
		build: "app"
		expect: {builds: true, protocols: {}}
	}
	"self-item": {
		about: "self:: reaches a schema under the importing schema's own namespace"
		packages: app: schemas: {
			"api":        "use self::models::Blob\n\n" + _impUsesBlob
			"api/models": _impModels
		}
		build: "app"
		expect: {builds: true, protocols: {}}
	}

	// A first segment that is not the package's: a dependency, and `std`.
	"dependency-path": {
		about: "a bare path whose first segment is a dependency reaches the dependency's schema"
		packages: {
			models: schemas: api: _impModels
			app: {
				schemas: api: "use models::api::Blob\n\n" + _impUsesBlob
				dependencies: models: {}
			}
		}
		build: "app"
		expect: {builds: true, protocols: {}}
	}
	"std-path": {
		about: "a bare path whose first segment is std reaches the standard library's schema"
		packages: app: schemas: api: "use std::http::Request\n\nstruct Call {\n    request: Request\n}"
		build: "app"
		expect: {builds: true, protocols: {}}
	}
	"unknown-first-segment": {
		about: "a bare path that is neither a dependency, std nor one of the package's schemas is unresolved, not this error; the use still brings Blob into scope, so the bare Blob after it is no second error, as in Rust"
		packages: app: schemas: {models: _impModels, api: "use modles::Blob\n\n" + _impUsesBlob}
		build: "app"
		expect: {builds: false, errors: ["import::unresolved"]}
	}

	// What keeps a first segment from meaning two things.
	"own-schema-named-like-dependency": {
		about: "a package's own schema named like one of its dependencies is refused, so a first segment is never both"
		packages: {
			models: schemas: api: _impModels
			app: {
				schemas: models: _impModels
				dependencies: models: {}
			}
		}
		build: "app"
		expect: {builds: false, error_contains: ["schema 'models.ids' is named like the dependency 'models'"]}
	}
	"dependency-named-std": {
		about: "std can't be a dependency's name"
		packages: {
			std: schemas: api: _impModels
			app: {
				schemas: api: _impPlain
				dependencies: std: {}
			}
		}
		build: "app"
		expect: {builds: false, error_contains: ["reserved"]}
	}
	"dependency-named-package": {
		about: "package can't be a dependency's name"
		packages: {
			package: schemas: api: _impModels
			app: {
				schemas: api: _impPlain
				dependencies: package: {}
			}
		}
		build: "app"
		expect: {builds: false, error_contains: ["reserved"]}
	}
	"dependency-named-parent": {
		about: "parent can't be a dependency's name"
		packages: {
			parent: schemas: api: _impModels
			app: {
				schemas: api: _impPlain
				dependencies: parent: {}
			}
		}
		build: "app"
		expect: {builds: false, error_contains: ["reserved"]}
	}
	"dependency-named-self": {
		about: "self can't be a dependency's name"
		packages: {
			self: schemas: api: _impModels
			app: {
				schemas: api: _impPlain
				dependencies: self: {}
			}
		}
		build: "app"
		expect: {builds: false, error_contains: ["reserved"]}
	}
}
