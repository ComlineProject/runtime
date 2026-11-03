//! A struct in the shape its format writes it, and only that (runtime#74).
//!
//! `json@1` sends a struct's fields by name (an object) and `msgpack@1` by
//! position (an array): each format's profile says so, and version
//! comparison relies on it. serde's derived `Deserialize` accepts either
//! shape for a struct, so a JSON array was read by position and a
//! MessagePack map by name. A reader whose schema reordered two fields of
//! the same type then got them swapped, with no error. That is exactly what
//! sending by name exists to prevent.
//!
//! [`Shaped`] wraps a format's deserializer and goes with every value it
//! reads, however deep. A struct written in the other shape is refused, and
//! so are bytes where a string is expected (MessagePack's `bin`). Nothing
//! else changes, and nothing is allocated.

use core::fmt;

use serde::de::{
    self, DeserializeSeed, Deserializer, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor,
};

/// The one shape a format writes a struct in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StructShape {
    /// By name: `json@1`.
    Map,
    /// By position: `msgpack@1`.
    Seq,
}

/// A deserializer that reads structs only in `shape`.
pub(crate) struct Shaped<D> {
    pub de: D,
    pub shape: StructShape,
}

/// A visitor, carrying the shape to whatever it reads next.
struct ShapedVisitor<V> {
    visitor: V,
    shape: StructShape,
}

/// The visitor of a struct: one shape only.
struct StructVisitor<V> {
    visitor: V,
    shape: StructShape,
}

/// The visitor of a string: never bytes.
struct StrVisitor<V> {
    visitor: V,
}

struct ShapedSeed<S> {
    seed: S,
    shape: StructShape,
}

struct ShapedAccess<A> {
    access: A,
    shape: StructShape,
}

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for ShapedSeed<S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, de: D) -> Result<S::Value, D::Error> {
        self.seed.deserialize(Shaped {
            de,
            shape: self.shape,
        })
    }
}

macro_rules! forward {
    ($($method:ident),* $(,)?) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, D::Error> {
                self.de.$method(ShapedVisitor { visitor, shape: self.shape })
            }
        )*
    };
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for Shaped<D> {
    type Error = D::Error;

    forward!(
        deserialize_any,
        deserialize_bool,
        deserialize_i8,
        deserialize_i16,
        deserialize_i32,
        deserialize_i64,
        deserialize_i128,
        deserialize_u8,
        deserialize_u16,
        deserialize_u32,
        deserialize_u64,
        deserialize_u128,
        deserialize_f32,
        deserialize_f64,
        deserialize_char,
        deserialize_bytes,
        deserialize_byte_buf,
        deserialize_option,
        deserialize_unit,
        deserialize_seq,
        deserialize_map,
        deserialize_identifier,
        deserialize_ignored_any,
    );

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, D::Error> {
        self.de.deserialize_str(StrVisitor { visitor })
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, D::Error> {
        self.de.deserialize_string(StrVisitor { visitor })
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.de.deserialize_unit_struct(
            name,
            ShapedVisitor {
                visitor,
                shape: self.shape,
            },
        )
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.de.deserialize_newtype_struct(
            name,
            ShapedVisitor {
                visitor,
                shape: self.shape,
            },
        )
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.de.deserialize_tuple(
            len,
            ShapedVisitor {
                visitor,
                shape: self.shape,
            },
        )
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.de.deserialize_tuple_struct(
            name,
            len,
            ShapedVisitor {
                visitor,
                shape: self.shape,
            },
        )
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.de.deserialize_struct(
            name,
            fields,
            StructVisitor {
                visitor,
                shape: self.shape,
            },
        )
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.de.deserialize_enum(
            name,
            variants,
            ShapedVisitor {
                visitor,
                shape: self.shape,
            },
        )
    }

    fn is_human_readable(&self) -> bool {
        self.de.is_human_readable()
    }
}

/// The `visit_*` methods that hand over a plain value, unchanged.
macro_rules! pass {
    ($($method:ident: $ty:ty),* $(,)?) => {
        $(
            fn $method<E: de::Error>(self, v: $ty) -> Result<Self::Value, E> {
                self.visitor.$method(v)
            }
        )*
    };
}

/// The `visit_*` methods that hand over more to read, each carrying the
/// shape on.
macro_rules! nested {
    () => {
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            self.visitor.visit_none()
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            self.visitor.visit_unit()
        }

        fn visit_some<D2: Deserializer<'de>>(self, de: D2) -> Result<Self::Value, D2::Error> {
            self.visitor.visit_some(Shaped {
                de,
                shape: self.shape,
            })
        }

        fn visit_newtype_struct<D2: Deserializer<'de>>(
            self,
            de: D2,
        ) -> Result<Self::Value, D2::Error> {
            self.visitor.visit_newtype_struct(Shaped {
                de,
                shape: self.shape,
            })
        }

        fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
            self.visitor.visit_enum(ShapedAccess {
                access: data,
                shape: self.shape,
            })
        }
    };
}

