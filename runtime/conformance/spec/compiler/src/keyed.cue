package cases

// The keyed form, `@unverified = { <guarantee> = "<reason>" }`: a reason for
// each guarantee given up, under its id in the catalog. In a schema, the
// entries are separated by commas.
groups: keyed: {
	"exact-set": {
		about: "a reason for each guarantee given up, and no others"
		packages: app: {
			config: [_allow]
			schemas: api: """
				@handshake = "none"
				@unverified = {
				    schema_agreement = "the gateway predates Comline",
				    encoding_agreement = "the gateway only speaks one encoding"
				}
				protocol P {
				    function f();
				}
				"""
		}
		build: "app"
		expect: {
			builds: true
			protocols: P: {
				given_up: ["schema_agreement", "encoding_agreement"]
				acknowledged: {
					schema_agreement: {scope: "protocol", reason: "the gateway predates Comline"}
					encoding_agreement: {scope: "protocol", reason: "the gateway only speaks one encoding"}
				}
			}
			warnings: ["unverified::schema-agreement", "unverified::encoding-agreement"]
		}
	}
	"unknown-guarantee": {
		about: "a key that is no guarantee's id is an error"
		packages: app: {
			config: [_allow]
			schemas: api: """
				@handshake = "none"
				@unverified = {
				    schema_agreement = "the gateway predates Comline",
				    encoding_agreement = "the gateway only speaks one encoding",
				    bounded_sise = "a typo"
				}
				protocol P {
				    function f();
				}
				"""
		}
		build: "app"
		expect: {builds: false, errors: ["unverified::unknown-guarantee"]}
	}
	"missing-guarantee": {
		about: "a guarantee given up and not listed is an error"
		packages: app: {
			config: [_allow]
			schemas: api: """
				@handshake = "none"
				@unverified = { schema_agreement = "the gateway predates Comline" }
				protocol P {
				    function f();
				}
				"""
		}
		build: "app"
		expect: {builds: false, errors: ["unverified::missing-guarantee"]}
	}
	"not-given-up": {
		about: "a guarantee listed and not given up is a warning, and not frozen"
		packages: app: {
			config: [_allow]
			schemas: api: """
				@handshake = "none"
				@unverified = {
				    schema_agreement = "the gateway predates Comline",
				    encoding_agreement = "the gateway only speaks one encoding",
				    version_honesty = "just in case"
				}
				protocol P {
				    function f();
				}
				"""
		}
		build: "app"
		expect: {
			builds: true
			protocols: P: given_up: ["schema_agreement", "encoding_agreement"]
			warnings: ["unverified::not-given-up", "unverified::schema-agreement", "unverified::encoding-agreement"]
		}
	}
	"duplicate-guarantee": {
		about: "a guarantee listed twice is an error"
		packages: app: {
			config: [_allow]
			schemas: api: """
				@handshake = "none"
				@unverified = {
				    schema_agreement = "the gateway predates Comline",
				    encoding_agreement = "the gateway only speaks one encoding",
				    schema_agreement = "again"
				}
				protocol P {
				    function f();
				}
				"""
		}
		build: "app"
		expect: {builds: false, errors: ["unverified::duplicate-guarantee"]}
	}
}
