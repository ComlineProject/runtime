// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The Comline specification's catalog of shared names, as Rust constants:
//! the guarantees, the facts and the values each may take, the official
//! encodings with their aliases, and the wire features, with the sets they
//! form. `catalog/` in the specification is the source; [`catalog`]'s data is
//! generated from it by `specs/check.sh`, and CI fails when it's stale.
//!
//! The ids are published names: they never change. A rename adds an entry and
//! deprecates the old one ([`Status::Deprecated`], `replaced_by`).

#![no_std]

mod catalog;

pub use catalog::{
    encoding, fact, feature, guarantee, sets, ENCODINGS, FACTS, FEATURES, GUARANTEES,
    SPEC_VERSION,
};

/// What a fact applies to, and what kind of encoding a name is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Format,
    Framing,
    Handshake,
}

impl Kind {
    /// As the catalog and profiles write it: `format`, `framing`, `handshake`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Kind::Format => "format",
            Kind::Framing => "framing",
            Kind::Handshake => "handshake",
        }
    }
}

/// What gives a guarantee up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// The handshake.
    Handshake,
    /// The format and framing.
    Encoding,
    /// A handshake policy.
    Policy,
    /// A type.
    Type,
}

/// Where an entry stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Stable,
    Experimental,
    /// Kept for what already uses it; `replaced_by` names what to use instead.
    Deprecated,
}

/// A guarantee Comline gives between peers, which a protocol can give up
/// with `@unverified`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guarantee {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub comes_from: Origin,
    pub status: Status,
    pub replaced_by: Option<&'static str>,
    /// The spec version that introduced it, where the changelog names one.
    pub since: Option<&'static str>,
}

/// One value a fact may take, and what it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Value {
    pub id: &'static str,
    pub meaning: &'static str,
}

/// A fact an encoding's profile states. A handshake's facts are booleans
/// (`"true"`, `"false"`), and `true` establishes the guarantee named in
/// `establishes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fact {
    pub id: &'static str,
    pub applies_to: Kind,
    pub title: &'static str,
    pub description: &'static str,
    pub values: &'static [Value],
    pub establishes: Option<&'static str>,
    pub status: Status,
    pub replaced_by: Option<&'static str>,
    pub since: Option<&'static str>,
}

impl Fact {
    /// Whether `value` is one this fact may take.
    pub fn allows(&self, value: &str) -> bool {
        self.values.iter().any(|v| v.id == value)
    }
}

/// An official encoding, by its wire name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoding {
    pub id: &'static str,
    pub kind: Kind,
    pub title: &'static str,
    pub description: &'static str,
    /// Other spellings a schema may write, resolved to `id`.
    pub aliases: &'static [&'static str],
    /// The spelling core freezes into the IR, where it isn't `id`.
    pub frozen_as: Option<&'static str>,
    /// Whether `profiles/<id>.json` states its facts.
    pub profile: bool,
    /// The feature whose vectors prove it.
    pub feature: Option<&'static str>,
    pub tags: &'static [&'static str],
    pub status: Status,
    pub replaced_by: Option<&'static str>,
    pub since: Option<&'static str>,
}

impl Encoding {
    /// The spelling core freezes: `frozen_as`, or else the wire name.
    pub const fn frozen(&self) -> &'static str {
        match self.frozen_as {
            Some(spelling) => spelling,
            None => self.id,
        }
    }
}

/// A wire feature: a set of vector cases, `vectors/<id>.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feature {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub status: Status,
    pub replaced_by: Option<&'static str>,
    pub since: Option<&'static str>,
}

/// The guarantee `id`.
pub fn guarantee(id: &str) -> Option<&'static Guarantee> {
    GUARANTEES.iter().find(|g| g.id == id)
}

/// The fact `id`.
pub fn fact(id: &str) -> Option<&'static Fact> {
    FACTS.iter().find(|f| f.id == id)
}

/// The facts a profile of `kind` states, in the order profiles list them.
pub fn facts_of(kind: Kind) -> impl Iterator<Item = &'static Fact> {
    FACTS.iter().filter(move |f| f.applies_to == kind)
}

/// The official encoding whose wire name is `id`.
pub fn encoding(id: &str) -> Option<&'static Encoding> {
    ENCODINGS.iter().find(|e| e.id == id)
}

/// The official encoding `name` spells: its wire name or one of its aliases.
pub fn resolve(name: &str) -> Option<&'static Encoding> {
    encoding(name).or_else(|| ENCODINGS.iter().find(|e| e.aliases.contains(&name)))
}

/// The feature `id`.
pub fn feature(id: &str) -> Option<&'static Feature> {
    FEATURES.iter().find(|f| f.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_id_constant_resolves() {
        for id in sets::GUARANTEES {
            assert!(guarantee(id).is_some(), "{id}");
        }
        for id in sets::OFFICIAL {
            assert!(encoding(id).is_some(), "{id}");
        }
        for id in sets::FEATURES {
            assert!(feature(id).is_some(), "{id}");
        }
        assert_eq!(guarantee(guarantee::BOUNDED_SIZE).unwrap().title, "bounded size");
        assert_eq!(encoding(encoding::PACKED_1).unwrap().kind, Kind::Format);
    }

    #[test]
    fn aliases_resolve_to_their_wire_name() {
        for (alias, id) in sets::ALIASES {
            assert_eq!(resolve(alias).map(|e| e.id), Some(*id), "{alias}");
        }
        assert_eq!(resolve("json-rpc").unwrap().id, encoding::JSONRPC_1);
        assert_eq!(resolve(encoding::DATAGRAM_1).unwrap().frozen(), "datagram");
        assert_eq!(resolve(encoding::PACKED_1).unwrap().frozen(), encoding::PACKED_1);
        assert!(resolve("packed@2").is_none());
    }

    #[test]
    fn facts_match_their_sets_in_order() {
        let ids = |kind| facts_of(kind).map(|f| f.id);
        assert!(ids(Kind::Format).eq(sets::FORMAT_FACTS.iter().copied()));
        assert!(ids(Kind::Framing).eq(sets::FRAMING_FACTS.iter().copied()));
        assert!(ids(Kind::Handshake).eq(sets::HANDSHAKE_FACTS.iter().copied()));
        assert!(fact(fact::CALLS).unwrap().allows("by_id"));
        assert!(!fact(fact::CALLS).unwrap().allows("by_guess"));
    }

    #[test]
    fn handshake_facts_establish_guarantees() {
        for f in facts_of(Kind::Handshake) {
            let g = f.establishes.expect("a handshake fact establishes a guarantee");
            assert!(guarantee(g).is_some(), "{g}");
            assert!(f.allows("true") && f.allows("false"));
        }
    }

    #[test]
    fn the_spec_version_is_a_version() {
        let (major, minor) = SPEC_VERSION.split_once('.').unwrap();
        assert!(major.parse::<u32>().is_ok() && minor.parse::<u32>().is_ok());
    }
}
