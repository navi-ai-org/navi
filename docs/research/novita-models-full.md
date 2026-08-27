# Novita AI — Complete Model Catalog

> Exhaustive inventory of every model available on the Novita AI platform.
>
> **Primary source:** `https://api.novita.ai/v3/openai/models` — the live OpenAI-compatible
> models endpoint. Returns structured JSON for every chat model with exact IDs, context
> windows, max output tokens, capabilities (`features`, `input_modalities`, `endpoints`), and
> pricing (in integer cents-per-1M-tokens). This is the authoritative, machine-readable list.
>
> **Secondary source:** `https://novita.ai/pricing` — the public pricing page, which additionally
> covers non-chat model types (Embedding, Reranker, Image, Video, Audio, AI Search) that the
> chat-models endpoint does not include.

## Summary

| Category | Count | Source |
|---|---:|---|
| LLM / chat models | 142 | `/v3/openai/models` |
| Embedding models | 3 | pricing page |
| Reranker models | 2 | pricing page |
| Image generation / editing APIs | 16 | pricing page |
| Video generation APIs | 20+ (Kling, Wan, Vidu, PixVerse, Seedance, Minimax Hailuo, Hunyuan, SVD) | pricing page |
| Audio (TTS / music / voice) APIs | 13 | pricing page |
| AI Search providers | 2 (EXA, Tavily) | pricing page |

### Reading the tables

- **Prices** are per **1 million (1M) tokens** for LLMs; per **image** for image models; per **video** or **per second** for video; per **1M characters** or **per song/voice** for audio.
- **Cache Read** = discounted prompt-cache-read price (Novita caches repeated prefixes).
- **Status:** `Active` = live & billable (status=1); `Archived` = listed but currently inactive (status=4).
- **Capabilities** legend: `tool-calling`=function/tool calling, `JSON`=structured outputs, `reasoning`=thinking/CoT, `vision`=image input, `video`=video input, `audio-in`=audio input, `anthropic-api`=Anthropic Messages API also supported, `responses-api`=OpenAI Responses API supported.
- **Streaming:** all active chat models support streaming via the OpenAI-compatible `chat/completions` endpoint (`stream: true`).

---

## 1. LLM / Chat Models — by family

All **142** models below are returned by `GET https://api.novita.ai/v3/openai/models` and use the `chat/completions` (OpenAI-compatible) endpoint. Where noted, `anthropic` and `responses` endpoints are also available.

### DeepSeek (21)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `deepseek/deepseek-ocr` | DeepSeek-OCR | 8,192 | 8,192 | $0.0300 | $0.0300 | - | vision | Active | - |
| `deepseek/deepseek-ocr-2` | DeepSeek-OCR 2 | 8,192 | 8,192 | $0.0300 | $0.0300 | - | serverless, vision | Active | - |
| `deepseek/deepseek-prover-v2-671b` | Deepseek Prover V2 671B | 160,000 | 160,000 | $0.7000 | $2.5000 | - | serverless | Archived | - |
| `deepseek/deepseek-r1` | DeepSeek R1 | 64,000 | 16,000 | $4.0000 | $4.0000 | - | tool-calling, reasoning, serverless | Active | - |
| `deepseek/deepseek-r1-0528` | DeepSeek R1 0528 | 163,840 | 32,768 | $0.7000 | $2.5000 | $0.3500 | tool-calling, JSON, reasoning, serverless | Active | - |
| `deepseek/deepseek-r1-0528-qwen3-8b` | DeepSeek R1 0528 Qwen3 8B | 128,000 | 32,000 | $0.0600 | $0.0900 | - | - | Active | - |
| `deepseek/deepseek-r1-distill-llama-70b` | DeepSeek R1 Distill LLama 70B | 8,192 | 8,192 | $0.8000 | $0.8000 | - | reasoning, serverless | Active | - |
| `deepseek/deepseek-r1-distill-qwen-14b` | DeepSeek R1 Distill Qwen 14B | 32,768 | 16,384 | $0.1500 | $0.1500 | - | - | Archived | - |
| `deepseek/deepseek-r1-distill-qwen-32b` | DeepSeek R1 Distill Qwen 32B | 64,000 | 32,000 | $0.3000 | $0.3000 | - | - | Archived | - |
| `deepseek/deepseek-r1-turbo` | DeepSeek R1 (Turbo)  | 64,000 | 16,000 | $0.7000 | $2.5000 | - | tool-calling, JSON, reasoning, serverless | Active | - |
| `deepseek/deepseek-r1/community` | DeepSeek R1 | 64,000 | 8,000 | $4.0000 | $4.0000 | - | tool-calling, reasoning, serverless | Active | - |
| `deepseek/deepseek-v3-0324` | DeepSeek V3 0324 | 163,840 | 65,536 | $0.2700 | $1.1200 | $0.1350 | tool-calling, JSON, serverless, anthropic-api | Active | - |
| `deepseek/deepseek-v3-turbo` | DeepSeek V3 (Turbo)  | 64,000 | 16,000 | $0.4000 | $1.3000 | - | tool-calling, serverless | Active | - |
| `deepseek/deepseek-v3.1` | DeepSeek V3.1 | 131,072 | 32,768 | $0.2700 | $1.0000 | $0.1350 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `deepseek/deepseek-v3.1-terminus` | Deepseek V3.1 Terminus | 131,072 | 32,768 | $0.2700 | $1.0000 | $0.1350 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `deepseek/deepseek-v3.2` | Deepseek V3.2 | 163,840 | 65,536 | $0.2690 | $0.4000 | $0.1345 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `deepseek/deepseek-v3.2-exp` | Deepseek V3.2 Exp | 163,840 | 65,536 | $0.2700 | $0.4100 | - | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `deepseek/deepseek-v3/community` | DeepSeek V3 | 64,000 | 8,000 | $0.8900 | $0.8900 | - | tool-calling, serverless | Active | - |
| `deepseek/deepseek-v4-flash` | Deepseek V4 Flash | 1,048,576 | 393,216 | $0.1400 | $0.2800 | $0.0280 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `deepseek/deepseek-v4-pro` | Deepseek V4 Pro | 1,048,576 | 393,216 | $1.6000 | $3.2000 | $0.1350 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `deepseek/deepseek_v3` | DeepSeek V3 | 64,000 | 16,000 | $0.8900 | $0.8900 | - | tool-calling, serverless | Active | - |

