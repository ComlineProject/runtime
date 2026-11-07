package conformance

// `reports/<runtime>.json`: a runtime's claims, with the cases proving each.
#Report: close({
	$schema?: string

	// Language runtime
	runtime!: string

	// Supported features
	support!: [string]: #ProvenClaim
})

// A claim, with the cases that prove it.
#ProvenClaim:
	close({
		state!:    "supported" | "unsupported"
		note?:     string
		tracking?: string
		proven_by!: [...#CaseId]
	}) |
	close({
		state!:    "partial"
		note?:     string
		tracking?: string
		failing!: [#CaseId, ...#CaseId]
		proven_by!: [...#CaseId]
	})
