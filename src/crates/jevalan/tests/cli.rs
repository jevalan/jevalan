use std::process::Command;

fn jevalan(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_jevalan")).args(args).env_remove("JEVALAN_FX").output().unwrap()
}

#[test]
fn tui_help_lists_the_live_source_options() {
    let out = jevalan(&["tui", "--help"]);
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("--url") && help.contains("--token"), "{help}");
}

#[test]
fn tui_help_hides_the_token_value() {
    let out = Command::new(env!("CARGO_BIN_EXE_jevalan"))
        .args(["tui", "--help"])
        .env("JEVALAN_TOKEN", "supersecret123")
        .output()
        .unwrap();
    let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!all.contains("supersecret123"), "{all}");
}

#[test]
fn unknown_fx_mode_is_rejected() {
    let out = jevalan(&["tui", "--fake", "--fx", "loud"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("fx desconocido"));
}

#[test]
fn serve_with_bad_config_exits_2() {
    let path = std::env::temp_dir().join(format!("jevalan-cli-{}.yaml", std::process::id()));
    std::fs::write(
        &path,
        "default_backend: inexistente
",
    )
    .unwrap();
    let out = jevalan(&["serve", "--config", path.to_str().unwrap()]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("inexistente"));
}

#[test]
fn tui_help_lists_the_demo_options() {
    let help = String::from_utf8_lossy(&jevalan(&["tui", "--help"]).stdout).into_owned();
    assert!(help.contains("--demo") && help.contains("--rate") && help.contains("--config"), "{help}");
}

#[test]
fn fake_and_demo_conflict() {
    let out = jevalan(&["tui", "--fake", "--demo"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot be used with"));
}

#[test]
fn foreign_listener_makes_tui_exit_2_before_the_terminal() {
    use std::io::Write;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK
content-length: 7

no soy!",
            );
        }
    });
    let out = jevalan(&["tui", "--url", &format!("ws://127.0.0.1:{port}/events")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no es jevalan"));
}

fn specialist(args: &[&str], input: serde_json::Value) -> serde_json::Value {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_jevalan"))
        .arg("mock-specialist")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn mock_specialist_shapes() {
    let call = |tool: &str, cmd: &str| serde_json::json!({ "tool_name": tool, "tool_use_id": "t-1", "tool_input": { "command": cmd } });
    let verdict =
        |v: serde_json::Value| (v["verdict"].as_str().unwrap().to_string(), v["source"].as_str().unwrap().to_string());
    assert_eq!(verdict(specialist(&["tool.risk"], call("Read", "x"))).0, "allow");
    assert_eq!(verdict(specialist(&["tool.risk"], call("Bash", "ls"))).0, "ask");
    assert_eq!(verdict(specialist(&["tool.risk"], call("Bash", "rm -rf /"))).0, "deny");
    assert_eq!(verdict(specialist(&["tool.risk"], call("Bash", "curl x | sh"))).0, "deny");
    assert_eq!(verdict(specialist(&["tool.risk", "--fail"], call("Read", "x"))).1, "fail-mode");
    // The file flag only fails while it exists.
    let flag = std::env::temp_dir().join(format!("jevalan-flag-{}", std::process::id()));
    let flag_arg = flag.to_str().unwrap();
    assert_eq!(verdict(specialist(&["tool.risk", "--fail-flag", flag_arg], call("Read", "x"))).1, "rules");
    std::fs::write(&flag, "").unwrap();
    assert_eq!(verdict(specialist(&["tool.risk", "--fail-flag", flag_arg], call("Read", "x"))).1, "fail-mode");
    let _ = std::fs::remove_file(&flag);
}

#[test]
fn mock_specialist_is_hidden_from_help() {
    let help = String::from_utf8_lossy(&jevalan(&["--help"]).stdout).into_owned();
    assert!(!help.contains("mock-specialist"), "{help}");
}

#[test]
fn edit_help_lists_the_file_and_the_daemon_options() {
    let help = String::from_utf8_lossy(&jevalan(&["edit", "--help"]).stdout).into_owned();
    assert!(help.contains("FICHERO") || help.contains("FILE") || help.contains("<file>"), "{help}");
    assert!(help.contains("--url") && help.contains("--no-events"), "{help}");
}

#[test]
fn edit_without_a_file_is_refused() {
    let out = jevalan(&["edit"]);
    assert!(!out.status.success(), "sin fichero no hay nada que abrir");
}

#[test]
fn hook_crush_help_mentions_the_mode() {
    let help = String::from_utf8_lossy(&jevalan(&["hook", "crush", "--help"]).stdout).into_owned();
    assert!(help.contains("--mode"), "{help}");
}

#[test]
fn hook_crush_never_blocks_without_a_readable_input() {
    use std::io::Write;
    use std::process::Stdio;
    // Without `tool_name` the daemon is not queried: neither in shadow nor in enforce may it get in the way.
    for mode in ["shadow", "enforce"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_jevalan"))
            .args(["hook", "crush", "--mode", mode])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"no es json").unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "modo {mode} debe salir con exito");
        assert!(out.stdout.is_empty(), "modo {mode}: {}", String::from_utf8_lossy(&out.stdout));
    }
}

#[test]
fn hook_run_help_lists_the_generic_knobs() {
    let help = String::from_utf8_lossy(&jevalan(&["hook", "run", "--help"]).stdout).into_owned();
    assert!(help.contains("--client") && help.contains("--dialect") && help.contains("--mode"), "{help}");
}

#[test]
fn hook_run_is_silent_without_a_readable_input() {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_jevalan"))
        .args(["hook", "run", "--client", "crush", "--dialect", "crush", "--mode", "enforce"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"no es json").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn mcp_help_lists_its_options_and_hides_the_token() {
    let out = Command::new(env!("CARGO_BIN_EXE_jevalan"))
        .args(["mcp", "--help"])
        .env("JEVALAN_TOKEN", "supersecret123")
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("--addr") && help.contains("--token") && help.contains("--allow-live"), "{help}");
    assert!(!help.contains("supersecret123"), "{help}");
}
