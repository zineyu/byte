---
Status: superseded by ADR-0016
---

# Store MVP secrets in plaintext config

Byte Agent's MVP will store OpenAI-compatible provider configuration, including API keys, in a local plaintext configuration file. This is not a secure product default; it was a deliberate MVP shortcut to reduce cross-platform keychain work, while keeping a SecretStore boundary so the plaintext implementation could later be replaced by OS keychain storage. That boundary was later removed in ADR-0016 because the seam was overkill for the single secret in the MVP.