### Qwen (35)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `qwen/qwen-2-7b-instruct` | Qwen 2 7B Instruct | 32,768 | 32,768 | $0.0540 | $0.0540 | - | - | Archived | - |
| `qwen/qwen-2-vl-72b-instruct` | Qwen 2 VL 72B Instruct | 32,768 | 120,000 | $0.4500 | $0.4500 | - | - | Archived | - |
| `qwen/qwen-2.5-72b-instruct` | Qwen 2.5 72B Instruct | 32,000 | 8,192 | $0.3800 | $0.4000 | - | tool-calling, JSON, serverless | Active | - |
| `qwen/qwen-mt-plus` | Qwen MT Plus | 16,384 | 8,192 | $0.2500 | $0.7500 | - | serverless | Active | - |
| `qwen/qwen2.5-7b-instruct` | Qwen2.5 7B Instruct | 32,000 | 8,192 | $0.0700 | $0.0700 | - | tool-calling, JSON, serverless | Archived | - |
| `qwen/qwen2.5-vl-72b-instruct` | Qwen2.5 VL 72B Instruct | 32,768 | 32,768 | $0.8000 | $0.8000 | - | serverless, vision, video | Archived | - |
| `qwen/qwen3-235b-a22b-fp8` | Qwen3 235B A22B | 40,960 | 20,000 | $0.2000 | $0.8000 | - | JSON, reasoning, serverless | Active | - |
| `qwen/qwen3-235b-a22b-instruct-2507` | Qwen3 235B A22B Instruct 2507 | 131,072 | 16,384 | $0.0900 | $0.5800 | - | tool-calling, JSON, serverless | Active | - |
| `qwen/qwen3-235b-a22b-thinking-2507` | Qwen3 235B A22b Thinking 2507 | 131,072 | 32,768 | $0.3000 | $3.0000 | - | tool-calling, reasoning, serverless, anthropic-api | Active | - |
| `qwen/qwen3-30b-a3b-fp8` | Qwen3 30B A3B | 40,960 | 20,000 | $0.0900 | $0.4500 | - | tool-calling, reasoning | Archived | - |
| `qwen/qwen3-32b-fp8` | Qwen3 32B | 40,960 | 20,000 | $0.1000 | $0.4500 | - | reasoning | Archived | - |
| `qwen/qwen3-4b-fp8` | Qwen3 4B | 128,000 | 8,192 | $0.0300 | $0.0300 | - | tool-calling, reasoning, serverless | Archived | - |
| `qwen/qwen3-8b-fp8` | Qwen3 8B | 128,000 | 20,000 | $0.0350 | $0.1380 | - | - | Archived | - |
| `qwen/qwen3-coder-30b-a3b-instruct` | Qwen3 Coder 30b A3B Instruct | 160,000 | 32,768 | $0.0700 | $0.2700 | - | tool-calling, JSON, serverless | Active | - |
| `qwen/qwen3-coder-480b-a35b-instruct` | Qwen3 Coder 480B A35B Instruct | 262,144 | 65,536 | $0.3800 | $1.5500 | - | tool-calling, JSON, serverless, anthropic-api | Active | - |
| `qwen/qwen3-coder-next` | Qwen3 Coder Next | 262,144 | 65,536 | $0.2000 | $1.5000 | - | tool-calling, JSON, serverless, anthropic-api | Active | - |
| `qwen/qwen3-max` | Qwen3 Max | 262,144 | 65,536 | $2.1100 | $8.4500 | - | tool-calling, JSON, serverless | Active | Tiered: 1-32768t:$0.8450/$3.3800; 32768-131072t:$1.4000/$5.6400; 131072-258048t:$2.1100/$8.4500 |
| `qwen/qwen3-next-80b-a3b-instruct` | Qwen3 Next 80B A3B Instruct | 131,072 | 32,768 | $0.1500 | $1.5000 | - | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `qwen/qwen3-next-80b-a3b-thinking` | Qwen3 Next 80B A3B Thinking | 131,072 | 32,768 | $0.1500 | $1.5000 | - | tool-calling, reasoning, serverless, anthropic-api | Archived | - |
| `qwen/qwen3-omni-30b-a3b-instruct` | Qwen3 Omni 30B A3B Instruct | 65,536 | 16,384 | $0.2500 | $0.9700 | - | tool-calling, JSON, serverless, vision, video, audio-in | Active | - |
| `qwen/qwen3-omni-30b-a3b-thinking` | Qwen3 Omni 30B A3B Thinking | 65,536 | 16,384 | $0.2500 | $0.9700 | - | tool-calling, JSON, reasoning, serverless, vision, video, audio-in | Active | - |
| `qwen/qwen3-vl-235b-a22b-instruct` | Qwen3 VL 235B A22B Instruct | 131,072 | 32,768 | $0.3000 | $1.5000 | - | tool-calling, JSON, serverless, vision, video | Active | - |
| `qwen/qwen3-vl-235b-a22b-thinking` | Qwen3 VL 235B A22B Thinking | 131,072 | 32,768 | $0.9800 | $3.9500 | - | tool-calling, reasoning, serverless, vision, video | Active | - |
| `qwen/qwen3-vl-30b-a3b-instruct` | qwen/qwen3-vl-30b-a3b-instruct | 131,072 | 32,768 | $0.2000 | $0.7000 | - | tool-calling, JSON, serverless, vision, video | Active | - |
| `qwen/qwen3-vl-30b-a3b-thinking` | qwen/qwen3-vl-30b-a3b-thinking | 131,072 | 32,768 | $0.2000 | $1.0000 | - | tool-calling, JSON, serverless, vision, video | Archived | - |
| `qwen/qwen3-vl-8b-instruct` | qwen/qwen3-vl-8b-instruct | 131,072 | 32,768 | $0.0800 | $0.5000 | - | tool-calling, JSON, serverless, vision, video | Archived | - |
| `qwen/qwen3.5-122b-a10b` | Qwen3.5-122B-A10B | 262,144 | 65,536 | $0.4000 | $3.2000 | - | tool-calling, JSON, reasoning, serverless, vision, video | Active | - |
| `qwen/qwen3.5-27b` | Qwen3.5-27B | 262,144 | 65,536 | $0.3000 | $2.4000 | - | tool-calling, JSON, reasoning, serverless, vision, video | Active | - |
| `qwen/qwen3.5-35b-a3b` | Qwen3.5-35B-A3B | 262,144 | 65,536 | $0.2500 | $2.0000 | - | tool-calling, JSON, reasoning, serverless, vision, video | Active | - |
| `qwen/qwen3.5-397b-a17b` | Qwen3.5-397B-A17B | 262,144 | 65,536 | $0.6000 | $3.6000 | - | tool-calling, JSON, reasoning, serverless, vision, video | Active | - |
| `qwen/qwen3.5-plus` | Qwen3.5-Plus | 1,000,000 | 65,536 | Free | Free | - | tool-calling, JSON, reasoning, serverless, vision, video | Active | Tiered: 1-256000t:$0.4000/$2.4000; 256000-1000000t:$0.5000/$3.0000 |
| `qwen/qwen3.6-27b` | Qwen3.6-27B | 262,144 | 65,536 | $0.6000 | $3.6000 | - | tool-calling, JSON, reasoning, serverless, vision, video | Active | - |
| `qwen/qwen3.6-35b-a3b` | Qwen3.6-35B-A3B | 262,144 | 65,536 | $0.2480 | $1.4850 | - | tool-calling, JSON, reasoning, serverless, vision, video | Active | - |
| `qwen/qwen3.6-plus` | Qwen3.6-Plus | 1,000,000 | 65,536 | Free | Free | - | tool-calling, JSON, reasoning, serverless, vision, video | Active | Tiered: 1-262144t:$0.5000/$3.0000; 262144-1000000t:$2.0000/$6.0000 |
| `qwen/qwen3.7-max` | Qwen3.7-Max | 1,000,000 | 65,536 | $1.2500 | $3.7500 | $0.2500 | tool-calling, JSON, reasoning, serverless | Active | - |

