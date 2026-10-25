// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `'static` names for serde.
//!
//! serde's data model takes struct, field and variant names as `&'static str`,
//! as generated types have them. A [`Value`](crate::Value) or
//! [`Type`](crate::Type) only has them at run time, so each distinct name is
//! leaked once and reused: memory grows with the number of distinct names
//! seen, not with the number of values encoded or decoded.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// `name` as a `&'static str`, leaked the first time it's seen.
pub fn name(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .expect("unpoisoned");
    if let Some(&known) = names.get(name) {
        return known;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(leaked);
    leaked
}

/// `names` as a `&'static [&'static str]`, for a struct's fields or an enum's
/// variants, leaked the first time that list is seen.
pub fn list<'a>(names: impl IntoIterator<Item = &'a str>) -> &'static [&'static str] {
    static LISTS: OnceLock<Mutex<HashMap<Vec<&'static str>, &'static [&'static str]>>> =
        OnceLock::new();
    let key: Vec<&'static str> = names.into_iter().map(name).collect();
    let mut lists = LISTS
        .get_or_init(Default::default)
        .lock()
        .expect("unpoisoned");
    if let Some(&known) = lists.get(&key) {
        return known;
    }
    let leaked: &'static [&'static str] = Box::leak(key.clone().into_boxed_slice());
    lists.insert(key, leaked);
    leaked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_leaked_once() {
        assert!(std::ptr::eq(name("Point"), name(&String::from("Point"))));
        assert!(std::ptr::eq(list(["x", "y"]), list(["x", "y"])));
        assert!(!std::ptr::eq(list(["x", "y"]), list(["y", "x"])));
    }
}
