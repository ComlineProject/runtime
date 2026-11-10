// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Decoding by a [`Type`] known only at run time, exactly as a generated type
//! decodes.
//!
//! Generated Rust types are plain `serde` derives. [`TypeSeed`] makes the same
//! calls a derive makes, with the same visitor behaviour, so a format accepts
//! or refuses the same bytes for both:
//!
//! - a struct is `deserialize_struct` with its name and field names, read as a
//!   sequence (fields in order, each one required) or as a map (by name or
//!   index; unknown fields skipped; a repeated field refused; a missing field
//!   is `None` if it's an option, refused otherwise);
//! - an enum is `deserialize_enum`, its variant read as an identifier: an
//!   index below the count, or a declared name;
//! - `T[N]` is `deserialize_tuple(N)` and needs N elements, `T[]` is
//!   `deserialize_seq`;
//! - scalars, strings, options and unit go through `serde`'s own impls;
//! - a bounded string, a bounded array and a recursive type's depth go
//!   through `comline-runtime`'s [`bounds`](comline_runtime::bounds), as
//!   generated code will.
//!
//! `tests/seed_vs_derive.rs` holds it to that against real derives.

use std::cell::Cell;
use std::fmt;

use comline_runtime::bounds::{self, BoundExceeded, CharsSeed, Depth, ItemsSeed};

use serde::de::{
    self, DeserializeSeed, EnumAccess, IgnoredAny, MapAccess, SeqAccess, Unexpected, VariantAccess,
    Visitor,
};
use serde::{Deserialize, Deserializer};

use crate::{intern, Type, Value};

/// Decodes the [`Value`] of a [`Type`], as a derive for that type would.
#[derive(Clone, Copy)]
pub struct TypeSeed<'t>(pub &'t Type);

impl<'de> DeserializeSeed<'de> for TypeSeed<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        Seed {
            ty: self.0,
            rec: None,
        }
        .deserialize(d)
    }
}

/// The innermost [`Type::Recursive`] being read: what `"self"` stands for,
/// and how deep it is.
struct Recursion<'t> {
    of: &'t Type,
    depth: Cell<Depth>,
}

impl Recursion<'_> {
    /// Reads one more level of the recursive type, refused past its depth.
    fn level<'de, D: Deserializer<'de>>(&self, d: D) -> Result<Value, D::Error> {
        let mut depth = self.depth.get();
        depth.enter().map_err(bounds::de_error)?;
        self.depth.set(depth);
        let value = Seed {
            ty: self.of,
            rec: Some(self),
        }
        .deserialize(d);
        let mut depth = self.depth.get();
        depth.leave();
        self.depth.set(depth);
        value
    }
}

/// A [`TypeSeed`] inside a recursive type.
#[derive(Clone, Copy)]
struct Seed<'t> {
    ty: &'t Type,
    rec: Option<&'t Recursion<'t>>,
}

