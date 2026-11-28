# Event fixtures

- **Owner:** Control-plane maintainers.
- **Purpose:** Validate event envelopes, lifecycle state coupling, and redaction rules.
- **Commit:** Authored synthetic event payloads only; never commit credentials, cookies, capability URLs, or full configuration.
- **Generate/fetch:** Author events from `docs/contracts/events.schema.json`, then validate them with the schema and event replay harness.

The baseline event is a redacted `runtime.running` envelope with no proxy material.
