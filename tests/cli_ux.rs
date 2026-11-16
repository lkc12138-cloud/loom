use serde_json::json;
use std::{
    fs,
    process::{Command, Stdio},
};

#[test]
fn bare_loom_command_prints_friendly_summary_when_non_interactive() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("data");
    let r = Command::new(env!("CARGO_BIN_EXE_loom"))
        .args(["--data-dir", d.to_str().unwrap()])
        .env_clear()
        .env("HOME", t.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let out = String::from_utf8_lossy(&r.stdout);
    assert!(out.contains("Loom 本机统一模型网关"));
    assert!(out.contains("loom doctor --ping"));
    assert!(out.contains("loom console"));
    let key = fs::read_to_string(d.join("hub.key")).unwrap();
    assert!(!out.contains(key.trim()));
}

#[test]
fn doctor_offline_diagnostics_report_status_without_leaking_secrets() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("data");
    fs::create_dir_all(d.join("keys")).unwrap();
    let kf = d.join("keys/local-deepseek.key");
    fs::write(&kf, "DOCTOR_SECRET_FIXTURE").unwrap();
    fs::write(d.join("hub.key"), "HUB_SECRET_FIXTURE\n").unwrap();
    fs::write(
        d.join("providers.json"),
        serde_json::to_vec(&json!({
            "proxy": "http://127.0.0.1:17890",
            "providers": {
                "local-deepseek": {
                    "prefix": "deepseek",
                    "name": "DeepSeek · API 钱包",
                    "baseURL": "https://api.deepseek.com",
                    "enabled": true,
                    "keyFile": kf,
                    "models": [{"id": "deepseek-flash", "name": "DeepSeek Flash"}]
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let r = Command::new(env!("CARGO_BIN_EXE_loom"))
        .args([
            "--data-dir",
            d.to_str().unwrap(),
            "doctor",
            "--port",
            "14399",
            "--admin-port",
            "14400",
        ])
        .env_clear()
        .env("HOME", t.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let out = String::from_utf8_lossy(&r.stdout);
    assert!(out.contains("[1/4]") && out.contains("[4/4]"));
    assert!(out.contains("DeepSeek"));
    assert!(!out.contains("DOCTOR_SECRET_FIXTURE"));
    assert!(!out.contains("HUB_SECRET_FIXTURE"));
    assert!(!out.contains("17890"));
}

#[test]
fn new_cli_subcommands_expose_expected_flags() {
    for (args, expected) in [
        (vec!["console", "--help"], "--page"),
        (vec!["doctor", "--help"], "--ping"),
        (vec!["app", "--help"], "--build-only"),
    ] {
        let r = Command::new(env!("CARGO_BIN_EXE_loom"))
            .args(&args)
            .output()
            .unwrap();
        assert!(r.status.success());
        assert!(
            String::from_utf8_lossy(&r.stdout).contains(expected),
            "missing {expected} in {:?}",
            args
        );
    }
}