### Baidu (ERNIE / Wenxin) (7)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `baidu/cobuddy` | CoBuddy | 131,072 | 65,536 | $0.2800 | $1.1300 | $0.0700 | tool-calling, reasoning, serverless | Active | - |
| `baidu/ernie-4.5-21B-a3b` | ERNIE 4.5 21B A3B | 120,000 | 8,000 | $0.0700 | $0.2800 | - | tool-calling, serverless | Active | - |
| `baidu/ernie-4.5-21B-a3b-thinking` | ERNIE-4.5-21B-A3B-Thinking | 131,072 | 65,536 | $0.0700 | $0.2800 | - | reasoning, serverless | Archived | - |
| `baidu/ernie-4.5-300b-a47b-paddle` | ERNIE 4.5 300B A47B | 123,000 | 12,000 | $0.2800 | $1.1000 | - | JSON, serverless | Archived | - |
| `baidu/ernie-4.5-vl-28b-a3b` | ERNIE 4.5 VL 28B A3B | 30,000 | 8,000 | $0.1400 | $0.5600 | - | tool-calling, reasoning, serverless, vision | Archived | - |
| `baidu/ernie-4.5-vl-28b-a3b-thinking` | ERNIE-4.5-VL-28B-A3B-Thinking | 131,072 | 65,536 | $0.3900 | $0.3900 | - | tool-calling, JSON, reasoning, serverless, vision, video | Archived | - |
| `baidu/ernie-4.5-vl-424b-a47b` | ERNIE 4.5 VL 424B A47B | 123,000 | 16,000 | $0.4200 | $1.2500 | - | reasoning, serverless, vision | Active | - |

### Z.AI (GLM / Zhipu) (14)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `zai-org/autoglm-phone-9b-multilingual` | AutoGLM-Phone-9B-Multilingual | 65,536 | 65,536 | $0.0350 | $0.1380 | - | serverless, vision | Active | - |
| `zai-org/glm-4.5` | GLM-4.5 | 131,072 | 98,304 | $0.6000 | $2.2000 | $0.1100 | tool-calling, reasoning, serverless | Archived | - |
| `zai-org/glm-4.5-air` | zai-org/glm-4.5-air | 131,072 | 98,304 | $0.1300 | $0.8500 | $0.0250 | tool-calling, reasoning, serverless | Active | - |
| `zai-org/glm-4.5v` | GLM 4.5V | 65,536 | 16,384 | $0.6000 | $1.8000 | $0.1100 | tool-calling, JSON, reasoning, serverless, vision, video | Active | - |
| `zai-org/glm-4.6` | GLM 4.6 | 204,800 | 131,072 | $0.5500 | $2.2000 | $0.1100 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `zai-org/glm-4.6v` | GLM 4.6V | 131,072 | 32,768 | $0.3000 | $0.9000 | $0.0550 | tool-calling, JSON, reasoning, serverless, vision, video, anthropic-api | Active | - |
| `zai-org/glm-4.7` | GLM-4.7 | 204,800 | 131,072 | $0.6000 | $2.2000 | $0.1100 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `zai-org/glm-4.7-flash` | GLM-4.7-Flash | 200,000 | 128,000 | $0.0700 | $0.4000 | $0.0100 | tool-calling, JSON, reasoning, serverless | Active | - |
| `zai-org/glm-4.7-h` | GLM-4.7 | 204,800 | 131,072 | $0.6000 | $2.2000 | $0.1100 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `zai-org/glm-5` | GLM-5 | 202,800 | 131,072 | $1.0000 | $3.2000 | $0.2000 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `zai-org/glm-5-turbo` | GLM-5-Turbo | 202,800 | 131,072 | $1.2000 | $4.0000 | $0.2400 | tool-calling, JSON, reasoning, serverless | Active | - |
| `zai-org/glm-5.1` | GLM-5.1 | 204,800 | 131,072 | $1.3800 | $4.4000 | $0.2600 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `zai-org/glm-5.2` | GLM 5.2 | 1,048,576 | 131,072 | $1.4000 | $4.4000 | $0.2600 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `zai-org/glm-5v-turbo` | GLM-5V-Turbo | 204,800 | 131,072 | $1.2000 | $4.0000 | $0.2400 | tool-calling, JSON, reasoning, serverless, vision, video, anthropic-api | Active | - |

### Sao10K (3)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `sao10k/l3-70b-euryale-v2.1` | L3 70B Euryale V2.1  | 8,192 | 8,192 | $1.4800 | $1.4800 | - | tool-calling | Active | - |
| `sao10k/l3-8b-lunaris` | Sao10k L3 8B Lunaris  | 8,192 | 8,192 | $0.0500 | $0.0500 | - | JSON, serverless | Active | - |
| `sao10k/l31-70b-euryale-v2.2` | L31 70B Euryale V2.2 | 8,192 | 8,192 | $1.4800 | $1.4800 | - | tool-calling, JSON, reasoning, serverless | Active | - |

### Sao10K (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `Sao10K/L3-8B-Stheno-v3.2` | L3 8B Stheno V3.2 | 8,192 | 32,000 | $0.0500 | $0.0500 | - | tool-calling, serverless | Active | - |

### MindAI (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `mindai/macaron-v1-venti` | Macaron V1 Venti | 1,048,576 | 131,072 | Free | Free | - | tool-calling, reasoning, serverless, anthropic-api, responses-api | Active | - |

