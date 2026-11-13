package cases

// Bounded size: every string, list and nesting a protocol reaches has a
// maximum, or the protocol gives up `bounded_size` with `@unverified`.

_streams: "the uploader streams whatever it's given"

// A type another schema of the package declares, held by a protocol there
// that is marked, so the string without a maximum is its to give up.
_models: """
	struct Blob {
	    data: string
	}

	@unverified = "\(_streams)"
	protocol Upload {
	    function put(b: Blob);
	}
	"""

groups: bounded: {
	"string-unmarked": {
		about: "a string without a maximum is an error where no marker gives bounded size up"
		packages: app: schemas: api: """
			protocol P {
			    function put(s: string);
			}
			"""
		build: "app"
		expect: {builds: false, errors: ["bounds::unbounded"]}
	}
	"list-unmarked": {
		about: "a list without a maximum is an error too"
		packages: app: schemas: api: """
			protocol P {
			    function put(xs: u8[]);
			}
			"""
		build: "app"
		expect: {builds: false, errors: ["bounds::unbounded"]}
	}
	"bounded": {
		about: "a string and a list with maximums give nothing up"
		packages: app: schemas: api: """
			protocol P {
			    function put(s: char[max 64], xs: u8[max 16]);
			}
			"""
		build: "app"
		expect: {
			builds: true
			protocols: P: given_up: []
			warnings: []
		}
	}
	"string-marked": {
		about: "a marked protocol gives bounded_size up for a string without a maximum"
		packages: app: {
			config: [_allow]
			schemas: api: """
				@unverified = "\(_streams)"
				protocol P {
				    function put(s: string);
				}
				"""
		}
		build: "app"
		expect: {
			builds: true
			protocols: P: {
				given_up: ["bounded_size"]
				acknowledged: bounded_size: {scope: "protocol", reason: _streams}
			}
			warnings: ["unverified::bounded-size"]
		}
	}
	"keyed-for-something-else": {
		about: "a keyed marker that doesn't list bounded_size doesn't cover a string without a maximum"
		packages: app: {
			config: [_allow]
			schemas: api: """
				@unverified = { version_honesty = "not this" }
				protocol P {
				    function put(s: string);
				}
				"""
		}
		build: "app"
		expect: {builds: false, errors: ["unverified::missing-guarantee"]}
	}
	"recursion-unmarked": {
		about: "a type that contains itself without @max_depth is an error"
		packages: app: schemas: api: """
			struct Node {
			    kids: Node[max 4]
			}

			protocol P {
			    function f(n: Node);
			}
			"""
		build: "app"
		expect: {builds: false, errors: ["bounds::unbounded-depth"]}
	}
	"recursion-marked": {
		about: "a marked protocol gives bounded_size up for a type that contains itself without @max_depth"
		packages: app: {
			config: [_allow]
			schemas: api: """
				struct Node {
				    kids: Node[max 4]
				}

				@unverified = "trees are small in practice"
				protocol P {
				    function f(n: Node);
				}
				"""
		}
		build: "app"
		expect: {
			builds: true
			protocols: P: {
				given_up: ["bounded_size"]
				acknowledged: bounded_size: {scope: "protocol", reason: "trees are small in practice"}
			}
			warnings: ["unverified::bounded-size"]
		}
	}
	"recursion-max-depth": {
		about: "a type that contains itself under @max_depth gives nothing up"
		packages: app: schemas: api: """
			@max_depth = 4
			struct Node {
			    kids: Node[max 4]
			}

			protocol P {
			    function f(n: Node);
			}
			"""
		build: "app"
		expect: {
			builds: true
			protocols: P: given_up: []
			warnings: []
		}
	}
	"other-schema-unmarked": {
		about: "a type from another schema that holds a string without a maximum is unbounded where an unmarked protocol uses it"
		packages: app: {
			config: [_allow]
			schemas: {
				models: _models
				api: """
					use models::Blob

					protocol Orders {
					    function send(b: Blob);
					}
					"""
			}
		}
		build: "app"
		expect: {builds: false, errors: ["bounds::unbounded"]}
	}
	"other-schema-marked": {
		about: "a marked protocol using it gives bounded_size up, as the one that declares it does"
		packages: app: {
			config: [_allow]
			schemas: {
				models: _models
				api: """
					use models::Blob

					@unverified = "orders carry the upload"
					protocol Orders {
					    function send(b: Blob);
					}
					"""
			}
		}
		build: "app"
		expect: {
			builds: true
			protocols: {
				Upload: {
					given_up: ["bounded_size"]
					acknowledged: bounded_size: {scope: "protocol", reason: _streams}
				}
				Orders: {
					given_up: ["bounded_size"]
					acknowledged: bounded_size: {scope: "protocol", reason: "orders carry the upload"}
				}
			}
			warnings: ["unverified::bounded-size"]
		}
	}
	"other-schema-bounded": {
		about: "a type bounded where it is declared is bounded where it is used"
		packages: app: schemas: {
			models: """
				struct Blob {
				    data: char[max 64]
				}
				"""
			api: """
				use models::Blob

				protocol Orders {
				    function send(b: Blob);
				}
				"""
		}
		build: "app"
		expect: {
			builds: true
			protocols: Orders: given_up: []
			warnings: []
		}
	}
}
