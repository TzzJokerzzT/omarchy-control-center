//! Integration tests for the headless surface.
//!
//! These run the real binary with a scratch XDG state directory, so nothing is
//! written to the developer's home and no system state is modified. The inventory
//! and plan exports are read-only by construction.

use std::process::Command;

fn binary() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_omarchy-control-center"));
    let scratch = std::env::temp_dir().join(format!("occ-it-{}", std::process::id()));
    command
        .env("XDG_CONFIG_HOME", scratch.join("config"))
        .env("XDG_STATE_HOME", scratch.join("state"));
    command
}

#[test]
fn print_inventory_emits_parseable_json_with_an_adapter_for_every_integration() {
    let output = binary().arg("--print-inventory").output().expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    let adapters = json["adapters"].as_array().expect("adapters array");
    let ids: Vec<&str> = adapters
        .iter()
        .filter_map(|adapter| adapter["id"].as_str())
        .collect();
    assert!(
        ids.contains(&"pacman"),
        "pacman must be registered: {ids:?}"
    );
    assert!(ids.contains(&"aur"), "aur must be registered: {ids:?}");
    assert!(
        ids.contains(&"flatpak"),
        "flatpak must be registered: {ids:?}"
    );

    // Every component carries the provenance the interface displays.
    for component in json["components"].as_array().expect("components array") {
        assert!(component["id"]["adapter"].is_string());
        assert!(component["id"]["native_id"].is_string());
        assert!(component["detection"]["program"].is_string());
        assert!(component["confidence"].is_string());
    }
}

#[test]
fn printing_a_plan_for_an_unknown_component_fails_without_touching_anything() {
    let output = binary()
        .args(["--print-plan", "occ-package-that-does-not-exist"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not installed"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "no plan may be printed for a missing component"
    );
}

#[test]
fn read_only_refuses_to_build_a_plan() {
    let output = binary()
        .args(["--read-only", "--print-plan", "bash"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("read-only"), "unexpected stderr: {stderr}");
}
