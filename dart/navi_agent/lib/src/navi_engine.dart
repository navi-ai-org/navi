/// High-level Dart API for the NAVI agent engine.
///
/// Wraps the native C ABI functions exposed by the navi-dart Rust crate,
/// providing idiomatic Dart async/await and Stream APIs.
///
/// ```dart
/// final engine = await NaviEngine.fromProject('/path/to/project');
/// final session = await engine.startSession();
/// final response = await engine.sendTurn(session.id, 'Hello!');
/// print(response.text);
/// engine.dispose();
/// ```
import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

import 'native_library.dart' as native;
import 'types.dart';

/// The main NAVI engine handle.
///
/// Create via [NaviEngine.fromProject].
/// Always call [dispose] when done.
class NaviEngine {
  final Pointer<native.NaviDartEngine> _ptr;
  bool _disposed = false;

  NaviEngine._(this._ptr);

  /// Creates a new engine for the given project directory.
  ///
  /// Config is loaded from `.navi/config.toml` in [projectDir] (if present)
  /// and the global config.
  static Future<NaviEngine> fromProject(String projectDir) async {
    final dirC = projectDir.toNativeUtf8();
    try {
      final ptr = native.naviEngineNew(dirC);
      if (ptr == nullptr) {
        throw NaviException(_lastError());
      }
      return NaviEngine._(ptr);
    } finally {
      calloc.free(dirC);
    }
  }

  /// Releases the engine handle and all associated resources.
  void dispose() {
    if (!_disposed) {
      _disposed = true;
      native.naviEngineFree(_ptr);
    }
  }

  // ── Sessions ─────────────────────────────────────────────────

  /// Starts a new agent session.
  ///
  /// Returns [SessionInfo] with the session ID and model details.
  /// If [sessionId] matches an existing session, returns its info.
  Future<SessionInfo> startSession({String? sessionId, String? projectDir}) {
    final request = <String, dynamic>{};
    if (sessionId != null) request['sessionId'] = sessionId;
    if (projectDir != null) request['projectDir'] = projectDir;
    final requestC = jsonEncode(request).toNativeUtf8();
    return _asyncCallString(
      requestC,
      (cb, ud) => native.naviEngineStartSession(_ptr, requestC, cb, ud),
      (json) => SessionInfo.fromJson(json),
    );
  }

  /// Closes an active session. Returns `true` if a session was removed.
  Future<bool> closeSession(String sessionId) {
    final sidC = sessionId.toNativeUtf8();
    return _asyncCallString(
      sidC,
      (cb, ud) => native.naviEngineCloseSession(_ptr, sidC, cb, ud),
      (json) => json as bool? ?? false,
    );
  }

  /// Returns the IDs of all active (in-memory) sessions.
  List<String> sessionIds() {
    final ptr = native.naviEngineSessionIds(_ptr);
    return _parseStringList(ptr);
  }

  // ── Turns ────────────────────────────────────────────────────

  /// Sends a user message to an active session and waits for the response.
  Future<TurnResponse> sendTurn(
    String sessionId,
    String message, {
    TurnOptions? options,
  }) {
    final request = <String, dynamic>{
      'sessionId': sessionId,
      'message': message,
    };
    if (options != null) {
      request.addAll(options.toJson());
    }
    final requestC = jsonEncode(request).toNativeUtf8();
    return _asyncCallString(
      requestC,
      (cb, ud) => native.naviEngineSendTurn(_ptr, requestC, cb, ud),
      (json) => TurnResponse.fromJson(json),
    );
  }

  /// Cancels the currently active turn for the given session.
  Future<void> cancelTurn(String sessionId) {
    final sidC = sessionId.toNativeUtf8();
    return _asyncCallString(
      sidC,
      (cb, ud) => native.naviEngineCancelTurn(_ptr, sidC, cb, ud),
      (_) {},
    );
  }

  // ── Models ───────────────────────────────────────────────────

  /// Lists all available models across configured providers.
  List<ModelInfo> listModels() {
    final ptr = native.naviEngineListModels(_ptr);
    final list = _parseJsonList(ptr);
    return list
        .map((e) => ModelInfo.fromJson(e as Map<String, dynamic>))
        .toList();
  }

  /// Changes the model used by an active session.
  Future<void> setModel(
    String sessionId,
    String provider,
    String model,
  ) {
    final sidC = sessionId.toNativeUtf8();
    final provC = provider.toNativeUtf8();
    final mdlC = model.toNativeUtf8();
    final completer = Completer<void>();

    final nativeCallable =
        native.NativeCallable<native.AsyncCallbackNative>.listener(
      (Pointer<Utf8> resultJson, Pointer<Utf8> error, Pointer<Void> ud) {
        calloc.free(sidC);
        calloc.free(provC);
        calloc.free(mdlC);
        _handleAsyncResult(completer, resultJson, error, (_) {});
        nativeCallable.close();
      },
    );

    native.naviEngineSetModel(
        _ptr, sidC, provC, mdlC, nativeCallable.nativeFunction, nullptr);
    return completer.future;
  }

  // ── Events ───────────────────────────────────────────────────

