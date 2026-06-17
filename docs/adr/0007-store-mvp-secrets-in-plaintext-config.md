# Store MVP secrets in plaintext config

Byte Agent's MVP will store OpenAI-compatible provider configuration, including API keys, in a local plaintext configuration file. This is not a secure product default; it is a deliberate MVP shortcut to reduce cross-platform keychain work, while keeping a SecretStore boundary so the plaintext implementation can later be replaced by OS keychain storage.
