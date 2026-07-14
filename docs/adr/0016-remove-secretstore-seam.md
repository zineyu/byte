---
Status: accepted, supersedes ADR-0007
---

# Remove SecretStore seam from MVP

MVP will keep the provider API key inside `ModelProviderConfig` and pass it directly to `OpenAiCompatibleProvider`. We are not introducing a `SecretStore` trait or adapter: the additional seam is overkill for a single secret in the current codebase and can be added later when OS keychain support is actually needed. This is a deliberate trade-off of future-proofing for implementation simplicity.