impl<'t> Seed<'t> {
    fn of(self, ty: &'t Type) -> Self {
        Seed { ty, rec: self.rec }
    }
}

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        Ok(match self.ty {
            Type::Bool => Value::Bool(bool::deserialize(d)?),
            Type::U8 => Value::U8(u8::deserialize(d)?),
            Type::U16 => Value::U16(u16::deserialize(d)?),
            Type::U32 => Value::U32(u32::deserialize(d)?),
            Type::U64 => Value::U64(u64::deserialize(d)?),
            Type::U128 => Value::U128(u128::deserialize(d)?),
            Type::I8 => Value::I8(i8::deserialize(d)?),
            Type::I16 => Value::I16(i16::deserialize(d)?),
            Type::I32 => Value::I32(i32::deserialize(d)?),
            Type::I64 => Value::I64(i64::deserialize(d)?),
            Type::I128 => Value::I128(i128::deserialize(d)?),
            Type::F32 => Value::F32(f32::deserialize(d)?),
            Type::F64 => Value::F64(f64::deserialize(d)?),
            Type::String => Value::String(String::deserialize(d)?),
            Type::Char => Value::Char(char::deserialize(d)?),
            Type::BoundedString { max } => {
                Value::String(CharsSeed::<String>::new(*max).deserialize(d)?)
            }
            Type::FixedString { len } => {
                let s = CharsSeed::<String>::new(*len).deserialize(d)?;
                // `char[N]` holds exactly N characters.
                if s.chars().count() as u64 != u64::from(*len) {
                    return Err(de::Error::invalid_length(s.chars().count(), &"`char[N]`"));
                }
                Value::String(s)
            }
            Type::BoundedArray { of, max } => {
                Value::Array(ItemsSeed::<_, Vec<Value>>::new(self.of(of), *max).deserialize(d)?)
            }
            Type::Recursive { max_depth, of } => {
                let rec = Recursion {
                    of,
                    depth: Cell::new(Depth::new(*max_depth)),
                };
                rec.level(d)?
            }
            Type::SelfRef => match self.rec {
                Some(rec) => rec.level(d)?,
                None => return Err(de::Error::custom("`self` outside a recursive type")),
            },
            Type::Unit => {
                <()>::deserialize(d)?;
                Value::Unit(())
            }
            Type::Option(inner) => Value::Option(
                d.deserialize_option(OptionVisitor(self.of(inner)))?
                    .map(Box::new),
            ),
            Type::Array(element) => Value::Array(d.deserialize_seq(SeqVisitor(self.of(element)))?),
            Type::Fixed { of, len } => Value::Tuple(d.deserialize_tuple(
                *len,
                FixedVisitor {
                    of: self.of(of),
                    len: *len,
                },
            )?),
            Type::Struct { name, fields } => d.deserialize_struct(
                intern::name(name),
                intern::list(fields.iter().map(|(f, _)| f.as_str())),
                StructVisitor {
                    name,
                    fields,
                    seed: self,
                },
            )?,
            Type::Enum { name, variants } => d.deserialize_enum(
                intern::name(name),
                intern::list(variants.iter().map(String::as_str)),
                EnumVisitor { name, variants },
            )?,
        })
    }
}

struct OptionVisitor<'t>(Seed<'t>);

impl<'de> Visitor<'de> for OptionVisitor<'_> {
    type Value = Option<Value>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("option")
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        self.0.deserialize(d).map(Some)
    }
}

struct SeqVisitor<'t>(Seed<'t>);

impl<'de> Visitor<'de> for SeqVisitor<'_> {
    type Value = Vec<Value>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a sequence")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        // A size hint is the input's claim, so it only bounds the first
        // allocation, as serde's own `Vec` impl does.
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096));
        while let Some(item) = seq.next_element_seed(self.0)? {
            items.push(item);
        }
        Ok(items)
    }
}

struct FixedVisitor<'t> {
    of: Seed<'t>,
    len: usize,
}

impl<'de> Visitor<'de> for FixedVisitor<'_> {
    type Value = Vec<Value>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "an array of length {}", self.len)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::with_capacity(self.len.min(4096));
        for i in 0..self.len {
            match seq.next_element_seed(self.of)? {
                Some(item) => items.push(item),
                None => return Err(de::Error::invalid_length(i, &self)),
            }
        }
        Ok(items)
    }
}

struct StructVisitor<'t> {
    name: &'t str,
    fields: &'t [(String, Type)],
    seed: Seed<'t>,
}

impl<'de> Visitor<'de> for StructVisitor<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "struct {} with {} elements",
            self.name,
            self.fields.len()
        )
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::with_capacity(self.fields.len());
        for (i, (field, ty)) in self.fields.iter().enumerate() {
            match seq.next_element_seed(self.seed.of(ty))? {
                Some(value) => values.push((field.clone(), value)),
                None => return Err(de::Error::invalid_length(i, &self)),
            }
        }
        Ok(Value::Struct {
            name: self.name.into(),
            fields: values,
        })
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut slots: Vec<Option<Value>> = vec![None; self.fields.len()];
        while let Some(key) = map.next_key_seed(FieldSeed(self.fields))? {
            match key {
                Some(i) => {
                    let (field, ty) = &self.fields[i];
                    if slots[i].is_some() {
                        return Err(de::Error::duplicate_field(intern::name(field)));
                    }
                    slots[i] = Some(map.next_value_seed(self.seed.of(ty))?);
                }
                None => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        let mut values = Vec::with_capacity(self.fields.len());
        for ((field, ty), slot) in self.fields.iter().zip(slots) {
            let value = match (slot, ty) {
                (Some(value), _) => value,
                // A derive's `missing_field` reads an absent option as `None`
                // and refuses anything else.
                (None, Type::Option(_)) => Value::Option(None),
                (None, _) => return Err(de::Error::missing_field(intern::name(field))),
            };
            values.push((field.clone(), value));
        }
        Ok(Value::Struct {
            name: self.name.into(),
            fields: values,
        })
    }
}

