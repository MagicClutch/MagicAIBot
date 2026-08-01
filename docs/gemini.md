# AI provider setup (Groq)

Groq is the AI provider for `magic_ai_bot`. It uses the OpenAI-compatible chat
completions API at `https://api.groq.com/openai/v1/chat/completions`.

## Environment variables

```powershell
$env:GROQ_API_KEY="your-groq-api-key"
```

Optional environment variables:

- `GROQ_MODEL` — model name (default: `deepseek-v4-flash-free`)
- `GROQ_BASE_URL` — base URL (default: `https://api.groq.com/openai/v1`)
- `RUST_LOG` — log level (default: info)

## TOML configuration

```toml
[groq]
enabled = true
model = "deepseek-v4-flash-free"
# base_url = "https://api.groq.com/openai/v1"
api_key_env = "GROQ_API_KEY"
request_timeout_seconds = 60
max_request_retries = 3
temperature = 0.1
```

## How it works

1. Player sends a request via Minecraft chat (`!<request>`) or console (`/ai <request>`).
2. The application creates an AI session and sends the request to Groq with tool definitions
   generated from the command registry.
3. Groq selects a tool call (command) or returns a text response.
4. The application validates the tool call against the registry, permissions, and limits.
5. The command is executed and the result is returned.
6. The session continues until the objective is complete or the task is cancelled.

## Migration from Gemini

Runtime AI requests now use Groq. Gemini is no longer used at runtime.
The `[gemini]` config section is preserved for backward compatibility but
`[groq]` is the active provider.

## Troubleshooting

| Error | Likely cause |
|---|---|
| `Groq API key is missing or unavailable` | `GROQ_API_KEY` env var not set |
| `Groq request failed with HTTP 401` | Invalid API key |
| `Groq request failed with HTTP 429` | Rate limited — reduce request frequency |
| `Groq request failed with HTTP 400` | Invalid request format |
| `Request timed out` | Network issue or Groq service unavailable |
