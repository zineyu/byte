# Model Provider Configuration

Byte Agent reads provider settings from a local TOML file. The daemon validates this configuration lazily on the first `send_message` call.

## Config location

Resolved in order:

1. `BYTE_CONFIG_PATH` environment variable
2. `$XDG_CONFIG_HOME/byte/config.toml`
3. `~/.config/byte/config.toml`

## Supported formats

### Flat format

```toml
provider = "openai"
base_url = "https://api.openai.com/v1"
model = "gpt-4o"
api_key = "PLACEHOLDER"
```

### Section format

```toml
provider = "openai"

[openai]
base_url = "https://api.openai.com/v1"
model = "gpt-4o"
api_key = "PLACEHOLDER"
```

Flat fields take precedence over section fields when both are present.

## Provider values

- `openai`: use the OpenAI-compatible chat-completions provider
- `echo`: test provider that echoes the developer message back as assistant deltas

## Optional fields

- `context_budget`: token budget for the active conversation path. When the estimated active path reaches 90% of this budget, the daemon automatically compacts the oldest messages into a visible Compaction Entry (see `docs/adr/0006-store-compaction-as-visible-session-entries.md`). Defaults to 8192 when omitted.

## Security note

API keys are stored in plaintext in this file during the MVP. See `docs/adr/0016-remove-secretstore-seam.md`.
