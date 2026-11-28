//! Stable error mapping for stored fingerprint JSON loading.

use serde_json::Value;

use crate::error::{CoreError, ErrorCode};

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
/// Missing required properties and malformed values map to
/// `FINGERPRINT_INVALID`; the message names the offending field. Secret values
/// are not included in diagnostics.
pub fn parse_spec_json(text: &str) -> Result<FingerprintSpec, CoreError> {
    let value: Value = serde_json::from_str(text)
        .map_err(|_| CoreError::validation(ErrorCode::FingerprintInvalid, "invalid JSON"))?;
    let object = value.as_object().ok_or_else(|| {
        CoreError::validation(ErrorCode::FingerprintInvalid, "spec must be a JSON object")
    })?;
    for field in REQUIRED_FIELDS {
        if !object.contains_key(*field) {
            return Err(CoreError::validation(
                ErrorCode::FingerprintInvalid,
                format!("required field is missing: {field}"),
            ));
        }
    }
    for field in object.keys() {
        if !ALLOWED_FIELDS.contains(&field.as_str()) {
            return Err(CoreError::validation(
                ErrorCode::FingerprintInvalid,
                format!("unknown field `{field}`"),
            ));
        }
    }
    serde_json::from_value(value).map_err(|_| {
        CoreError::validation(
            ErrorCode::FingerprintInvalid,
            "field type or nested value is invalid",
        )
    })
}
