# The catalog

Every name that more than one Comline program has to agree on, defined once.
Each entry has a stable id, a title and a description, and where they apply
a value domain, aliases, a status and plain references to other entries.
Sets of names are queries over the entries, so they can't drift from them.

| Kind | Ids | File |
| --- | --- | --- |
| Guarantees | `schema_agreement`, `bounded_size`, ... | [`guarantees.cue`](guarantees.cue) |
| Facts, with their values | `struct_fields`, `calls`, `schema_agreement`, ... | [`facts.cue`](facts.cue) |
| Official encodings, with their aliases | `packed@1`, `jsonrpc@1`, `comline@2`, `none`, ... | [`encodings.cue`](encodings.cue) |
| Wire features | `name-hash`, `framing-datagram3`, ... | [`features.cue`](features.cue) |
| Sets | `formats`, `framing_facts`, `aliases`, ... | [`sets.cue`](sets.cue) |

What an entry may say, and the rules across entries, are in
[`schema.cue`](schema.cue):

- An id is a published name, so it never changes. A rename adds an entry
  and deprecates the old one (`status: "deprecated"`, `replaced_by`).
- An alias resolves to exactly one encoding, and is never another
  encoding's name. Where core freezes another spelling than the wire name
  (`frozen_as`: `jsonrpc`, `datagram`), that spelling is an alias too.
- A reference (`establishes`, `feature`) names an entry that exists.
- A handshake's facts are booleans.
- `since` is the spec version that introduced an entry, where the changelog
  names one.

## What reads it

- [`specs/profile.cue`](../specs/profile.cue) takes each kind's facts and
  their values from here, so a profile is held to the catalog's domains.
- [`features.json`](../features.json) is generated from `features`.
- [`gen/catalog.json`](gen/catalog.json) is the whole catalog as JSON, for
  readers without CUE: the compiler, the runtimes, generators, the language
  server, the Observatory and the docs.
- `specs/check.sh` checks that `profiles/` holds exactly the encodings with a
  profile, each of its encoding's kind, and `vectors/` exactly the features.

A third party's encoding isn't an entry here: its package declares it in
`config.idp`, in the same shape, and the same rules judge it.

## Changing it

Edit the `.cue` files, then run `specs/check.sh` to regenerate
`features.json` and `gen/catalog.json`. CI runs `specs/check.sh --check`,
which fails when either is stale.

Order matters where it shows: facts are listed in the order profiles list
them, and features in the order `features.json` does.
