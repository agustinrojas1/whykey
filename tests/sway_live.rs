//! Optional live Sway IPC checks. These are ignored and require explicit opt-in.

use whykey::capture::CapturePolicy;
use whykey::sway_capture::capture_connect;

fn require_live_prerequisites() {
    assert_eq!(
        std::env::var("WHYKEY_RUN_LIVE_TESTS").as_deref(),
        Ok("1"),
        "live Sway tests require WHYKEY_RUN_LIVE_TESTS=1"
    );
    assert!(
        std::env::var_os("SWAYSOCK").is_some(),
        "live Sway tests require SWAYSOCK from a running compositor"
    );
}

#[test]
#[ignore = "requires an explicitly opted-in live Sway session"]
fn sway_binding_backend_is_pass_through_only() {
    require_live_prerequisites();
    let mut session = capture_connect().expect("Sway IPC must connect");
    assert!(session.arm(CapturePolicy::Suppress).is_err());
    session
        .arm(CapturePolicy::PassThrough)
        .expect("pass-through observation must arm");
    assert!(!session.is_suppressing());
    session.close().expect("Sway IPC must close");
}