  /// Subscribes to the event stream for a session.
  ///
  /// Returns a [Stream] of [RuntimeEvent]s. Events include assistant deltas,
  /// tool calls, approval requests, and completion signals.
  ///
  /// The subscription is automatically cleaned up when the stream is cancelled.
  Stream<RuntimeEvent> subscribeEvents(String sessionId) {
    final controller = StreamController<RuntimeEvent>();
    final sessionIdC = sessionId.toNativeUtf8();

    // Each subscription gets its own callback that routes to exactly
    // one StreamController (NOT shared across all subscriptions).
    final callback = Pointer.fromFunction<native.EventCallbackNative>(
      _createEventCallbackFor(controller),
      0, // continue
    );

    final subPtr =
        native.naviEngineSubscribeEvents(_ptr, sessionIdC, callback, nullptr);
    calloc.free(sessionIdC);

    if (subPtr == nullptr) {
      controller.addError(NaviException(_lastError()));
      controller.close();
      return controller.stream;
    }

    _activeSubscriptions[controller.identityHashCode] = (
      subPtr,
      controller,
      callback,
    );

    controller.onCancel = () {
      final entry = _activeSubscriptions.remove(controller.identityHashCode);
      if (entry != null) {
        native.naviEventSubscriptionFree(entry.$1);
      }
    };

    return controller.stream;
  }

  static final Map<int,
      (
        Pointer<native.NaviEventSubscription>,
        StreamController,
        Pointer<NativeFunction<native.EventCallbackNative>>,
      )> _activeSubscriptions = {};

  /// Creates a per-subscription event callback.
  static native.EventCallbackDart _createEventCallbackFor(
      StreamController<RuntimeEvent> controller) {
    return (Pointer<Utf8> eventJson, Pointer<Void> userData) {
      if (controller.isClosed) return 1; // stop
      if (eventJson == nullptr) return 0;
      try {
        final jsonStr = eventJson.toDartString();
        final parsed = tryParseJson(jsonStr);
        if (parsed != null) {
          controller.add(RuntimeEvent.fromJson(parsed));
        }
      } catch (_) {}
      return 0;
    };
  }

  // ── Config ───────────────────────────────────────────────────

  /// Returns a snapshot of the current loaded configuration.
  EngineConfig loadedConfig() {
    final ptr = native.naviEngineLoadedConfig(_ptr);
    return _parseJson(ptr, (j) => EngineConfig.fromJson(j));
  }

  // ── Internal helpers ─────────────────────────────────────────

  /// Runs an async operation via callback and returns a Future.
  ///
  /// [strC] is the pre-allocated C string (will be freed in the callback).
  Future<T> _asyncCallString<T>(
    Pointer<Utf8> strC,
    void Function(
            Pointer<NativeFunction<native.AsyncCallbackNative>>, Pointer<Void>)
        ffiCall,
    T Function(Map<String, dynamic>?) parse,
  ) {
    final completer = Completer<T>();

    // Create the listener. IMPORTANT: we do NOT close it here.
    // We close it inside the callback after the completer is resolved,
    // ensuring the listener survives until the Rust callback fires.
    final nativeCallable =
        native.NativeCallable<native.AsyncCallbackNative>.listener(
      (Pointer<Utf8> resultJson, Pointer<Utf8> error, Pointer<Void> ud) {
        calloc.free(strC); // free the C string allocated by the caller
        _handleAsyncResult(completer, resultJson, error, parse);
        nativeCallable.close(); // safe to close now — callback has fired
      },
    );

    ffiCall(nativeCallable.nativeFunction, nullptr);
    return completer.future;
  }

  /// Handles the result of an async callback, resolving [completer].
  static void _handleAsyncResult<T>(
    Completer<T> completer,
    Pointer<Utf8> resultJson,
    Pointer<Utf8> error,
    T Function(Map<String, dynamic>?) parse,
  ) {
    if (completer.isCompleted) return;

    if (error != nullptr) {
      try {
        final msg = error.toDartString();
        completer.completeError(NaviException(msg));
      } catch (e) {
        completer.completeError(NaviException('Unknown error'));
      }
      return;
    }

    if (resultJson != nullptr) {
      try {
        final jsonStr = resultJson.toDartString();
        if (jsonStr == 'null' || jsonStr.isEmpty) {
          completer.complete(parse(null));
        } else {
          final parsed = tryParseJson(jsonStr);
          completer.complete(parse(parsed));
        }
      } catch (e) {
        completer.completeError(NaviException('Failed to parse result: $e'));
      }
    } else {
      completer.complete(parse(null));
    }
  }

  List<String> _parseStringList(Pointer<Utf8> ptr) {
    if (ptr == nullptr) return [];
    try {
      final jsonStr = ptr.toDartString();
      final list = tryParseJsonList(jsonStr);
      if (list == null) return [];
      return list.map((e) => e as String).toList();
    } finally {
      native.naviStringFree(ptr);
    }
  }

  List<dynamic> _parseJsonList(Pointer<Utf8> ptr) {
    if (ptr == nullptr) return [];
    try {
      final jsonStr = ptr.toDartString();
      return tryParseJsonList(jsonStr) ?? [];
    } finally {
      native.naviStringFree(ptr);
    }
  }

  T _parseJson<T>(Pointer<Utf8> ptr, T Function(Map<String, dynamic>) parse) {
    if (ptr == nullptr) throw NaviException(_lastError());
    try {
      final jsonStr = ptr.toDartString();
      final parsed = tryParseJson(jsonStr);
      if (parsed == null) throw NaviException('Failed to parse JSON');
      return parse(parsed);
    } finally {
      native.naviStringFree(ptr);
    }
  }
}

/// Returns the last error message from the native library.
String _lastError() {
  final ptr = native.naviLastError();
  if (ptr == nullptr) return 'Unknown error';
  try {
    return ptr.toDartString();
  } catch (_) {
    return 'Unknown error';
  }
}