impl<'de, V: Visitor<'de>> Visitor<'de> for ShapedVisitor<V> {
    type Value = V::Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.visitor.expecting(f)
    }

    pass!(
        visit_bool: bool,
        visit_i8: i8,
        visit_i16: i16,
        visit_i32: i32,
        visit_i64: i64,
        visit_i128: i128,
        visit_u8: u8,
        visit_u16: u16,
        visit_u32: u32,
        visit_u64: u64,
        visit_u128: u128,
        visit_f32: f32,
        visit_f64: f64,
        visit_char: char,
        visit_str: &str,
        visit_borrowed_str: &'de str,
        visit_bytes: &[u8],
        visit_borrowed_bytes: &'de [u8],
    );

    #[cfg(feature = "std")]
    fn visit_string<E: de::Error>(self, v: std::string::String) -> Result<Self::Value, E> {
        self.visitor.visit_string(v)
    }

    #[cfg(feature = "std")]
    fn visit_byte_buf<E: de::Error>(self, v: std::vec::Vec<u8>) -> Result<Self::Value, E> {
        self.visitor.visit_byte_buf(v)
    }

    nested!();

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        self.visitor.visit_seq(ShapedAccess {
            access: seq,
            shape: self.shape,
        })
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        self.visitor.visit_map(ShapedAccess {
            access: map,
            shape: self.shape,
        })
    }
}

impl<'de, V: Visitor<'de>> Visitor<'de> for StructVisitor<V> {
    type Value = V::Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.visitor.expecting(f)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        match self.shape {
            StructShape::Seq => self.visitor.visit_seq(ShapedAccess {
                access: seq,
                shape: self.shape,
            }),
            StructShape::Map => Err(de::Error::invalid_type(
                de::Unexpected::Seq,
                &"a struct by name",
            )),
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        match self.shape {
            StructShape::Map => self.visitor.visit_map(ShapedAccess {
                access: map,
                shape: self.shape,
            }),
            StructShape::Seq => Err(de::Error::invalid_type(
                de::Unexpected::Map,
                &"a struct by position",
            )),
        }
    }
}

impl<'de, V: Visitor<'de>> Visitor<'de> for StrVisitor<V> {
    type Value = V::Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.visitor.expecting(f)
    }

    pass!(visit_str: &str, visit_borrowed_str: &'de str, visit_char: char);

    #[cfg(feature = "std")]
    fn visit_string<E: de::Error>(self, v: std::string::String) -> Result<Self::Value, E> {
        self.visitor.visit_string(v)
    }

    // `visit_bytes` is left to the default, which refuses it: a string is
    // never bytes on the wire, though serde's own string visitors take UTF-8
    // bytes for one.
}

impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for ShapedAccess<A> {
    type Error = A::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, A::Error> {
        self.access.next_element_seed(ShapedSeed {
            seed,
            shape: self.shape,
        })
    }

    fn size_hint(&self) -> Option<usize> {
        self.access.size_hint()
    }
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for ShapedAccess<A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, A::Error> {
        self.access.next_key_seed(ShapedSeed {
            seed,
            shape: self.shape,
        })
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, A::Error> {
        self.access.next_value_seed(ShapedSeed {
            seed,
            shape: self.shape,
        })
    }

    fn size_hint(&self) -> Option<usize> {
        self.access.size_hint()
    }
}

impl<'de, A: EnumAccess<'de>> EnumAccess<'de> for ShapedAccess<A> {
    type Error = A::Error;
    type Variant = ShapedAccess<A::Variant>;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), A::Error> {
        let shape = self.shape;
        let (value, variant) = self.access.variant_seed(ShapedSeed { seed, shape })?;
        Ok((
            value,
            ShapedAccess {
                access: variant,
                shape,
            },
        ))
    }
}

impl<'de, A: VariantAccess<'de>> VariantAccess<'de> for ShapedAccess<A> {
    type Error = A::Error;

    fn unit_variant(self) -> Result<(), A::Error> {
        self.access.unit_variant()
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, A::Error> {
        self.access.newtype_variant_seed(ShapedSeed {
            seed,
            shape: self.shape,
        })
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, A::Error> {
        self.access.tuple_variant(
            len,
            ShapedVisitor {
                visitor,
                shape: self.shape,
            },
        )
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, A::Error> {
        self.access.struct_variant(
            fields,
            StructVisitor {
                visitor,
                shape: self.shape,
            },
        )
    }
}