### InclusionAI (Ling) (4)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `inclusionai/ling-2.6-1t` | Ling-2.6-1T | 262,144 | 32,768 | $0.3000 | $2.5000 | $0.0600 | tool-calling, JSON, serverless | Active | - |
| `inclusionai/ling-2.6-flash` | Ling-2.6-flash | 262,144 | 32,768 | $0.1000 | $0.3000 | $0.0200 | tool-calling, JSON, serverless | Active | - |
| `inclusionai/ling-3.0-flash` | Ling-3.0-flash | 262,144 | 32,768 | Free | Free | - | tool-calling, reasoning, serverless | Active | - |
| `inclusionai/ring-2.6-1t` | Ring-2.6-1T | 262,144 | 65,536 | $0.3000 | $2.5000 | $0.0600 | tool-calling, JSON, reasoning, serverless | Active | - |

### Tencent (Hunyuan) (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `tencent/hy3` | Hy3 | 262,144 | 262,144 | $0.1400 | $0.5800 | $0.0350 | tool-calling, JSON, reasoning, serverless | Active | - |

### MoonshotAI (Kimi) (7)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `moonshotai/kimi-k2-0905` | Kimi K2 0905 | 262,144 | 100,352 | $0.6000 | $2.5000 | - | tool-calling, JSON, serverless, anthropic-api | Active | - |
| `moonshotai/kimi-k2-instruct` | Kimi K2 Instruct | 131,072 | 100,352 | $0.5700 | $2.3000 | - | tool-calling, serverless, anthropic-api | Active | - |
| `moonshotai/kimi-k2-thinking` | Kimi K2 Thinking | 262,144 | 100,352 | $0.6000 | $2.5000 | $0.1500 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `moonshotai/kimi-k2.5` | Kimi K2.5 | 262,144 | 262,144 | $0.6000 | $3.0000 | $0.1000 | tool-calling, JSON, reasoning, serverless, vision, video, anthropic-api | Active | - |
| `moonshotai/kimi-k2.6` | Kimi K2.6 | 262,144 | 262,144 | $0.8000 | $3.4000 | $0.1600 | tool-calling, JSON, reasoning, serverless, vision, video, anthropic-api | Active | - |
| `moonshotai/kimi-k2.7-code` | Kimi K2.7 Code | 262,144 | 262,144 | $0.9500 | $4.0000 | $0.1900 | tool-calling, JSON, reasoning, serverless, vision, video, anthropic-api | Active | - |
| `moonshotai/kimi-k3` | Kimi K3 | 1,048,576 | 1,048,576 | $3.0000 | $15.0000 | $0.3000 | tool-calling, JSON, reasoning, serverless, vision, video, anthropic-api | Active | - |

### MiniMax (8)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `minimax/m2-her` | M2-her | 32,000 | - | Free | Free | - | serverless | Active | - |
| `minimax/minimax-m2` | MiniMax-M2 | 204,800 | 131,072 | $0.3000 | $1.2000 | $0.0300 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `minimax/minimax-m2.1` | Minimax M2.1 | 204,800 | 131,072 | $0.3000 | $1.2000 | $0.0300 | tool-calling, JSON, serverless, anthropic-api | Active | - |
| `minimax/minimax-m2.5` | MiniMax M2.5 | 204,800 | 131,100 | $0.3000 | $1.2000 | $0.0300 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `minimax/minimax-m2.5-highspeed` | MiniMax M2.5-highspeed | 204,800 | 131,100 | $0.6000 | $2.4000 | $0.0300 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `minimax/minimax-m2.7` | MiniMax M2.7 | 204,800 | 131,072 | $0.3000 | $1.2000 | $0.0600 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `minimax/minimax-m2.7-highspeed` | MiniMax M2.7-highspeed | 204,800 | 131,072 | $0.6000 | $2.4000 | $0.0600 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |
| `minimax/minimax-m3` | MiniMax M3 | 1,000,000 | 131,072 | $0.3000 | $1.2000 | $0.0600 | tool-calling, JSON, reasoning, serverless, vision, video, anthropic-api | Active | Tiered: 1-524288t:$0.3000/$1.2000; 524288-1000000t:$0.6000/$2.4000 |

### MiniMax (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `minimaxai/minimax-m1-80k` | MiniMax M1 | 1,000,000 | 40,000 | $0.5500 | $2.2000 | - | tool-calling, JSON, reasoning, serverless | Active | - |

### StepFun (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `stepfun/step-3.7-flash` | Step 3.7 Flash | 262,144 | 256,000 | $0.2000 | $1.1500 | $0.0400 | tool-calling, JSON, reasoning, serverless, vision, video | Active | - |

### NVIDIA (Nemotron) (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `nvidia/nemotron-3-nano-30b-a3b` | Nemotron 3 Nano 30B A3B | 262,144 | 32,768 | $0.0500 | $0.2000 | - | tool-calling, JSON, reasoning, serverless | Active | - |

### Google (Gemma) (4)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `google/gemma-3-12b-it` | Gemma3 12B | 131,072 | 8,192 | $0.0500 | $0.1000 | - | vision | Active | - |
| `google/gemma-3-27b-it` | Gemma 3 27B | 98,304 | 16,384 | $0.1190 | $0.2000 | - | serverless, vision | Active | - |
| `google/gemma-4-26b-a4b-it` | Gemma 4 26B A4B | 262,144 | 131,072 | $0.1300 | $0.4000 | - | tool-calling, JSON, reasoning, serverless, vision | Active | - |
| `google/gemma-4-31b-it` | Gemma 4 31B | 262,144 | 131,072 | $0.1400 | $0.4000 | - | tool-calling, JSON, reasoning, serverless, vision | Active | - |

### KwaiKAT (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `kwaipilot/kat-coder-pro` | Kat Coder Pro | 256,000 | 128,000 | $0.3000 | $1.2000 | $0.0600 | tool-calling, JSON, serverless | Active | - |

### OpenAI (GPT-OSS) (2)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `openai/gpt-oss-120b` | OpenAI GPT OSS 120B | 131,072 | 32,768 | $0.0500 | $0.2500 | - | tool-calling, JSON, reasoning, serverless, vision | Active | - |
| `openai/gpt-oss-20b` | OpenAI: GPT OSS 20B | 131,072 | 32,768 | $0.0400 | $0.1500 | - | JSON, reasoning, serverless, vision | Active | - |

