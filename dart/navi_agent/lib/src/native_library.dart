// FFI function signatures for the navi-dart native library.
//
// This file defines the low-level C function bindings that Dart calls
// through dart:ffi. It is not part of the public API.

import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

// ── Type aliases ───────────────────────────────────────────────────

/// Opaque pointer to the NaviDartEngine Rust struct.
typedef NaviDartEngine = Opaque;

/// Opaque pointer to the NaviEventSubscription Rust struct.
typedef NaviEventSubscription = Opaque;

/// Async result callback: (result_json, error, user_data) → void.
typedef AsyncCallbackNative = Void Function(
    Pointer<Utf8> resultJson, Pointer<Utf8> error, Pointer<Void> userData);
typedef AsyncCallbackDart = void Function(
    Pointer<Utf8> resultJson, Pointer<Utf8> error, Pointer<Void> userData);

/// Event callback: (event_json, user_data) → i32 (0=continue, non-zero=stop).
typedef EventCallbackNative = Int32 Function(
    Pointer<Utf8> eventJson, Pointer<Void> userData);
typedef EventCallbackDart = int Function(
    Pointer<Utf8> eventJson, Pointer<Void> userData);

// ── Library loading ────────────────────────────────────────────────

DynamicLibrary _openNaviLib() {
  if (Platform.isLinux) {
    return DynamicLibrary.open('libnavi_dart.so');
  } else if (Platform.isMacOS) {
    return DynamicLibrary.open('libnavi_dart.dylib');
  } else if (Platform.isWindows) {
    return DynamicLibrary.open('navi_dart.dll');
  }
  throw UnsupportedError('Unsupported platform: ${Platform.operatingSystem}');
}

/// The loaded native library.
final DynamicLibrary naviLib = _openNaviLib();

// ── Error & string management ──────────────────────────────────────

// char* navi_last_error()
typedef NaviLastErrorNative = Pointer<Utf8> Function();
typedef NaviLastErrorDart = Pointer<Utf8> Function();
final NaviLastErrorDart naviLastError = naviLib
    .lookupFunction<NaviLastErrorNative, NaviLastErrorDart>('navi_last_error');

// void navi_string_free(char*)
typedef NaviStringFreeNative = Void Function(Pointer<Utf8>);
typedef NaviStringFreeDart = void Function(Pointer<Utf8>);
final NaviStringFreeDart naviStringFree = naviLib
    .lookupFunction<NaviStringFreeNative, NaviStringFreeDart>(
        'navi_string_free');

// ── Engine lifecycle ───────────────────────────────────────────────

// NaviDartEngine* navi_engine_new(const char*)
typedef NaviEngineNewNative = Pointer<NaviDartEngine> Function(Pointer<Utf8>);
typedef NaviEngineNewDart = Pointer<NaviDartEngine> Function(Pointer<Utf8>);
final NaviEngineNewDart naviEngineNew = naviLib
    .lookupFunction<NaviEngineNewNative, NaviEngineNewDart>(
        'navi_engine_new');

// void navi_engine_free(NaviDartEngine*)
typedef NaviEngineFreeNative = Void Function(Pointer<NaviDartEngine>);
typedef NaviEngineFreeDart = void Function(Pointer<NaviDartEngine>);
final NaviEngineFreeDart naviEngineFree = naviLib
    .lookupFunction<NaviEngineFreeNative, NaviEngineFreeDart>(
        'navi_engine_free');

// ── Sessions ───────────────────────────────────────────────────────

// void navi_engine_start_session(engine, request_json, callback, user_data)
typedef NaviEngineStartSessionNative = Void Function(
    Pointer<NaviDartEngine>,
    Pointer<Utf8>,
    Pointer<NativeFunction<AsyncCallbackNative>>,
    Pointer<Void>);
typedef NaviEngineStartSessionDart = void Function(
    Pointer<NaviDartEngine>,
    Pointer<Utf8>,
    Pointer<NativeFunction<AsyncCallbackNative>>,
    Pointer<Void>);
final NaviEngineStartSessionDart naviEngineStartSession = naviLib
    .lookupFunction<NaviEngineStartSessionNative, NaviEngineStartSessionDart>(
        'navi_engine_start_session');

// void navi_engine_close_session(engine, session_id, callback, user_data)
typedef NaviEngineCloseSessionNative = Void Function(
    Pointer<NaviDartEngine>,
    Pointer<Utf8>,
    Pointer<NativeFunction<AsyncCallbackNative>>,
    Pointer<Void>);
typedef NaviEngineCloseSessionDart = void Function(
    Pointer<NaviDartEngine>,
    Pointer<Utf8>,
    Pointer<NativeFunction<AsyncCallbackNative>>,
    Pointer<Void>);
final NaviEngineCloseSessionDart naviEngineCloseSession = naviLib
    .lookupFunction<NaviEngineCloseSessionNative, NaviEngineCloseSessionDart>(
        'navi_engine_close_session');

// char* navi_engine_session_ids(engine)
typedef NaviEngineSessionIdsNative = Pointer<Utf8> Function(
    Pointer<NaviDartEngine>);
typedef NaviEngineSessionIdsDart = Pointer<Utf8> Function(
    Pointer<NaviDartEngine>);
