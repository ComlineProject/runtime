package cases

// Dependency acceptance: every unverified protocol of a dependency has to be
// accepted by the consumer, under the dependency in its `config.idp`, for
// exactly the guarantees it gives up. It is an error without a code, so these
// cases say what the message contains.

// A package whose `Billing` runs no handshake, marked.
_billing: {
	config: [_allow]
	schemas: api: """
		@handshake = "none"
		@unverified = "the 2009 gateway"
		protocol Billing {
		    function charge();
		}
		"""
}

_consumerSchema: """
	struct A {
	    x: u32
	}
	"""

groups: acceptance: {
	"not-accepted": {
		about: "an unverified protocol of a dependency has to be accepted"
		packages: {
			billing: _billing
			app: {schemas: api: _consumerSchema, dependencies: billing: {}}
		}
		build: "app"
		expect: {
			builds: false
			error_contains: [
				"protocol `Billing` is unverified, giving up schema agreement and encoding agreement",
				"the 2009 gateway",
			]
		}
	}
	"accepted": {
		about: "accepted for exactly what it gives up, it builds"
		packages: {
			billing: _billing
			app: {
				schemas: api: _consumerSchema
				dependencies: billing: accept: Billing: ["schema_agreement", "encoding_agreement"]
			}
		}
		build: "app"
		expect: {builds: true, protocols: {}}
	}
	"accepted-for-less": {
		about: "accepted for less than it gives up, it stops again"
		packages: {
			billing: _billing
			app: {
				schemas: api: _consumerSchema
				dependencies: billing: accept: Billing: ["schema_agreement"]
			}
		}
		build: "app"
		expect: {
			builds: false
			error_contains: [
				"protocol `Billing` now gives up schema agreement and encoding agreement",
				"it was accepted for schema agreement",
			]
		}
	}
	"accepted-for-more": {
		about: "accepted for more than it gives up, it stops too: the set has to match exactly"
		packages: {
			billing: _billing
			app: {
				schemas: api: _consumerSchema
				dependencies: billing: accept: Billing: ["schema_agreement", "encoding_agreement", "bounded_size"]
			}
		}
		build: "app"
		expect: {
			builds: false
			error_contains: ["protocol `Billing` now gives up schema agreement and encoding agreement"]
		}
	}
	"package-reasons-shown": {
		about: "what the package said once is what the consumer is shown to accept"
		packages: {
			billing: {
				config: ["handshake = \"none\"", """
					unverified = {
					    schema_agreement = "the gateway pins schemas by contract"
					    encoding_agreement = "the gateway only speaks one encoding"
					}
					""", _allow]
				schemas: api: """
					protocol Billing {
					    function charge();
					}
					"""
			}
			app: {schemas: api: _consumerSchema, dependencies: billing: {}}
		}
		build: "app"
		expect: {
			builds: false
			error_contains: [
				"protocol `Billing` is unverified, giving up schema agreement and encoding agreement",
				"schema agreement \"the gateway pins schemas by contract\"",
				"encoding agreement \"the gateway only speaks one encoding\"",
			]
		}
	}
	"judged-by-its-own-package": {
		about: "a dependency is judged by its own package's settings and default handshake, not the consumer's"
		packages: {
			billing: {
				config: ["handshake = \"none\"", _allow]
				schemas: api: """
					@unverified = "the 2009 gateway"
					protocol Billing {
					    function charge();
					}
					"""
			}
			app: {
				schemas: api: _consumerSchema
				dependencies: billing: accept: Billing: ["schema_agreement", "encoding_agreement"]
			}
		}
		build: "app"
		expect: {builds: true, protocols: {}}
	}
}