### Meta (Llama) (8)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `meta-llama/llama-3-70b-instruct` | Llama3 70B Instruct | 8,192 | 8,000 | $0.5100 | $0.7400 | - | JSON, serverless | Archived | - |
| `meta-llama/llama-3-8b-instruct` | Llama 3 8B Instruct | 8,192 | 8,192 | $0.0400 | $0.0400 | - | JSON, serverless | Archived | - |
| `meta-llama/llama-3.1-8b-instruct` | Llama 3.1 8B Instruct | 16,384 | 16,384 | $0.0200 | $0.0500 | - | JSON, serverless | Active | - |
| `meta-llama/llama-3.2-1b-instruct` | Llama 3.2 1B Instruct  | 131,000 | 32,000 | $0.0200 | $0.0200 | - | JSON | Active | - |
| `meta-llama/llama-3.2-3b-instruct` | Llama 3.2 3B Instruct | 32,768 | 32,000 | $0.0300 | $0.0500 | - | - | Active | - |
| `meta-llama/llama-3.3-70b-instruct` | Llama 3.3 70B Instruct | 6,000 | 120,000 | $0.1350 | $0.4000 | - | tool-calling, JSON, serverless | Active | - |
| `meta-llama/llama-4-maverick-17b-128e-instruct-fp8` | Llama 4 Maverick Instruct | 1,048,576 | 8,192 | $0.2700 | $0.8500 | - | JSON, serverless, vision | Active | - |
| `meta-llama/llama-4-scout-17b-16e-instruct` | Llama 4 Scout Instruct | 131,072 | 131,072 | $0.1800 | $0.5900 | - | serverless, vision | Active | - |

### Mistral (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `mistralai/mistral-nemo` | Mistral Nemo | 60,288 | 16,000 | $0.0400 | $0.1700 | - | JSON, serverless | Active | - |

### Baichuan (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `baichuan/baichuan-m2-32b` | BaiChuan M2 32B | 131,072 | 131,072 | $0.0700 | $0.0700 | - | - | Active | - |

### Microsoft (WizardLM) (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `microsoft/wizardlm-2-8x22b` | Wizardlm 2 8x22B | 65,535 | 8,000 | $0.6200 | $0.6200 | - | JSON, serverless | Active | - |

### NousResearch (2)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `nousresearch/hermes-2-pro-llama-3-8b` | Hermes 2 Pro Llama 3 8B | 8,192 | 8,192 | $0.1400 | $0.1400 | - | JSON | Active | - |
| `nousresearch/nous-hermes-llama2-13b` | Nous Hermes Llama2 13B | 4,096 | 32,768 | $0.1700 | $0.1700 | - | - | Archived | - |

### Teknium (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `teknium/openhermes-2.5-mistral-7b` | Openhermes2.5 Mistral 7B | 4,096 | 8,000 | $0.1700 | $0.1700 | - | - | Archived | - |

### OpenChat (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `openchat/openchat-7b` | OpenChat 7B | 4,096 | 4,096 | $0.0600 | $0.0600 | - | JSON | Archived | - |

### Gryphe (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `gryphe/mythomax-l2-13b` | Mythomax L2 13B | 4,096 | 3,200 | $0.0900 | $0.0900 | - | - | Active | - |

### PaddlePaddle (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `paddlepaddle/paddleocr-vl` | PaddleOCR-VL | 16,384 | 16,384 | $0.0200 | $0.0200 | - | vision | Active | - |

### Xiaomi (MiMo) (4)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `xiaomimimo/mimo-v2-flash` | XiaomiMiMo/MiMo-V2-Flash | 262,144 | 32,000 | $0.1100 | $0.3300 | $0.0240 | tool-calling, JSON, reasoning, serverless, anthropic-api | Archived | - |
| `xiaomimimo/mimo-v2-pro` | XiaomiMiMo/MiMo-V2-Pro | 1,048,576 | 131,072 | $2.0000 | $6.0000 | $0.4000 | tool-calling, JSON, reasoning, serverless, anthropic-api | Archived | Tiered: 1-262144t:$1.0000/$3.0000; 262144-1048576t:$2.0000/$6.0000 |
| `xiaomimimo/mimo-v2.5` | XiaomiMiMo/MiMo-V2.5 | 1,048,576 | 131,072 | $0.1680 | $0.3360 | $0.0034 | tool-calling, JSON, reasoning, serverless, vision, video, anthropic-api | Active | - |
| `xiaomimimo/mimo-v2.5-pro` | XiaomiMiMo/MiMo-V2.5-Pro | 1,048,576 | 131,072 | $0.5220 | $1.0440 | $0.0043 | tool-calling, JSON, reasoning, serverless, anthropic-api | Active | - |

### Nex-AGI (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `nex-agi/nex-n2-pro` | Nex-N2-Pro | 262,144 | 262,144 | Free | Free | - | tool-calling, JSON, reasoning, serverless, vision | Archived | - |

### InclusionAI (Ling alias) (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `elephant` | Ling-2.6-flash | 262,144 | 32,768 | $0.1000 | $0.3000 | $0.0200 | tool-calling, JSON, serverless | Archived | - |

### Novita (Bunny) (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `bunny` | Bunny | 262,144 | 32,768 | Free | Free | - | tool-calling, JSON, serverless | Active | - |

### THUDM (GLM) (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `thudm/glm-4-32b-0414` | GLM-4-32B-0414 | 32,000 | 32,000 | $0.5500 | $1.6600 | - | tool-calling, JSON, serverless | Active | - |

### Novita (internal) (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `gt-4p` | gt-4p | - | 131,072 | Free | Free | - | tool-calling, JSON, serverless, vision | Active | - |

### Dev/Test (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `dev/glm46` | dev/glm46 | 256,000 | 256,000 | Free | Free | - | tool-calling, JSON, serverless, anthropic-api | Active | - |

### Novita (test) (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `ai_infer_test_1` | ai_infer_test_1 | 200,000 | 200,000 | Free | Free | - | tool-calling, JSON, serverless | Active | - |

### Novita (test) (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `ai_infer_test_2` | ai_infer_test_2 | 200,000 | 200,000 | Free | Free | - | tool-calling, JSON, serverless | Active | - |

### Novita (test) (1)

| Model ID | Display Name | Context | Max Output | Input $/1M | Output $/1M | Cache Read $/1M | Capabilities | Status | Notes |
|---|---|---:|---:|---:|---:|---:|---|---|---|
| `ai_infer_test_3` | ai_infer_test_3 | 200,000 | 200,000 | Free | Free | - | tool-calling, JSON, serverless | Active | - |

---

## 2. Embedding & Reranker Models

From the pricing page (`/pricing` → Embeddings section). These are not in the chat models endpoint.

### Embeddings

| API Name | Context | Input $/1M | Notes |
|---|---:|---:|---|
| `qwen/qwen3-embedding-0.6b` | 32,768 | $0.07 | Qwen3 embedding, 0.6B |
| `Qwen3 Embedding 8B` | 4,096 | $0.07 | Qwen3 embedding, 8B |
| `BAAI/BGE-M3` | 96,000 | $0.01 | BGE-M3 multilingual embedding |

### Rerankers

| API Name | Context | Input $/1M | Output $/1M |
|---|---:|---:|---:|
| `Qwen3 Reranker 8B` | 4,096 | $0.05 | $0.05 |
| `baai/bge-reranker-v2-m3` | 8,000 | $0.01 | $0.01 |

---

## 3. Image Generation & Editing Models

