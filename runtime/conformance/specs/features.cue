package conformance

// `features.json`: the wire features, with stable ids.
#Features: close({
	$schema?: string
	features!: [...close({
		id!:          string
		title!:       string
		description!: string
	})]
})
