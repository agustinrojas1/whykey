//! Live Hyprland tests: mutate a dedicated compositor session.
//!
//! These tests never run in the ordinary suite. They require all three:
//! `--ignored`, `WHYKEY_RUN_LIVE_TESTS=1`, and an explicit test instance
//! signature (`WHYKEY_LIVE_INSTANCE_SIGNATURE` equal to
//! `HYPRLAND_INSTANCE_SIGNATURE`). Anything else fails with a prerequisite
//! message instead of passing silently. Run only in a dedicated Hyprland
//! session; they change the active submap and issue `hyprctl reload`.

use std::process::Command;
use std::time::Duration;

use whykey::hyprland_capture::{HyprlandCaptureSession, raw_current_submap};

fn require_live_prerequisites() -> String {
    assert_eq!(
        std::env::var("WHYKEY_RUN_LIVE_TESTS").as_deref(),
        Ok("1"),
        "live Hyprland tests require WHYKEY_RUN_LIVE_TESTS=1 in addition to \
         `cargo test --test hyprland_live -- --ignored`"
    );
    let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").expect(
        "live Hyprland tests require HYPRLAND_INSTANCE_SIGNATURE from a running compositor",
    );
    let expected = std::env::var("WHYKEY_LIVE_INSTANCE_SIGNATURE").expect(
        "live Hyprland tests require WHYKEY_LIVE_INSTANCE_SIGNATURE naming the \
         dedicated test session; refusing to mutate an unconfirmed compositor",
    );
    assert_eq!(
        expected, signature,
        "WHYKEY_LIVE_INSTANCE_SIGNATURE must equal HYPRLAND_INSTANCE_SIGNATURE; \
         refusing to mutate a compositor that is not the dedicated test session"
    );
    signature
}

fn submap_target_matches(current: &str, target: &str) -> bool {
    if target == "reset" || target == "default" {
        current.is_empty() || current == "default" || current == "reset"
    } else {
        current == target
    }
}
#[test]
#[ignore = "mutates a live Hyprland session; run explicitly with a dedicated compositor"]
fn restore_submap_verifies_table_result_and_res_false() {
    require_live_prerequisites();
    let before = raw_current_submap();
    assert_ne!(
        before, "__whykey_capture",
        "live test must start outside the private capture submap, found {before:?}"
    );
    // Open arms capture (installing the private submap) and records the
    // original submap; restore_submap exercises the owner-checked Lua route
    // that verifies table results, `res == false`, and the postcondition.
    let mut session =
        HyprlandCaptureSession::open().expect("live Hyprland capture session must open");
    let original = session.saved_submap().to_owned();
    assert_eq!(
        original, before,
        "session must record the pre-test submap exactly"
    );
    assert_eq!(raw_current_submap(), "__whykey_capture");
    session
        .restore_submap()
        .expect("restoring the recorded submap must succeed");
    assert!(
        submap_target_matches(&raw_current_submap(), &original),
        "submap must be restored from {original:?}, got {:?}",
        raw_current_submap()
    );
}

#[test]
#[ignore = "mutates a live Hyprland session; run explicitly with a dedicated compositor"]
fn live_config_reload_recovers_submap_cleanly() {
    require_live_prerequisites();
    let mut session =
        HyprlandCaptureSession::open().expect("live Hyprland capture session must open");
    let original_submap = session.saved_submap().to_owned();
    assert_eq!(raw_current_submap(), "__whykey_capture");

    let mut reload_cmd = Command::new("hyprctl");
    reload_cmd.arg("reload");
    let reload_res = whykey::command::output(&mut reload_cmd);
    let output = reload_res.expect("hyprctl reload must execute");
    assert!(output.status.success(), "hyprctl reload must succeed");

    std::thread::sleep(Duration::from_millis(200));

    session
        .close()
        .expect("capture cleanup must succeed after reload");

    assert!(
        submap_target_matches(&raw_current_submap(), &original_submap),
        "submap must be restored from {:?}, got {:?}",
        original_submap,
        raw_current_submap()
    );
}
