# navi-dart

Dart FFI bindings for the NAVI agent runtime SDK.

## Architecture

`navi-dart` is a Rust crate that compiles to a C-compatible shared library (`.so`/`.dylib`/`.dll`). It wraps `navi-sdk`'s `NaviEngine` and exposes a C ABI surface that Dart can consume via `dart:ffi`.

```
┌──────────────┐     dart:ffi     ┌──────────────┐     Rust API     ┌──────────────┐
│  Dart/Flutter│ ───────────────→ │  navi-dart   │ ───────────────→ │  navi-sdk    │
│  (navi_agent)│ ←─────────────── │  (cdylib)    │ ←─────────────── │  NaviEngine  │
│              │   C ABI + JSON   │              │                  │              │
└──────────────┘                  └──────────────┘                  └──────────────┘
```

## Features

- **Full engine surface**: sessions, turns, events, approvals, goals, background tasks, credentials, skills, MCP, provider sync, registry, plugins, saved sessions, config
- **Async via callbacks**: async operations use `NativeCallable.listener` for safe cross-thread notification
- **Event streaming**: subscribe to real-time `RuntimeEvent`s via persistent callbacks
- **JSON interchange**: complex types are serialized as JSON strings across the FFI boundary
- **Memory-safe**: opaque pointers for engine handles, explicit `navi_string_free` for allocated strings

## Building

```bash
# Debug build
cargo build -p navi-dart

# Release build
cargo build -p navi-dart --release
```

The output is a shared library at `target/{debug,release}/libnavi_dart.{so,dylib,dll}`.

## Testing

```bash
# Rust unit + integration tests
cargo test -p navi-dart -- --test-threads=1
```

## C ABI Surface

### Engine lifecycle

| Function | Returns | Description |
|---|---|---|
| `navi_engine_new(project_dir)` | `*mut NaviDartEngine` | Create engine |
| `navi_engine_new_learning_tutor(project_dir)` | `*mut NaviDartEngine` | Create learning tutor engine |
| `navi_engine_free(engine)` | `void` | Free engine handle |

### Sessions

| Function | Returns | Description |
|---|---|---|
| `navi_engine_start_session(engine, request_json, cb, ud)` | async | Start session |
| `navi_engine_close_session(engine, session_id, cb, ud)` | async | Close session |
| `navi_engine_session_ids(engine)` | `*mut c_char` (JSON) | List active session IDs |

### Turns

| Function | Returns | Description |
|---|---|---|
| `navi_engine_send_turn(engine, request_json, cb, ud)` | async | Send turn |
| `navi_engine_cancel_turn(engine, session_id, cb, ud)` | async | Cancel turn |

### Events

| Function | Returns | Description |
|---|---|---|
| `navi_engine_subscribe_events(engine, session_id, cb, ud)` | `*mut NaviEventSubscription` | Subscribe to events |
| `navi_event_subscription_free(sub)` | void | Free subscription |

### Models & Config

| Function | Returns | Description |
|---|---|---|
| `navi_engine_list_models(engine)` | `*mut c_char` (JSON) | List models |
| `navi_engine_set_model(engine, sid, provider, model, cb, ud)` | async | Set model |
| `navi_engine_select_model(engine, pid, model, target, cb, ud)` | async | Select model |
| `navi_engine_loaded_config(engine)` | `*mut c_char` (JSON) | Get config |

### And more...

Goals, background tasks, credentials, skills, MCP, provider sync, registry, plugins, saved sessions — all covered via the same pattern.

## Dart Package

The companion Dart package `navi_agent` (at `dart/navi_agent/`) provides idiomatic Dart bindings:

```dart
import 'package:navi_agent/navi_agent.dart';

final engine = await NaviEngine.fromProject('.');
final session = await engine.startSession();
final response = await engine.sendTurn(session.id, 'Hello!');
print(response.text);
engine.dispose();
```

## Async Pattern

Async operations use callback function pointers:

1. Dart creates a `NativeCallable.listener` (safely bridges cross-thread calls)
2. Dart passes the callback pointer to Rust via FFI
3. Rust spawns a tokio task, calls the callback when done
4. The callback runs on the Dart isolate's event loop

Event streaming uses the same pattern with a persistent callback per subscription.
