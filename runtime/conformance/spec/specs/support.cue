package conformance

// `support/<runtime>.json`: what a runtime claims, per feature.
#Support: close({
	$schema?: string
	runtime!: string
	support!: [string]: #Claim
})

// A claim. Only a partial one lists the cases known to fail.
#Claim:
	close({
		state!:    "supported" | "unsupported"
		note?:     string
		tracking?: string
	}) |
	close({
		state!:    "partial"
		note?:     string
		tracking?: string
		failing!: [#CaseId, ...#CaseId]
	})