From `/pricing` → Image section. Prices are **per image**. Pricing varies with image dimensions, inference steps, and upscaling factors.

### Diffusion / text-to-image / image-to-image

| API Name | Mode | Price / image | Notes |
|---|---|---:|---|
| Text to Image | 512×512, 5 steps | $0.001 | Generic txt2img |
| Image to Image | 512×512, 5 steps | $0.001 | Generic img2img |
| Inpainting | 512×512, 5 steps | $0.0015 | Image edit |
| Remove Background | — | $0.017 | Image edit |
| Replace Background | — | $0.0255 | Image edit |
| Remove Text | — | $0.017 | Text edit |
| Cleanup | — | $0.017 | Image edit |
| Merge Face | — | $0.0255 | Face edit |
| Image Eraser | — | $0.0250 | Image edit |
| Image Remove Background | — | $0.0180 | Image edit |
| Image Upscaler | — | $0.0100 | Upscale |
| Seedream 4.0 | — | $0.03 | Text-to-image, image-to-image |
| Seedream 4.5 | — | $0.0300 | Text-to-image, image-to-image |
| Seedream 5.0 lite | — | $0.0350 | Text-to-image, image-to-image |
| Qwen-Image Text to Image | — | $0.02 | Text to image |
| Qwen-Image Edit | — | $0.02 | Image edit |
| Z Image Turbo | — | $0.0050 | Text-to-image |
| Z Image Turbo LoRA | — | $0.0100 | Text-to-image |

### Flux.1 Kontext (image-to-image)

| API Name | Mode | Price / image |
|---|---|---:|
| Flux.1 Kontext Dev | — | $0.0225 |
| Flux.1 Kontext Dev | fast_mode | $0.018 |
| Flux.1 Kontext Max | — | $0.072 |
| Flux.1 Kontext Pro | — | $0.36 |

---

## 4. Video Generation Models

From `/pricing` → Video section. Prices vary by frames, steps, resolution, and duration. Listed per-video or per-second.

