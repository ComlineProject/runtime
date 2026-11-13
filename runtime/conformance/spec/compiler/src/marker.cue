package cases

groups: marker: {
	"gate-off": {
		about: "@unverified is refused where the package's settings don't allow it"
		packages: app: schemas: api: """
			@handshake = "none"
			@unverified = "the gateway predates Comline"
			protocol P {
			    function f();
			}
			"""
		build: "app"
		expect: {builds: false, errors: ["settings::forbidden"]}
	}
	"missing-marker": {
		about: "a protocol with no handshake gives up two guarantees and has to say why"
		packages: app: {
			config: [_allow]
			schemas: api: """
				@handshake = "none"
				protocol P {
				    function f();
				}
				"""
		}
		build: "app"
		expect: {builds: false, errors: ["unverified::missing-marker"]}
	}
	"plain-marker": {
		about: "one reason covers everything a protocol gives up, and it is frozen as the set"
		packages: app: {
			config: [_allow]
			schemas: api: """
				@handshake = "none"
				@unverified = "the gateway predates Comline"
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
					encoding_agreement: {scope: "protocol", reason: "the gateway predates Comline"}
				}
			}
			warnings: ["unverified::schema-agreement", "unverified::encoding-agreement"]
		}
	}
	"package-handshake-unmarked": {
		about: "the package's default handshake counts: a protocol that takes none has to say why"
		packages: app: {
			config: ["handshake = \"none\"", _allow]
			schemas: api: """
				protocol P {
				    function f();
				}
				"""
		}
		build: "app"
		expect: {builds: false, errors: ["unverified::missing-marker"]}
	}
	"package-handshake-marked": {
		about: "the marker covers what the package's default handshake gives up"
		packages: app: {
			config: ["handshake = \"none\"", _allow]
			schemas: api: """
				@unverified = "legacy"
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
					schema_agreement: {scope: "protocol", reason: "legacy"}
					encoding_agreement: {scope: "protocol", reason: "legacy"}
				}
			}
			warnings: ["unverified::schema-agreement", "unverified::encoding-agreement"]
		}
	}
	"missing-reason": {
		about: "a marker with a blank reason is an error"
		packages: app: {
			config: [_allow]
			schemas: api: """
				@handshake = "none"
				@unverified = "  "
				protocol P {
				    function f();
				}
				"""
		}
		build: "app"
		expect: {builds: false, errors: ["unverified::missing-reason"]}
	}
	"nothing-given-up": {
		about: "a marker where nothing is given up is a warning, and freezes nothing"
		packages: app: {
			config: [_allow]
			schemas: api: """
				@unverified = "just in case"
				protocol P {
				    function f();
				}
				"""
		}
		build: "app"
		expect: {
			builds: true
			protocols: P: given_up: []
			warnings: ["unverified::nothing-given-up"]
		}
	}
}
