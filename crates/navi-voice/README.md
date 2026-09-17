# navi-voice

Voice dictation support for NAVI: mic recorder discovery, WAV helpers, and
**remote** speech-to-text via registry transcription providers (OpenAI / Groq
Whisper).

> Local ONNX inference (`ort` / `ort-sys`, Nemotron streaming ASR) was removed.
> There is no local STT/TTS engine in this crate.

## Remote transcription

Provider metadata (base URL, API key env, model list) comes from the NAVI
registry; `navi-core` resolves the credentials and builds the request.

```rust
use navi_voice::{RemoteTranscriptionConfig, RemoteTranscriptionKind, transcribe_file_remote};

let cfg = RemoteTranscriptionConfig {
    provider_id: "openai".into(),
    kind: RemoteTranscriptionKind::OpenaiAudioTranscriptions,
    base_url: "https://api.openai.com/v1".into(),
    transcription_path: "/audio/transcriptions".into(),
    api_key: std::env::var("OPENAI_API_KEY")?,
    model: "whisper-1".into(),
    language: Some("en".into()),
};
let result = transcribe_file_remote(&cfg, std::path::Path::new("clip.wav")).await?;
println!("{}", result.text);
```

## Capture helpers

- `capture`: discovers `pw-record` / `parec` / `arecord` on `PATH`.
- `wav`: load/resample mono `f32` audio and encode 16 kHz 16-bit PCM WAV in memory.
- `doctor`: diagnostics for the recorder stack (`run_doctor`).

## CLI

```bash
navi voice status
navi voice providers
navi voice doctor
navi voice transcribe /path/to/audio.wav --language en-US
```

Set a remote provider in `~/.config/navi/config.toml`:

```toml
[voice]
enabled = true
provider = "openai"   # or groq
model = "whisper-1"
language = "auto"
```

## Tests

```bash
cargo test -p navi-voice -- --test-threads=4
```