| API Name | Mode | Duration | Resolution | Pricing |
|---|---|---|---|---|
| Text to Video | 32 frames, 20 steps | — | — | $0.0307 /video |
| Heygen Video-translate | — | — | — | $0.0375 /video |
| Hunyuan Video Fast | — | 5s | 1280×720 | $0.30 /video |
| Kling V1.6 Image to Video | Standard | 5s | 720P | $0.27 /video |
| Kling V1.6 Image to Video | Standard | 10s | 720P | $0.54 /video |
| Kling V1.6 Image to Video | Professional | 5s | 1080P | $0.46 /video |
| Kling V1.6 Image to Video | Professional | 10s | 1080P | $0.92 /video |
| Kling V1.6 Text to Video | Standard | 5s | 720P | $0.27 /video |
| Kling V1.6 Text to Video | Standard | 10s | 720P | $0.54 /video |
| Kling V2.5 Turbo Image to Video | — | 5s | 1080P | $0.35 /video |
| Kling V2.5 Turbo Image to Video | — | 10s | 1080P | $0.70 /video |
| Kling V2.5 Turbo Text to Video | — | 5s | 1080P | $0.35 /video |
| Kling V2.5 Turbo Text to Video | — | 10s | 1080P | $0.70 /video |
| Kling V2.6 Pro Image to Video | No Audio | 5s | 1080P | $0.35 /video |
| Kling V2.6 Pro Image to Video | No Audio | 10s | 1080P | $0.70 /video |
| Kling V2.6 Pro Image to Video | Audio | 5s | 1080P | $0.70 /video |
| Kling V2.6 Pro Image to Video | Audio | 10s | 1080P | $1.40 /video |
| Kling V2.6 Pro Text to Video | No Audio | 5s | 1080P | $0.35 /video |
| Kling V2.6 Pro Text to Video | No Audio | 10s | 1080P | $0.70 /video |
| Kling V2.6 Pro Text to Video | Audio | 5s | 1080P | $0.70 /video |
| Kling V2.6 Pro Text to Video | Audio | 10s | 1080P | $1.40 /video |
| Kling v3.0 4K Image-to-Video | No Audio | — | — | $0.4200 /s |
| Kling v3.0 4K Image-to-Video | Audio | — | — | $0.6300 /s |
| Kling v3.0 4K Text-to-Video | No Audio | — | — | $0.4200 /s |
| Kling v3.0 4K Text-to-Video | Audio | — | — | $0.6300 /s |
| Kling V3.0 Motion Control | Standard | — | — | $0.1260 /s |
| Kling V3.0 Motion Control | Professional | — | — | $0.1680 /s |
| Kling v3.0 Pro Image-to-Video | No Audio | — | — | $0.1120 /s |
| Kling v3.0 Pro Image-to-Video | Audio | — | — | $0.1680 /s |
| Kling v3.0 Pro Text-to-Video | No Audio | — | — | $0.1120 /s |
| Kling v3.0 Pro Text-to-Video | Audio | — | — | $0.1680 /s |
| Kling v3.0 Standard Image-to-Video | No Audio | — | — | $0.0840 /s |
| Kling v3.0 Standard Image-to-Video | Audio | — | — | $0.1260 /s |
| Kling v3.0 Standard Text-to-Video | No Audio | — | — | $0.0840 /s |
| Kling v3.0 Standard Text-to-Video | Audio | — | — | $0.1260 /s |
| Minimax Hailuo 2.3 Fast Image to Video | — | 6s | 768P | $0.19 /video |
| Minimax Hailuo 2.3 Fast Image to Video | — | 10s | 768P | $0.32 /video |
| Minimax Hailuo 2.3 Fast Image to Video | — | 6s | 1080P | $0.33 /video |
| Minimax Hailuo 2.3 Image to Video | — | 6s | 768P | $0.28 /video |
| Minimax Hailuo 2.3 Image to Video | — | 10s | 768P | $0.56 /video |
| Minimax Hailuo 2.3 Image to Video | — | 6s | 1080P | $0.49 /video |
| Minimax Hailuo 2.3 Text to Video | — | 6s | 768P | $0.28 /video |
| Minimax Hailuo 2.3 Text to Video | — | 10s | 768P | $0.56 /video |
| Minimax Hailuo 2.3 Text to Video | — | 6s | 1080P | $0.49 /video |
| PixVerse V4.5 Image to Video | — | 5s | 360P/540P | $0.25 /video |
| PixVerse V4.5 Image to Video | — | 5s | 720P | $0.35 /video |
| PixVerse V4.5 Image to Video | — | 5s | 1080P | $0.70 /video |
| PixVerse V4.5 Image to Video | fast_mode | 5s | 360P/540P | $0.50 /video |
| PixVerse V4.5 Image to Video | fast_mode | 5s | 720P | $0.70 /video |
| PixVerse V4.5 Text to Video | — | 5s | 360P/540P | $0.25 /video |
| PixVerse V4.5 Text to Video | — | 5s | 720P | $0.35 /video |
| PixVerse V4.5 Text to Video | — | 5s | 1080P | $0.70 /video |
| PixVerse V4.5 Text to Video | fast_mode | 5s | 360P/540P | $0.50 /video |
| PixVerse V4.5 Text to Video | fast_mode | 5s | 720P | $0.70 /video |
| Seedance 1.5 Pro Image To Video | FLF/Online/Audio | — | 1080p | $0.1160 /s |
| Seedance 1.5 Pro Image To Video | FLF/Batch/Audio | — | 1080p | $0.0580 /s |
| Seedance 1.5 Pro Image To Video | FLF/Online/Silent | — | 1080p | $0.0580 /s |
| Seedance 1.5 Pro Image To Video | FLF/Batch/Silent | — | 1080p | $0.0290 /s |
| Seedance 1.5 Pro Image To Video | FF/Online/Audio | — | 1080p | $0.1160 /s |
| Seedance 1.5 Pro Image To Video | FF/Batch/Audio | — | 1080p | $0.0580 /s |
| Seedance 1.5 Pro Image To Video | FF/Online/Silent | — | 1080p | $0.0580 /s |
| Seedance 1.5 Pro Image To Video | FF/Batch/Silent | — | 1080p | $0.0290 /s |
| Seedance 1.5 Pro Text To Video | FF/Online/Audio | — | 1080p | $0.1160 /s |
| Seedance 1.5 Pro Text To Video | FF/Batch/Audio | — | 1080p | $0.0580 /s |
| Seedance 1.5 Pro Text To Video | FF/Online/Silent | — | 1080p | $0.0580 /s |
| Seedance 1.5 Pro Text To Video | FF/Batch/Silent | — | 1080p | $0.0290 /s |
| Seedance 1.5 Pro (480p/720p tiers also available at $0.0060–$0.0520/s) | — | — | 480p/720p | see pricing page |
| Vidu Q2 (Text/Reference to Video) | Text to Video | 5s | 540P/720P/1080P | $0.0802–$0.2677 /video |
| Vidu Q2 (Reference to Video) | Reference to Video | 5s | 540P/720P/1080P | $0.1562–$0.5132 /video |
| Vidu Q2 Pro (Image to Video) | Image to Video | 5s | 540P/720P/1080P | $0.1472–$0.5135 /video |
| Vidu Q2 Pro Fast (Image to Video) | Image to Video | 5s | 720P/1080P | $0.0713–$0.143 /video |
| Vidu Q2 Template | credit-based | — | — | $0.045–$2.07 /video |
| Vidu Q2 Turbo (Image to Video) | Image to Video | 5s | 540P/720P/1080P | $0.0624–$0.3347 /video |
| Vidu Q3 Pro Image-to-Video | Off-Peak/Peak | — | 540P/720P/1080P | $0.0313–$0.1429 /s |
| Vidu Q3 Pro Start-End-to-Video | Off-Peak/Peak | — | 540P/720P/1080P | $0.0313–$0.1429 /s |
| Vidu Q3 Pro Text to Video | Off-Peak/Peak | — | 540P/720P/1080P | $0.0313–$0.1429 /s |
| Vidu Q3 Turbo Image-to-Video | Off-Peak/Peak | — | 540P/720P/1080P | $0.0179–$0.0714 /s |
| Vidu Q3 Turbo Start-End-to-Video | Off-Peak/Peak | — | 540P/720P/1080P | $0.0179–$0.0714 /s |
| Vidu Q3 Turbo Text-to-Video | Off-Peak/Peak | — | 540P/720P/1080P | $0.0179–$0.0714 /s |
| Wan 2.1 Image to Video | LoRA/Fast/Standard | — | 480P/720P | $0.1250–$0.3000 /video |
| Wan 2.1 Text to Video | LoRA/Fast/Standard | — | 480P/720P | $0.1250–$0.3000 /video |
| Wan 2.2 Image to Video | No LoRA/LoRA | 5s/8s | 480P/720P/1080P | $0.0900–$0.5040 /video |
| Wan 2.2 Text to Video | No LoRA/LoRA | 5s/8s | 480P/720P/1080P | $0.0900–$0.5040 /video |
| Wan 2.5 Image to Video | — | 5s/10s | 480P/720P/1080P | $0.25–$1.50 /video |
| Wan 2.5 Image to Video Preview | — | 5s/10s | 480P/720P/1080P | $0.2500–$1.5000 /video |
| Wan 2.5 Text to Video | — | 5s/10s | 480P/720P/1080P | $0.25–$1.50 /video |
| Wan 2.5 Text to Video Preview | — | 5s/10s | 480P/720P/1080P | $0.2500–$1.5000 /video |
| Wan 2.6 Image to Video | — | 5s/10s/15s | 720P/1080P | $0.50–$2.25 /video |
| Wan 2.6 Reference to Video | — | 5s/10s | 720P/1080P | $0.50–$1.50 /video |
| Wan 2.6 Text to Video | — | 5s/10s/15s | 720P/1080P | $0.50–$2.25 /video |
| Wan 2.6 Video Reference | — | 5s/10s | 720P/1080P | $0.5000–$1.5000 /video |
| Wan 2.7 Image-to-Video | — | — | 720P/1080P | $0.1000–$0.1500 /s |
| Wan 2.7 Reference-to-Video | — | — | 720P/1080P | $0.1000–$0.1500 /s |
| Wan 2.7 Text-to-Video | — | — | 720P/1080P | $0.1000–$0.1500 /s |
| Wan 2.7 Video Editing | — | — | 720P/1080P | $0.1000–$0.1500 /s |
| Image to Video (SVD) | SVD-XT, 20 steps | — | — | $0.024 /video |
| Image to Video (SVD) | SVD, 20 steps | — | — | $0.0134 /video |

> Seedance 1.5 Pro has many FF/FLF × Batch/Online × Silent/Audio × 480p/720p/1080p tiers
> (full per-second rates from $0.0060 to $0.1160/s). See the pricing page for the complete matrix.

---

## 5. Audio Models (TTS, Music, Voice)

From `/pricing` → Audio section.

