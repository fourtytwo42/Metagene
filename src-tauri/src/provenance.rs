use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const RESULT_PROVENANCE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResultProvenance {
    pub schema_version: u32,
    pub engine_revision: String,
    pub card_data_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario_schema_version: Option<u32>,
    pub configuration_hash: String,
}

impl ResultProvenance {
    pub fn new(
        engine_revision: impl Into<String>,
        card_data_revision: impl Into<String>,
        scenario_schema_version: Option<u32>,
        configuration_hash: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: RESULT_PROVENANCE_SCHEMA_VERSION,
            engine_revision: engine_revision.into(),
            card_data_revision: card_data_revision.into(),
            scenario_schema_version,
            configuration_hash: configuration_hash.into(),
        }
    }

    pub fn legacy(engine_revision: impl Into<String>) -> Self {
        Self {
            schema_version: 0,
            engine_revision: engine_revision.into(),
            card_data_revision: "legacy-unknown".into(),
            scenario_schema_version: None,
            configuration_hash: "legacy-unknown".into(),
        }
    }
}

pub fn configuration_hash<T: Serialize>(
    domain: &str,
    value: &T,
) -> Result<String, serde_json::Error> {
    let mut value = serde_json::to_value(value)?;
    canonicalize_json(&mut value);
    let mut hasher = Sha256::new();
    hasher.update(b"metagene-result-configuration-v1\0");
    hasher.update(domain.as_bytes());
    hasher.update(b"\0");
    serde_json::to_writer(HashWriter(&mut hasher), &value)?;
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn canonicalize_json(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(canonicalize_json),
        Value::Object(values) => {
            let mut sorted = BTreeMap::new();
            for (key, mut value) in std::mem::take(values) {
                canonicalize_json(&mut value);
                sorted.insert(key, value);
            }
            values.extend(sorted);
        }
        _ => {}
    }
}

struct HashWriter<'a>(&'a mut Sha256);

impl std::io::Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_hash_canonicalizes_object_keys_but_preserves_arrays() {
        let left = serde_json::json!({"b":2,"a":{"y":1,"x":0},"cards":["A","B"]});
        let same = serde_json::json!({"cards":["A","B"],"a":{"x":0,"y":1},"b":2});
        let reordered = serde_json::json!({"b":2,"a":{"y":1,"x":0},"cards":["B","A"]});
        assert_eq!(
            configuration_hash("test", &left).unwrap(),
            configuration_hash("test", &same).unwrap()
        );
        assert_ne!(
            configuration_hash("test", &left).unwrap(),
            configuration_hash("test", &reordered).unwrap()
        );
    }
}
