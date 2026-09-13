//! Shared deserialization helper: values written either as strings
//! (`"280 KB"`, `"5%"`) or as bare JSON numbers (bytes).

use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;

use serde::de::{self, Deserializer, Visitor};

/// Deserializes any type whose `FromStr` error is [`crate::WasmCheckError`]
/// from a string or a JSON number.
///
/// Numbers are rendered back to text and parsed through `FromStr`, so a single
/// code path validates both forms and the error message stays specific
/// (`invalid size \`10 GB\``) instead of a generic type mismatch.
pub(crate) fn string_or_number<'de, D, T>(
    deserializer: D,
    expected: &'static str,
) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr<Err = crate::WasmCheckError>,
{
    struct Value<T> {
        expected: &'static str,
        marker: PhantomData<T>,
    }

    impl<T> Visitor<'_> for Value<T>
    where
        T: FromStr<Err = crate::WasmCheckError>,
    {
        type Value = T;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.expected)
        }

        fn visit_u64<E: de::Error>(self, value: u64) -> Result<T, E> {
            value.to_string().parse().map_err(E::custom)
        }

        fn visit_i64<E: de::Error>(self, value: i64) -> Result<T, E> {
            u64::try_from(value)
                .map_err(E::custom)?
                .to_string()
                .parse()
                .map_err(E::custom)
        }

        fn visit_f64<E: de::Error>(self, value: f64) -> Result<T, E> {
            if !value.is_finite() || value < 0.0 {
                return Err(E::custom("value must be a finite non-negative number"));
            }
            value.to_string().parse().map_err(E::custom)
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<T, E> {
            value.parse().map_err(E::custom)
        }
    }

    deserializer.deserialize_any(Value {
        expected,
        marker: PhantomData,
    })
}
