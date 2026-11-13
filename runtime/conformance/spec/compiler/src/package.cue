package cases

// The package's own `unverified`, in `config.idp`: why the package gives up
// what its own settings give up, once for every protocol that takes them.
// In `config.idp`, a dictionary's entries are separated by whitespace.

_why:          "the gateway pins schemas by contract"
_noHandshake:  "handshake = \"none\""
_packageKeyed: """
	unverified = {
	    schema_agreement = "\(_why)"
	    encoding_agreement = "\(_why)"
	}
	"""
_bare: """
	protocol P {
	    function f();
	}
	"""

groups: package: {
	"keyed-covers-inherited": {
		about: "the package says why for a protocol that takes the package's handshake"
		packages: app: {config: [_noHandshake, _packageKeyed, _allow], schemas: api: _bare}
		build: "app"
		expect: {
			builds: true
			protocols: P: {
				given_up: ["schema_agreement", "encoding_agreement"]
				acknowledged: {
					schema_agreement: {scope: "package", reason: _why}
					encoding_agreement: {scope: "package", reason: _why}
				}
			}
			warnings: ["unverified::schema-agreement", "unverified::encoding-agreement"]
		}
	}
	"plain-covers-inherited": {
		about: "the plain form covers every guarantee a package setting gives up"
		packages: app: {
			config: [_noHandshake, "unverified = \"\(_why)\"", _allow]
			schemas: api: _bare
		}
		build: "app"
		expect: {
			builds: true
			protocols: P: {
				given_up: ["schema_agreement", "encoding_agreement"]
				acknowledged: {
					schema_agreement: {scope: "package", reason: _why}
					encoding_agreement: {scope: "package", reason: _why}
				}
			}
			warnings: ["unverified::schema-agreement", "unverified::encoding-agreement"]
		}
	}
	"own-handshake-not-covered": {
		about: "a protocol with its own @handshake isn't covered by the package"
		packages: app: {
			config: [_noHandshake, _packageKeyed, _allow]
			schemas: api: """
				protocol P {
				    function f();
				}

				@handshake = "none"
				protocol Q {
				    function g();
				}
				"""
		}
		build: "app"
		expect: {builds: false, errors: ["unverified::missing-marker"]}
	}
	"own-handshake-marked": {
		about: "a protocol with its own @handshake says why itself; the package covers the other"
		packages: app: {
			config: [_noHandshake, _packageKeyed, _allow]
			schemas: api: """
				protocol P {
				    function f();
				}

				@handshake = "none"
				@unverified = "Q talks to the gateway directly"
				protocol Q {
				    function g();
				}
				"""
		}
		build: "app"
		expect: {
			builds: true
			protocols: {
				P: {
					given_up: ["schema_agreement", "encoding_agreement"]
					acknowledged: {
						schema_agreement: {scope: "package", reason: _why}
						encoding_agreement: {scope: "package", reason: _why}
					}
				}
				Q: {
					given_up: ["schema_agreement", "encoding_agreement"]
					acknowledged: {
						schema_agreement: {scope: "protocol", reason: "Q talks to the gateway directly"}
						encoding_agreement: {scope: "protocol", reason: "Q talks to the gateway directly"}
					}
				}
			}
			warnings: ["unverified::schema-agreement", "unverified::encoding-agreement"]
		}
	}
	"wrong-scope": {
		about: "bounded_size can't be acknowledged at package scope"
		packages: app: {
			config: ["unverified = { bounded_size = \"\(_why)\" }", _allow]
			schemas: api: _bare
		}
		build: "app"
		expect: {builds: false, errors: ["unverified::wrong-scope"]}
	}
	"not-given-up": {
		about: "a guarantee the package lists and no protocol gives up through it is a warning"
		packages: app: {config: [_packageKeyed, _allow], schemas: api: _bare}
		build: "app"
		expect: {
			builds: true
			protocols: P: given_up: []
			warnings: ["unverified::not-given-up"]
		}
	}
	"gate-off": {
		about: "the package's unverified is held to the same gate as the marker"
		packages: app: {config: [_noHandshake, _packageKeyed], schemas: api: _bare}
		build: "app"
		expect: {builds: false, errors: ["settings::forbidden"]}
	}
	"adds-bounded-size": {
		about: "a marker says why for bounded_size, which the package can't; the package says the rest"
		packages: app: {
			config: [_noHandshake, _packageKeyed, _allow]
			schemas: api: """
				@unverified = { bounded_size = "the uploader streams whatever it's given" }
				protocol P {
				    function put(s: string);
				}
				"""
		}
		build: "app"
		expect: {
			builds: true
			protocols: P: {
				given_up: ["schema_agreement", "encoding_agreement", "bounded_size"]
				acknowledged: {
					schema_agreement: {scope: "package", reason: _why}
					encoding_agreement: {scope: "package", reason: _why}
					bounded_size: {scope: "protocol", reason: "the uploader streams whatever it's given"}
				}
			}
			warnings: ["unverified::schema-agreement", "unverified::encoding-agreement", "unverified::bounded-size"]
		}
	}
	"bounded-size-not-covered": {
		about: "the package covering the handshake doesn't cover an unbounded string"
		packages: app: {
			config: [_noHandshake, _packageKeyed, _allow]
			schemas: api: """
				protocol P {
				    function put(s: string);
				}
				"""
		}
		build: "app"
		expect: {builds: false, errors: ["bounds::unbounded"]}
	}
}
