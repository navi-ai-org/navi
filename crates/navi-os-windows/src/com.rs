//! COM initialization for UI Automation (UIA).
//!
//! UIA is a COM-based API: every call into `IUIAutomation` requires the
//! current thread to live in a COM apartment. NAVI runs UIA from the tokio
//! runtime (worker threads), so we initialize the **multi-threaded apartment
//! (MTA)** once per process and never uninitialize it.
//!
//! Why never uninitialize: `CoUninitialize` can unload COM DLLs while other
//! threads still hold UIA interface pointers, causing use-after-free. The
//! Microsoft guidance for long-running MTA clients is to init once and let
//! the OS clean up at process exit. See microsoft/windows-rs#1169.
//!
//! `CoInitializeEx` on a thread that already lives in the MTA (the common
//! case after the first init) returns `S_FALSE` — we treat that as success.

use std::sync::Once;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::HRESULT;

static INIT: Once = Once::new();
static mut INIT_RESULT: HRESULT = HRESULT(0);

/// Ensures COM is initialized in the MTA for the current process.
///
/// Safe to call from any thread, any number of times. The first call performs
/// `CoInitializeEx(None, COINIT_MULTITHREADED)`; subsequent calls are no-ops
/// (the MTA is process-wide once established).
///
/// Returns `Ok(())` if COM is ready (either this call initialized it, a
/// previous call did, or the thread was already in the MTA). Returns an error
/// only if the first initialization failed with a real error (not `S_FALSE`).
pub fn ensure_com_initialized() -> anyhow::Result<()> {
    // Fast path: already initialized successfully on a previous call.
    // We still call CoInitializeEx on the current thread because COM
    // per-thread accounting requires it, but we ignore S_FALSE.
    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        // S_FALSE (1) means this thread was already initialized — fine.
        // S_OK (0) means we just initialized it — fine.
        // RPC_E_CHANGED_MODE (0x80010106) means the thread is in a different
        // apartment (STA). This shouldn't happen in tokio worker threads, but
        // if it does, the caller can't use UIA from here.
        if hr.is_ok() || hr == HRESULT(1) {
            // Record success on the first init so we know COM is up.
            INIT.call_once(|| {
                INIT_RESULT = hr;
            });
            Ok(())
        } else if hr.0 as u32 == 0x80010106 {
            // RPC_E_CHANGED_MODE — thread is STA; don't uninitialize (would
            // tear down the STA the caller set up).
            anyhow::bail!(
                "COM init failed: thread is in a different apartment (RPC_E_CHANGED_MODE). \
                 UIA requires MTA; the calling thread appears to be STA."
            )
        } else {
            anyhow::bail!("CoInitializeEx failed: HRESULT 0x{:08X}", hr.0 as u32)
        }
    }
}

/// Explicitly uninitializes COM on the current thread.
///
/// **Not called in normal operation** — we leak the MTA for process lifetime
/// to avoid DLL unload races. Exposed for tests that want a clean teardown.
#[allow(dead_code)]
pub unsafe fn uninitialize_com() {
    unsafe {
        CoUninitialize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn ensure_com_initialized_is_idempotent() {
        // Calling twice on the same thread must not error.
        ensure_com_initialized().expect("first init");
        ensure_com_initialized().expect("second init");
    }
}
