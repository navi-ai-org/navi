# navi-dart

[![Crates.io](https://img.shields.io/crates/v/navi-dart)](https://crates.io/crates/navi-dart)
[![License](https://img.shields.io/crates/l/navi-dart)](../LICENSE)

Dart FFI bindings for the [NAVI](https://github.com/navi-ai-org/navi) agent runtime SDK.

`navi-dart` exposes a C-compatible ABI (`extern "C"`) that [`dart:ffi`](https://api.dart.dev/stable/dart-ffi/dart-ffi-library.html) can consume, enabling Dart and Flutter applications to embed the NAVI agent engine.

## Design

- **Opaque pointers** for engine handles — Dart sees `Pointer<Void>`, Rust manages lifetimes
- **JSON strings** for complex data (sessions, turns, events) — simple FFI boundary
- **Callback function pointers** for async operations — cross-thread notification from Rust to Dart
- **Panic isolation** — panics in the agent runtime are caught and reported as errors, not crashes

## Usage from Dart

```dart
import 'dart:ffi';
import 'package:navi_dart/navi_dart.dart';

final engine = NaviEngine.create(projectDir: '/path/to/project');
final session = engine.startSession();
final response = engine.sendTurn(session.id, 'Explain this codebase');
print(response.text);
engine.closeSession(session.id);
```

## Building

Requires a Rust toolchain. Build the `cdylib` for your target platform:

```bash
cargo build -p navi-dart --release
```

The output is a shared library (`.so`, `.dylib`, or `.dll`) that Dart's FFI can load.

## Part of the NAVI workspace

This crate depends on [`navi-sdk`](https://crates.io/crates/navi-sdk) and [`navi-core`](https://crates.io/crates/navi-core).

**Full project:** <https://github.com/navi-ai-org/navi>
**License:** Apache-2.0