/// A struct's field key: its index, or `None` for a field the type doesn't
/// have, which a derive skips.
struct FieldSeed<'t>(&'t [(String, Type)]);

impl<'de> DeserializeSeed<'de> for FieldSeed<'_> {
    type Value = Option<usize>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Option<usize>, D::Error> {
        d.deserialize_identifier(self)
    }
}

impl<'de> Visitor<'de> for FieldSeed<'_> {
    type Value = Option<usize>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("field identifier")
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Option<usize>, E> {
        Ok(usize::try_from(v).ok().filter(|&i| i < self.0.len()))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Option<usize>, E> {
        Ok(self.0.iter().position(|(f, _)| f == v))
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Option<usize>, E> {
        Ok(self.0.iter().position(|(f, _)| f.as_bytes() == v))
    }
}

struct EnumVisitor<'t> {
    name: &'t str,
    variants: &'t [String],
}

impl<'de> Visitor<'de> for EnumVisitor<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "enum {}", self.name)
    }

    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Value, A::Error> {
        let (index, variant) = data.variant_seed(VariantSeed(self.variants))?;
        variant.unit_variant()?;
        Ok(Value::UnitVariant {
            enum_name: self.name.into(),
            index: index as u32,
            variant: self.variants[index].clone(),
        })
    }
}

/// An enum's variant: an index below the count or a declared name, and
/// anything else refused, as a derive's variant identifier does.
struct VariantSeed<'t>(&'t [String]);

impl VariantSeed<'_> {
    fn names(&self) -> &'static [&'static str] {
        intern::list(self.0.iter().map(String::as_str))
    }
}

impl<'de> DeserializeSeed<'de> for VariantSeed<'_> {
    type Value = usize;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<usize, D::Error> {
        d.deserialize_identifier(self)
    }
}

