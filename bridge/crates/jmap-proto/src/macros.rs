// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Internal macros for the many "closed set of JSON strings" enums.

/// Declares a fieldless enum whose values are fixed JSON strings, with
/// `as_str`, `ALL`, `FromStr` (unknown strings are an [`UnknownValue`]
/// error), `Display`, `Serialize` and `Deserialize`.
///
/// [`UnknownValue`]: crate::types::UnknownValue
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident ($kind:literal) {
            $( $(#[$vmeta:meta])* $variant:ident = $text:literal, )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[non_exhaustive]
        $vis enum $name {
            $( $(#[$vmeta])* $variant, )+
        }

        impl $name {
            /// Every value, in declaration order.
            pub const ALL: &'static [Self] = &[ $( Self::$variant, )+ ];

            /// The JSON string of this value.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $text, )+
                }
            }
        }

        impl ::core::str::FromStr for $name {
            type Err = $crate::types::UnknownValue;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $( $text => Ok(Self::$variant), )+
                    _ => Err($crate::types::UnknownValue::new($kind, s)),
                }
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = <::std::borrow::Cow<'de, str> as ::serde::Deserialize>::deserialize(deserializer)?;
                text.parse().map_err(::serde::de::Error::custom)
            }
        }
    };
}

/// Implements `Serialize` (as the `Display` string) and `Deserialize` (with
/// `FromStr`) for a type.
macro_rules! serde_via_str {
    ($name:ty) => {
        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Self, D::Error> {
                let text = <::std::borrow::Cow<'de, str> as ::serde::Deserialize>::deserialize(
                    deserializer,
                )?;
                text.parse().map_err(::serde::de::Error::custom)
            }
        }
    };
}

pub(crate) use serde_via_str;
pub(crate) use string_enum;
