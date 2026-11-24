# API fixtures

- **Owner:** Control-plane maintainers.
- **Purpose:** Exercise request/response examples against the OpenAPI contract.
- **Commit:** Authored synthetic requests and responses only; never commit session cookies, bearer tokens, CSRF secrets, or customer data.
- **Generate/fetch:** Derive samples from `docs/contracts/openapi.yaml`; validate them in the API contract test harness with credentials omitted or redacted.

The committed sample is a harmless API-version request/response pair.