impl<'de> Visitor<'de> for VariantSeed<'_> {
    type Value = usize;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("variant identifier")
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<usize, E> {
        match usize::try_from(v) {
            Ok(i) if i < self.0.len() => Ok(i),
            _ => Err(E::invalid_value(
                Unexpected::Unsigned(v),
                &format!("variant index 0 <= i < {}", self.0.len()).as_str(),
            )),
        }
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<usize, E> {
        self.0
            .iter()
            .position(|name| name == v)
            .ok_or_else(|| E::unknown_variant(v, self.names()))
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<usize, E> {
        self.0
            .iter()
            .position(|name| name.as_bytes() == v)
            .ok_or_else(|| E::unknown_variant(&String::from_utf8_lossy(v), self.names()))
    }
}

/// Whether `value` is a value of `ty`'s shape: what a generated type could
/// hold, bounds aside ([`within_bounds`] checks those).
pub fn check(value: &Value, ty: &Type) -> Result<(), String> {
    check_in(value, ty, None)
}

fn check_in(value: &Value, ty: &Type, rec: Option<&Type>) -> Result<(), String> {
    let check = |value: &Value, ty: &Type| check_in(value, ty, rec);
    let mismatch = || Err(format!("{value:?} isn't a {ty:?}"));
    match (ty, value) {
        (Type::Recursive { of, .. }, _) => check_in(value, of, Some(ty)),
        (Type::SelfRef, _) => match rec {
            Some(rec) => check_in(value, rec, None),
            None => Err("`self` outside a recursive type".into()),
        },
        (Type::BoundedString { .. }, Value::String(_)) | (Type::Char, Value::Char(_)) => Ok(()),
        (Type::FixedString { len }, Value::String(s))
            if s.chars().count() as u64 == u64::from(*len) =>
        {
            Ok(())
        }
        (Type::BoundedArray { of, .. }, Value::Array(items)) => {
            items.iter().try_for_each(|item| check(item, of))
        }
        (Type::Bool, Value::Bool(_))
        | (Type::U8, Value::U8(_))
        | (Type::U16, Value::U16(_))
        | (Type::U32, Value::U32(_))
        | (Type::U64, Value::U64(_))
        | (Type::U128, Value::U128(_))
        | (Type::I8, Value::I8(_))
        | (Type::I16, Value::I16(_))
        | (Type::I32, Value::I32(_))
        | (Type::I64, Value::I64(_))
        | (Type::I128, Value::I128(_))
        | (Type::F32, Value::F32(_))
        | (Type::F64, Value::F64(_))
        | (Type::String, Value::String(_))
        | (Type::Unit, Value::Unit(())) => Ok(()),
        (Type::Option(_), Value::Option(None)) => Ok(()),
        (Type::Option(inner), Value::Option(Some(v))) => check(v, inner),
        (Type::Array(element), Value::Array(items)) => {
            items.iter().try_for_each(|item| check(item, element))
        }
        (Type::Fixed { of, len }, Value::Tuple(items)) if items.len() == *len => {
            items.iter().try_for_each(|item| check(item, of))
        }
        (
            Type::Struct { name, fields },
            Value::Struct {
                name: vname,
                fields: vfields,
            },
        ) if name == vname
            && fields.len() == vfields.len()
            && fields.iter().zip(vfields).all(|((f, _), (vf, _))| f == vf) =>
        {
            fields
                .iter()
                .zip(vfields)
                .try_for_each(|((_, t), (_, v))| check(v, t))
        }
        (
            Type::Enum { name, variants },
            Value::UnitVariant {
                enum_name,
                index,
                variant,
            },
        ) if name == enum_name && variants.get(*index as usize).is_some_and(|v| v == variant) => {
            Ok(())
        }
        _ => mismatch(),
    }
}

/// Whether `value`, a value of `ty`'s shape ([`check`]), is within its
/// bounds: the check an encoder makes before writing anything, through
/// `comline-runtime`'s [`bounds`].
pub fn within_bounds(value: &Value, ty: &Type) -> Result<(), BoundExceeded> {
    fn walk<'t>(
        value: &Value,
        ty: &'t Type,
        rec: Option<(&'t Type, &Cell<Depth>)>,
    ) -> Result<(), BoundExceeded> {
        let level = |value: &Value, of: &'t Type, depth: &Cell<Depth>| {
            let mut d = depth.get();
            d.enter()?;
            depth.set(d);
            let result = walk(value, of, Some((of, depth)));
            let mut d = depth.get();
            d.leave();
            depth.set(d);
            result
        };
        match (ty, value) {
            (Type::Recursive { max_depth, of }, _) => {
                level(value, of, &Cell::new(Depth::new(*max_depth)))
            }
            (Type::SelfRef, _) => match rec {
                Some((of, depth)) => level(value, of, depth),
                None => Ok(()),
            },
            (Type::BoundedString { max }, Value::String(s)) => bounds::check_chars(s, *max),
            (Type::FixedString { len }, Value::String(s)) => bounds::check_chars(s, *len),
            (Type::BoundedArray { of, max }, Value::Array(items)) => {
                bounds::check_items(items.len(), *max)?;
                items.iter().try_for_each(|item| walk(item, of, rec))
            }
            (Type::Option(inner), Value::Option(Some(v))) => walk(v, inner, rec),
            (Type::Array(of), Value::Array(items))
            | (Type::Fixed { of, .. }, Value::Tuple(items)) => {
                items.iter().try_for_each(|item| walk(item, of, rec))
            }
            (Type::Struct { fields, .. }, Value::Struct { fields: values, .. }) => fields
                .iter()
                .zip(values)
                .try_for_each(|((_, t), (_, v))| walk(v, t, rec)),
            _ => Ok(()),
        }
    }
    walk(value, ty, None)
}
