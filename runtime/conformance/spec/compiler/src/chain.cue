package cases

// Three packages: `c` depends on `b`, which depends on `a`. `a`'s `Blob`
// holds a string without a maximum that only its own marked `Upload`
// reaches; `b` holds it in `Wrapper`, which only its own marked `Wrap`
// reaches; `c` uses `Wrapper` in `Orders`.
//
// Resolving a dependency takes accepting every unverified protocol of its
// package and of the packages it depends on, as its own build saw them: `c`
// accepts `Upload` as well as `Wrap`.

_a: {
	config: [_allow]
	schemas: api: """
		struct Blob {
		    data: string
		}

		@unverified = "the uploader streams whatever it's given"
		protocol Upload {
		    function put(b: Blob);
		}
		"""
}

// `b`, which depends on `a`.
_b: {
	config: [_allow]
	schemas: api: """
		use a::api::Blob

		struct Wrapper {
		    blob: Blob
		}

		@unverified = "wrapping uploads"
		protocol Wrap {
		    function wrap(w: Wrapper);
		}
		"""
	dependencies: a: {}
}

// `b` accepting `a`'s `Upload`.
_bAccepting: _b & {dependencies: a: accept: Upload: ["bounded_size"]}

groups: chain: {
	"unmarked-is-unbounded": {
		about: "a type that holds another package's unbounded value, two packages away, is unbounded where an unmarked protocol uses it"
		packages: {
			a: _a
			b: _bAccepting
			c: {
				schemas: api: """
					use b::api::Wrapper

					protocol Orders {
					    function send(w: Wrapper);
					}
					"""
				dependencies: b: accept: {
					Wrap: ["bounded_size"]
					Upload: ["bounded_size"]
				}
			}
		}
		build: "c"
		expect: {builds: false, errors: ["bounds::unbounded"]}
	}
	"marked-gives-up": {
		about: "a marked protocol using it gives bounded_size up, for its own consumers to accept"
		packages: {
			a: _a
			b: _bAccepting
			c: {
				config: [_allow]
				schemas: api: """
					use b::api::Wrapper

					@unverified = "orders carry the upload"
					protocol Orders {
					    function send(w: Wrapper);
					}
					"""
				dependencies: b: accept: {
					Wrap: ["bounded_size"]
					Upload: ["bounded_size"]
				}
			}
		}
		build: "c"
		expect: {
			builds: true
			protocols: Orders: {
				given_up: ["bounded_size"]
				acknowledged: bounded_size: {scope: "protocol", reason: "orders carry the upload"}
			}
			warnings: ["unverified::bounded-size"]
		}
	}
	"acceptance-needs-every-link": {
		about: "accepting b's protocol is not enough: a's, which b's package contains, is accepted too"
		packages: {
			a: _a
			b: _bAccepting
			c: {
				config: [_allow]
				schemas: api: """
					use b::api::Wrapper

					@unverified = "orders carry the upload"
					protocol Orders {
					    function send(w: Wrapper);
					}
					"""
				dependencies: b: accept: Wrap: ["bounded_size"]
			}
		}
		build: "c"
		expect: {
			builds: false
			error_contains: ["protocol `Upload` is unverified, giving up bounded size"]
		}
	}
	"middle-doesnt-accept-the-first": {
		about: "b's own build fails when it doesn't accept a's protocol, so building c does"
		packages: {
			a: _a
			b: _b
			c: {
				schemas: api: """
					use b::api::Wrapper

					protocol Orders {
					    function send(w: Wrapper);
					}
					"""
				dependencies: b: accept: {
					Wrap: ["bounded_size"]
					Upload: ["bounded_size"]
				}
			}
		}
		build: "c"
		expect: {
			builds: false
			error_contains: ["protocol `Upload` is unverified, giving up bounded size"]
		}
	}
}