| API Name | Mode | Pricing |
|---|---|---|
| Fish Audio Text to Speech | — | $15 /1M characters |
| Fish Audio Voice Cloning | — | $0.10 /voice |
| Fish Audio S2 Pro Text to Speech | — | $15.0000 /1M characters |
| MiniMax Speech 2.8 HD Async Text-to-Speech | — | $100.0000 /1M characters |
| MiniMax Speech 2.8 HD Sync Text-to-Speech | — | $100.0000 /1M characters |
| MiniMax Speech 2.8 Turbo Async Text-to-Speech | — | $60.0000 /1M characters |
| MiniMax Speech 2.8 Turbo Sync Text-to-Speech | — | $60.0000 /1M characters |
| MiniMax Lyrics | — | $0.0100 /song |
| MiniMax Music | music-2.5+ | $0.1500 /song |
| MiniMax Music | music-2.5 | $0.1500 /song |
| MiniMax Music | music-2.0 | $0.0300 /song |
| MiniMax speech-2.6-hd | T2A / T2A Async | $100 /1M characters |
| MiniMax speech-2.6-turbo | T2A / T2A Async | $60 /1M characters |
| MiniMax Voice Design | — | $3.0000 /voice |
| MiniMax Voice-Cloning | — | $1.50 /voice |
| MOSS TTS v1.5 | — | Free |
| Text to Speech (generic) | — | $15 /1M characters |

---

## 6. AI Search Providers

From `/pricing` → AI Search section.

### EXA

| Mode | Pricing |
|---|---|
| neuralSearch | $0.007 /request |
| deepSearch | $0.012 /request |
| deepReasoningSearch | $0.015 /request |
| additional_result | $0.001 /result (beyond 10 results) |
| answer | $0.005 /request |
| contentText | $0.001 /item |
| contentHighlight | $0.001 /item |
| contentSummary | $0.001 /item |

### Tavily

| Mode | Pricing |
|---|---|
| basicSearch | $0.008 /request |
| advancedSearch | $0.016 /request |
| basicExtract | $0.0016 /url |
| advancedExtract | $0.0032 /url |
| regularMapping | $0.0008 /page |
| instructedMapping | $0.0016 /page |
| Crawl | Extract + Mapping Cost |

---

## 7. Model families at a glance

| Family | Example IDs | Notes |
|---|---|---|
| DeepSeek | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash`, `deepseek/deepseek-v3.2`, `deepseek/deepseek-r1-0528`, `deepseek/deepseek-ocr-2` | V4-Pro/Flash flagships (1M ctx, reasoning, tools), V3.x, R1 reasoning, OCR vision, distills, Turbo & community variants |
| Qwen | `qwen/qwen3.7-max`, `qwen/qwen3.6-27b`, `qwen/qwen3.5-397b-a17b`, `qwen/qwen3-coder-480b-a35b-instruct`, `qwen/qwen3-vl-235b-a22b-thinking` | 35 models: 3.x/3.5/3.6/3.7, Coder, VL (vision), Omni (audio+video), MT (translation), embedding & reranker |
| GLM / Z.AI | `zai-org/glm-5.2`, `zai-org/glm-5.1`, `zai-org/glm-5-turbo`, `zai-org/glm-4.6`, `zai-org/glm-4.5v`, `zai-org/autoglm-phone-9b-multilingual` | GLM-5.x flagship (up to 1M ctx), 4.x, V (vision), Turbo, AutoGLM phone agent |
| Kimi / MoonshotAI | `moonshotai/kimi-k3`, `moonshotai/kimi-k2.7-code`, `moonshotai/kimi-k2.6`, `moonshotai/kimi-k2-thinking` | K3 (2.8T params, 1M ctx, multimodal), K2.x code/thinking/instruct variants |
| MiniMax | `minimax/minimax-m3`, `minimax/minimax-m2.7`, `minimax/minimax-m2.5-highspeed`, `minimaxai/minimax-m1-80k` | M3 flagship (1M ctx, multimodal), M2.x, highspeed, M1 (1M ctx) |
| Llama / Meta | `meta-llama/llama-4-maverick-17b-128e-instruct-fp8`, `meta-llama/llama-4-scout-17b-16e-instruct`, `meta-llama/llama-3.3-70b-instruct`, `meta-llama/llama-3.1-8b-instruct` | Llama 4 (Maverick/Scout, vision), 3.3, 3.2, 3.1, 3 |
| Gemma / Google | `google/gemma-4-31b-it`, `google/gemma-4-26b-a4b-it`, `google/gemma-3-27b-it`, `google/gemma-3-12b-it` | Gemma 4 (256K ctx, vision, reasoning), Gemma 3 |
| Baidu ERNIE | `baidu/cobuddy`, `baidu/ernie-4.5-vl-424b-a47b`, `baidu/ernie-4.5-21b-a3b` | ERNIE 4.5, VL (vision), thinking variants, CoBuddy coding |
| Xiaomi MiMo | `xiaomimimo/mimo-v2.5`, `xiaomimimo/mimo-v2.5-pro`, `xiaomimimo/mimo-v2-pro` | MiMo-V2/V2.5 (1M ctx, multimodal, agentic) |
| InclusionAI Ling | `inclusionai/ling-3.0-flash`, `inclusionai/ling-2.6-1t`, `inclusionai/ring-2.6-1t` | Ling (MoE), Ring (reasoning), 3.0 free |
| Others | NVIDIA Nemotron, OpenAI GPT-OSS (120B/20B), StepFun Step-3.7, Tencent Hy3, Baichuan M2, KwaiKAT Kat-Coder, Microsoft WizardLM, NousResearch Hermes, Sao10K roleplay models, PaddleOCR-VL, etc. | wide variety of open models |

---

## Methodology

1. **`GET https://api.novita.ai/v3/openai/models`** — this OpenAI-compatible endpoint returned a JSON object with a `data` array of 142 model objects. Each object contains `id` (exact API model ID), `display_name`, `context_size`, `max_output_tokens`, `features` (array incl. `function-calling`, `structured-outputs`, `reasoning`, `serverless`), `input_modalities` / `output_modalities` (text/image/video/audio), `endpoints` (e.g. `chat/completions`, `anthropic`, `responses`), `input_token_price_per_m` / `output_token_price_per_m` (integer cents-per-1M-tokens), and `pricing.input_cache_read`.
2. **`https://novita.ai/pricing`** — the public pricing page was rendered and its full text captured. It organizes LLMs by vendor family (Deepseek, Qwen, Baidu, Zai-org, Sao10K, Mind Lab, inclusionai, Hunyuan, MoonshotAI, MiniMax, StepFun, Nvidia, Gemma, KwaiKAT, OpenAI, Llama, Mistral, Others) plus dedicated sections for **Embeddings**, **Image**, **Video**, **Audio**, and **AI Search**. These sections contain models not exposed by the chat-models endpoint.
3. Prices from the API are in integer cents-per-million (e.g. `1400` → `$0.1400/1M`). All prices in this document are normalized to USD per 1M tokens (LLM), per image (image models), per video or per second (video), or per 1M characters / per song / per voice (audio).

_Compiled from live data. Model availability and pricing may change; verify at the source URLs before production use._
