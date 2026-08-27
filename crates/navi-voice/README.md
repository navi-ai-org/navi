# navi-voice

Local speech dictation for NAVI: mic capture discovery, model download, and ONNX Nemotron streaming ASR.

## Engines

| Id | Runtime | Status |
|----|---------|--------|
| `nemotron_streaming` | ONNX Runtime (`ort`) | Working — streaming RNNT |
| `distil_whisper` | candle | Planned (PR4) |

## CLI

```bash
navi voice init --engine nemotron_streaming   # download + verify (~800MB)
navi voice status
navi voice doctor
navi voice transcribe /path/to/audio.wav --language en-US
```

Model files live under `{data_dir}/voice/models/` (Linux default: `~/.local/share/navi/voice/models/`).

## Features

- `onnx` (default): Nemotron 3.5 ASR Streaming 0.6B INT4 via ONNX Runtime.
- Disable with `--no-default-features` on `navi-voice` / omit `voice-onnx` on `navi-cli` if you only need download/doctor without ORT.

## Library

```rust
use navi_voice::NemotronOnnxEngine;

let mut engine = NemotronOnnxEngine::load(model_dir, "en-US")?;
let result = engine.transcribe_wav("sample.wav")?;
println!("{}", result.text);
```

Streaming: `push_audio` / `process_pcm_chunk` / `flush` with 16 kHz mono f32.

## Tests

```bash
cargo test -p navi-voice -- --test-threads=4
# E2E (needs installed model + /tmp/libri16.wav or NAVI_VOICE_TEST_WAV):
cargo test -p navi-voice --test transcribe_libri -- --nocapture
```
