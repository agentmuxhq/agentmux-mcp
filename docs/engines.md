# Engines

AgentMux works without any AI model of its own. Engines are optional extras. Each is **off by default**; once you turn one on it is used fully, not half-way. Configure them in **Settings → Engines** or with `agentmux engine`.

| Role | What it does | Choices |
|---|---|---|
| **Writer** | Writes contextual summaries and product briefs; answers questions from memory | Off (outline) · Local (Ollama) · Cloud key (OpenAI-compatible: OpenRouter, Gemini's OpenAI endpoint, LM Studio, MLX servers…) · One of your agents (run headless) |
| **Search by meaning** | Embeddings for meaning-based search and routing | Off (words) · Local (Ollama) · Cloud key |
| **Decisions** | Picks a thread when a topic is ambiguous, with a confidence | Built in · Writer engine · Jeff |
| **Guard** | Screens cloud bots' questions before local agents read them; blocks likely prompt injections | Off · Writer engine · Jeff |

**Privacy:**
- An engine on `127.0.0.1` keeps everything on your Mac.
- A cloud engine is marked **sends data off this Mac** in Settings.
- Plain `http` is only accepted for engines on this Mac.
- The cloud key lives in the macOS Keychain, never in the vault.

**What fits your Mac:** Settings → Engines shows your RAM and a recommendation. With 16 GB or more, a 3–4B local writer, `nomic-embed-text` and Jeff all fit.

## Local writer and search (Ollama)

```sh
brew install ollama && ollama serve
ollama pull qwen2.5:3b
ollama pull nomic-embed-text
agentmux engine set writer ollama http://127.0.0.1:11434 qwen2.5:3b
agentmux engine set embed ollama http://127.0.0.1:11434 nomic-embed-text
agentmux engine test writer
```

## Cloud key (OpenRouter, Gemini)

Save the key under **Settings → Engines → Cloud key** (Keychain). Then:

```sh
agentmux engine set writer openai https://openrouter.ai/api/v1 google/gemini-2.5-flash
```

Gemini's own endpoint works too: `https://generativelanguage.googleapis.com/v1beta/openai`.

## Jeff (local decisions and guard)

[Jeff](https://github.com/firelex/jeff) is an open (MIT) 0.8B "System 1" model. It picks between options with calibrated probabilities, and runs on your Mac in about 2 GB. AgentMux uses:

- its **`ground`** adapter for routing decisions;
- its **`guard`** adapter for prompt-injection screening.

```sh
git clone https://github.com/firelex/jeff && cd jeff
uv sync --no-default-groups --extra lora --extra mac
uv run --no-default-groups hf download mstrasser/jeff-base --revision v1.3 --local-dir jeff-base-v1.3
for name in ground guard; do uv run --no-default-groups hf download mstrasser/jeff-adapter-$name --revision v1.3 --local-dir adapters/$name; done
JEFF_BACKEND=mlx JEFF_CHECKPOINT=jeff-base-v1.3 JEFF_ADAPTERS=adapters JEFF_QUEUE_MS=500 PORT=8765 \
  uv run --no-default-groups --extra lora --extra mac jeff-serve
```

```sh
agentmux engine set decide jeff http://127.0.0.1:8765
agentmux engine set guard jeff http://127.0.0.1:8765 guard
agentmux engine test decide
```

AgentMux calls Jeff's `POST /v1/systemone`. It uses a `choice` question for routing (the candidate threads plus "new") and a yes/no question for the guard. A decision under 0.7 confidence turns into "ask the user". A guard score at or above `engine.guard.threshold` (0.8) blocks the question, and the block is posted in the **Cloud bots** room.

Whether Jeff helps *you* shows in **Settings → Engines → Routing**: how often AgentMux had to ask, and how often its own best guess was already right.
