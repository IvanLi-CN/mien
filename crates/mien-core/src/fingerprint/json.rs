//! Stable error mapping for stored fingerprint JSON loading.

use serde_json::Value;

use crate::error::CoreError;

use super::spec::FingerprintSpec;

const REQUIRED_FIELDS: &[&str] = &[
    "schema_version",
    "seed",
    "platform",
    "platform_version",
    "brand",
    "brand_version",
    "hardware_concurrency",
    "languages",
    "timezone",
    "window",
    "webrtc",
    "network",
];
const ALLOWED_FIELDS: &[&str] = &[
    "schema_version",
    "seed",
    "platform",
    "platform_version",
    "brand",
    "brand_version",
    "hardware_concurrency",
    "languages",
    "timezone",
    "window",
    "spoof",
    "webrtc",
    "network",
];

/// Parse a stored/exported fingerprint JSON object without silently dropping fields.
///
/// Missing required properties map to `FINGERPRINT_REQUIRED_FIELD`; malformed
/// values and unknown properties map to `FINGERPRINT_INVALID`. Secret values
/// are not included in diagnostics.
pub fn parse_spec_json(text: &str) -> Result<FingerprintSpec, CoreError> {
    let value: Value = serde_json::from_str(text)
        .map_err(|_| CoreError::Validation("FINGERPRINT_INVALID: invalid JSON".to_owned()))?;
    let object = value.as_object().ok_or_else(|| {
        CoreError::Validation("FINGERPRINT_INVALID: spec must be a JSON object".to_owned())
    })?;
    for field in REQUIRED_FIELDS {
        if !object.contains_key(*field) {
            return Err(CoreError::Validation(format!(
                "FINGERPRINT_REQUIRED_FIELD: {field}"
            )));
        }
    }
    for field in object.keys() {
        if !ALLOWED_FIELDS.contains(&field.as_str()) {
            return Err(CoreError::Validation(format!(
                "FINGERPRINT_INVALID: unknown field `{field}`"
            )));
        }
    }
    serde_json::from_value(value).map_err(|_| {
        CoreError::Validation(
            "FINGERPRINT_INVALID: field type or nested value is invalid".to_owned(),
        )
    })
}
