//! Compose control: the TUI must surrender the terminal when the supervisor
//! asks it to, so the next frontend generation can be activated.
//!
//! These tests drive the *real* event loop against a `TestBackend`, so they
//! prove the swap path without a TTY.

use std::time::Duration;

use navi_tui::LoopExit;
use navi_tui::host::{ControlChannel, ControlMsg};
use navi_tui::testing::{Harness, TestConfig};

#[test]
fn quiesce_request_exits_the_loop_with_quiesced() {
    let mut harness = Harness::new(TestConfig::default());
    let control = ControlChannel::new();

    let sender = control.clone();
    let quiescer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            sender.send(ControlMsg::Quiesce {
                reason: "runtime swap".to_string(),
            }),
            "first quiesce request must be accepted"
        );
    });

    let exit = harness.run_loop_until_control(&control);
    quiescer.join().expect("quiescer thread");
    assert_eq!(exit, LoopExit::Quiesced);
    // The session survives the frontend: it belongs to the engine.
    assert_eq!(harness.session_id(), "session-test");
}

#[test]
fn shutdown_control_exits_the_loop_with_shutdown() {
    let mut harness = Harness::new(TestConfig::default());
    let control = ControlChannel::new();
    let sender = control.clone();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        sender.send(ControlMsg::Shutdown);
    });

    let exit = harness.run_loop_until_control(&control);
    stopper.join().expect("stopper thread");
    assert_eq!(exit, LoopExit::Shutdown);
}

#[test]
fn repeated_quiesce_requests_are_idempotent() {
    let control = ControlChannel::new();
    assert!(control.send(ControlMsg::Quiesce {
        reason: "first".into()
    }));
    assert!(
        !control.send(ControlMsg::Quiesce {
            reason: "second".into()
        }),
        "a second quiesce is a no-op (I06)"
    );
    assert!(control.quiesce_requested());
    assert!(matches!(control.poll(), Some(ControlMsg::Quiesce { .. })));
    assert!(control.poll().is_none());
}
