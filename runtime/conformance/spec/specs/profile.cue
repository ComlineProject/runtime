package conformance

// `profiles/<name>.json`: facts about how an encoding encodes, each naming
// the vector cases that prove it.
#Profile: #FormatProfile | #FramingProfile | #HandshakeProfile

// A format states every fact.
#FormatProfile: close({
	$schema?: string
	name!:    #ProfileName
	kind!:    "format"
	facts!: close({
		struct_fields!:    close({value!: "by_position" | "by_name", proven_by!: #ProvenBy})
		unknown_field!:    close({value!: "rejected" | "ignored", proven_by!: #ProvenBy})
		missing_optional!: close({value!: "rejected" | "absent", proven_by!: #ProvenBy})
		enum_variants!:    close({value!: "by_name" | "by_index", proven_by!: #ProvenBy})
		unknown_variant!:  close({value!: "rejected" | "tolerated", proven_by!: #ProvenBy})
		integer_width!:    close({value!: "by_value" | "exact", proven_by!: #ProvenBy})
		output!:           close({value!: "bytes" | "json", proven_by!: #ProvenBy})
	})
})

// A framing states every fact.
#FramingProfile: close({
	$schema?: string
	name!:    #ProfileName
	kind!:    "framing"
	facts!: close({
		calls!:  close({value!: "by_index" | "by_name" | "by_id", proven_by!: #ProvenBy})
		directions!: close({value!: "both" | "one", proven_by!: #ProvenBy})
		errors!: close({value!: "by_ordinal" | "by_name", proven_by!: #ProvenBy})
		payload!: close({value!: "opaque" | "json", proven_by!: #ProvenBy})
	})
})

// A handshake may leave a fact out, which reads as `false`: not established.
#HandshakeProfile: close({
	$schema?: string
	name!:    #ProfileName
	kind!:    "handshake"
	facts!: close({
		schema_agreement?:   #Agreement
		encoding_agreement?: #Agreement
	})
})

// The cases that prove a fact.
#ProvenBy: [#CaseId, ...#CaseId]

// `true` needs the cases that show it; `false` claims nothing.
#Agreement:
	close({value!: true, proven_by!: #ProvenBy}) |
	close({value!: false, proven_by?: []})
