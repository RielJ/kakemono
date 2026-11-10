//! Serde helpers shared by the provider response types.

use serde::{Deserialize, Deserializer};

/// Decodes `null` as `T::default()`, like Go's `encoding/json` decodes `null` into a zero
/// value.
///
/// Use it with `#[serde(default, deserialize_with = "crate::de::null_default")]` (or a
/// container-level `#[serde(default)]`) so a missing field is empty too. `#[serde(default)]`
/// alone covers only missing fields: a `null` would fail the whole response.
pub(crate) fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    #[derive(Debug, Default, PartialEq, Deserialize)]
    #[serde(default)]
    struct Row {
        #[serde(deserialize_with = "super::null_default")]
        name: String,
        #[serde(deserialize_with = "super::null_default")]
        tags: Vec<String>,
    }

    #[test]
    fn null_and_missing_are_empty() {
        let null: Row = serde_json::from_str(r#"{"name": null, "tags": null}"#).unwrap();
        let missing: Row = serde_json::from_str("{}").unwrap();
        assert_eq!(null, Row::default());
        assert_eq!(missing, Row::default());
        let set: Row = serde_json::from_str(r#"{"name": "a", "tags": ["b"]}"#).unwrap();
        assert_eq!(set.name, "a");
        assert_eq!(set.tags, ["b"]);
    }
}