final NaviEngineSessionIdsDart naviEngineSessionIds = naviLib
    .lookupFunction<NaviEngineSessionIdsNative, NaviEngineSessionIdsDart>(
        'navi_engine_session_ids');

// ── Turns ──────────────────────────────────────────────────────────

// void navi_engine_send_turn(engine, request_json, callback, user_data)
typedef NaviEngineSendTurnNative = Void Function(
    Pointer<NaviDartEngine>,
    Pointer<Utf8>,
    Pointer<NativeFunction<AsyncCallbackNative>>,
    Pointer<Void>);
typedef NaviEngineSendTurnDart = void Function(
    Pointer<NaviDartEngine>,
    Pointer<Utf8>,
    Pointer<NativeFunction<AsyncCallbackNative>>,
    Pointer<Void>);
final NaviEngineSendTurnDart naviEngineSendTurn = naviLib
    .lookupFunction<NaviEngineSendTurnNative, NaviEngineSendTurnDart>(
        'navi_engine_send_turn');

// void navi_engine_cancel_turn(engine, session_id, callback, user_data)
typedef NaviEngineCancelTurnNative = Void Function(
    Pointer<NaviDartEngine>,
    Pointer<Utf8>,
    Pointer<NativeFunction<AsyncCallbackNative>>,
    Pointer<Void>);
typedef NaviEngineCancelTurnDart = void Function(
    Pointer<NaviDartEngine>,
    Pointer<Utf8>,
    Pointer<NativeFunction<AsyncCallbackNative>>,
    Pointer<Void>);
final NaviEngineCancelTurnDart naviEngineCancelTurn = naviLib
    .lookupFunction<NaviEngineCancelTurnNative, NaviEngineCancelTurnDart>(
        'navi_engine_cancel_turn');

// ── Models ─────────────────────────────────────────────────────────

// char* navi_engine_list_models(engine)
typedef NaviEngineListModelsNative = Pointer<Utf8> Function(
    Pointer<NaviDartEngine>);
typedef NaviEngineListModelsDart = Pointer<Utf8> Function(
    Pointer<NaviDartEngine>);
final NaviEngineListModelsDart naviEngineListModels = naviLib
    .lookupFunction<NaviEngineListModelsNative, NaviEngineListModelsDart>(
        'navi_engine_list_models');

// void navi_engine_set_model(engine, session_id, provider, model, cb, ud)
typedef NaviEngineSetModelNative = Void Function(
    Pointer<NaviDartEngine>,
    Pointer<Utf8>,
    Pointer<Utf8>,
    Pointer<Utf8>,
    Pointer<NativeFunction<AsyncCallbackNative>>,
    Pointer<Void>);
typedef NaviEngineSetModelDart = void Function(
    Pointer<NaviDartEngine>,
    Pointer<Utf8>,
    Pointer<Utf8>,
    Pointer<Utf8>,
    Pointer<NativeFunction<AsyncCallbackNative>>,
    Pointer<Void>);
final NaviEngineSetModelDart naviEngineSetModel = naviLib
    .lookupFunction<NaviEngineSetModelNative, NaviEngineSetModelDart>(
        'navi_engine_set_model');

// ── Events ─────────────────────────────────────────────────────────

// NaviEventSubscription* navi_engine_subscribe_events(engine, sid, cb, ud)
typedef NaviEngineSubscribeEventsNative = Pointer<NaviEventSubscription>
    Function(
        Pointer<NaviDartEngine>,
        Pointer<Utf8>,
        Pointer<NativeFunction<EventCallbackNative>>,
        Pointer<Void>);
typedef NaviEngineSubscribeEventsDart = Pointer<NaviEventSubscription> Function(
    Pointer<NaviDartEngine>,
    Pointer<Utf8>,
    Pointer<NativeFunction<EventCallbackNative>>,
    Pointer<Void>);
final NaviEngineSubscribeEventsDart naviEngineSubscribeEvents = naviLib
    .lookupFunction<NaviEngineSubscribeEventsNative,
        NaviEngineSubscribeEventsDart>('navi_engine_subscribe_events');

// void navi_event_subscription_free(sub)
typedef NaviEventSubscriptionFreeNative = Void Function(
    Pointer<NaviEventSubscription>);
typedef NaviEventSubscriptionFreeDart = void Function(
    Pointer<NaviEventSubscription>);
final NaviEventSubscriptionFreeDart naviEventSubscriptionFree = naviLib
    .lookupFunction<NaviEventSubscriptionFreeNative,
        NaviEventSubscriptionFreeDart>('navi_event_subscription_free');

// ── Config ─────────────────────────────────────────────────────────

// char* navi_engine_loaded_config(engine)
typedef NaviEngineLoadedConfigNative = Pointer<Utf8> Function(
    Pointer<NaviDartEngine>);
typedef NaviEngineLoadedConfigDart = Pointer<Utf8> Function(
    Pointer<NaviDartEngine>);
final NaviEngineLoadedConfigDart naviEngineLoadedConfig = naviLib
    .lookupFunction<NaviEngineLoadedConfigNative, NaviEngineLoadedConfigDart>(
        'navi_engine_loaded_config');
