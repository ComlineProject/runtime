package conformance

import "github.com/comlineproject/specification/catalog"

// `profiles/<name>.json`: facts about how an encoding encodes, each naming
// the vector cases that prove it. Which facts each kind states, and the
// values each may take, are the catalog's (`catalog/facts.cue`).
#Profile: #FormatProfile | #FramingProfile | #HandshakeProfile

// A format states every fact.
#FormatProfile: close({
	$schema?: string
	name!:    #ProfileName
	kind!:    "format"
	facts!: close({
		for id, fact in catalog.facts if fact.applies_to == "format" {
			(id)!: close({value!: or([for v, _ in fact.values {v}]), proven_by!: #ProvenBy})
		}
	})
})

// A framing states every fact.
#FramingProfile: close({
	$schema?: string
	name!:    #ProfileName
	kind!:    "framing"
	facts!: close({
		for id, fact in catalog.facts if fact.applies_to == "framing" {
			(id)!: close({value!: or([for v, _ in fact.values {v}]), proven_by!: #ProvenBy})
		}
	})
})

// A handshake may leave a fact out, which reads as `false`: not established.
#HandshakeProfile: close({
	$schema?: string
	name!:    #ProfileName
	kind!:    "handshake"
	facts!: close({
		for id, fact in catalog.facts if fact.applies_to == "handshake" {
			(id)?: #Agreement
		}
	})
})

// The cases that prove a fact.
#ProvenBy: [#CaseId, ...#CaseId]

// `true` needs the cases that show it; `false` claims nothing.
#Agreement:
	close({value!: true, proven_by!: #ProvenBy}) |
	close({value!: false, proven_by?: []})
