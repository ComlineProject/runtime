// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Running a schema's `@validators` against a value.
//!
//! Generated code implements [`Validate`] for every type that carries a
//! check, directly or through a field, an element or a `type` alias. The
//! checks themselves are inlined there; this module only holds what they
//! report and the plumbing that walks nested values.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// One failed check: where the value sits and the validator's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// The path to the value from the one validated: `name`,
    /// `members[3]`, `author.name`. Empty for the validated value itself.
    pub field: String,
    /// The validator's message, placeholders filled in.
    pub message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl core::error::Error for ValidationError {}

/// Every failed check on one value, in the order they ran. Never empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationErrors(pub Vec<ValidationError>);

impl fmt::Display for ValidationErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, error) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("; ")?;
            }
            fmt::Display::fmt(error, f)?;
        }
        Ok(())
    }
}

impl core::error::Error for ValidationErrors {}

/// A value whose schema attaches checks to it or to something inside it.
pub trait Validate {
    /// Append this value's failures to `out`, naming each by its path under
    /// `path`. With `first`, stop as soon as `out` is non-empty.
    ///
    /// Plumbing for generated code; call [`Validate::validate`] or
    /// [`Validate::validate_first`] instead.
    #[doc(hidden)]
    fn validate_into(&self, path: &str, out: &mut Vec<ValidationError>, first: bool);

    /// Run every check, collecting all failures.
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut out = Vec::new();
        self.validate_into("", &mut out, false);
        if out.is_empty() {
            Ok(())
        } else {
            Err(ValidationErrors(out))
        }
    }

    /// Run the checks until one fails.
    fn validate_first(&self) -> Result<(), ValidationError> {
        let mut out = Vec::new();
        self.validate_into("", &mut out, true);
        match out.into_iter().next() {
            None => Ok(()),
            Some(error) => Err(error),
        }
    }
}

/// `path.field`, or just `field` at the top.
#[doc(hidden)]
pub fn field_path(path: &str, field: &str) -> String {
    if path.is_empty() {
        String::from(field)
    } else {
        format!("{path}.{field}")
    }
}

/// `path[index]`.
#[doc(hidden)]
pub fn index_path(path: &str, index: usize) -> String {
    format!("{path}[{index}]")
}

/// An absent optional value has nothing to check.
impl<T: Validate> Validate for Option<T> {
    fn validate_into(&self, path: &str, out: &mut Vec<ValidationError>, first: bool) {
        if let Some(value) = self {
            value.validate_into(path, out, first);
        }
    }
}

impl<T: Validate> Validate for Box<T> {
    fn validate_into(&self, path: &str, out: &mut Vec<ValidationError>, first: bool) {
        (**self).validate_into(path, out, first);
    }
}

impl<T: Validate> Validate for [T] {
    fn validate_into(&self, path: &str, out: &mut Vec<ValidationError>, first: bool) {
        for (i, element) in self.iter().enumerate() {
            if first && !out.is_empty() {
                return;
            }
            element.validate_into(&index_path(path, i), out, first);
        }
    }
}

impl<T: Validate> Validate for Vec<T> {
    fn validate_into(&self, path: &str, out: &mut Vec<ValidationError>, first: bool) {
        self.as_slice().validate_into(path, out, first);
    }
}

impl<T: Validate, const N: usize> Validate for [T; N] {
    fn validate_into(&self, path: &str, out: &mut Vec<ValidationError>, first: bool) {
        self.as_slice().validate_into(path, out, first);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    /// Stands in for a generated type: fails when `name` is empty.
    struct User {
        name: String,
    }

    impl Validate for User {
        fn validate_into(&self, path: &str, out: &mut Vec<ValidationError>, _first: bool) {
            if self.name.is_empty() {
                let field = field_path(path, "name");
                out.push(ValidationError { message: format!("{field} is empty"), field });
            }
        }
    }

    fn user(name: &str) -> User {
        User { name: name.to_string() }
    }

    #[test]
    fn a_valid_value_is_ok() {
        assert_eq!(user("ada").validate(), Ok(()));
        assert_eq!(user("ada").validate_first(), Ok(()));
    }

    #[test]
    fn a_failure_is_named_by_its_path() {
        let errors = user("").validate().unwrap_err();
        assert_eq!(errors.0, [ValidationError { field: "name".into(), message: "name is empty".into() }]);
        assert_eq!(errors.to_string(), "name is empty");
    }

    #[test]
    fn elements_are_indexed_and_collected() {
        let users = vec![user("ada"), user(""), user("")];
        let fields: Vec<String> = users.validate().unwrap_err().0.into_iter().map(|e| e.field).collect();
        assert_eq!(fields, ["[1].name", "[2].name"]);
    }

    #[test]
    fn validate_first_stops_at_one() {
        let users = [user(""), user("")];
        assert_eq!(users.validate_first().unwrap_err().field, "[0].name");
    }

    #[test]
    fn an_absent_optional_is_skipped() {
        assert_eq!(None::<User>.validate(), Ok(()));
        assert!(Some(user("")).validate().is_err());
    }

    #[test]
    fn several_failures_read_as_one_message() {
        let errors = ValidationErrors(vec![
            ValidationError { field: "a".into(), message: "a is bad".into() },
            ValidationError { field: "b".into(), message: "b is bad".into() },
        ]);
        assert_eq!(errors.to_string(), "a is bad; b is bad");
    }
}
