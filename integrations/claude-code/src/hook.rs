use serde_json::Value;

use crate::classify::{classify, Decision};
use crate::config::Config;
use crate::rewrite::{hook_output, rewritten_command};

pub fn process_hook(input: &str, exe: &str, config: &Config) -> Option<Value> {
    let parsed: Value = serde_json::from_str(input).ok()?;
    let command = parsed.get("tool_input")?.get("command")?.as_str()?;
    match classify(command, config) {
        Decision::Passthrough => None,
        Decision::Sandbox(runtime) => {
            let rewritten = rewritten_command(exe, runtime.key, command);
            Some(hook_output(&rewritten, config.auto_allow))
        }
    }
}

pub fn payload_cwd(input: &str) -> Option<String> {
    serde_json::from_str::<Value>(input)
        .ok()?
        .get("cwd")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rewrite::{decode_command, encode_command};

    fn input_for(command: &str) -> String {
        serde_json::json!({
            "tool_name": "Bash",
            "cwd": "/proj",
            "tool_input": { "command": command }
        })
        .to_string()
    }

    #[test]
    fn sandbox_worthy_command_is_rewritten() {
        let out = process_hook(
            &input_for("python app.py"),
            "/bin/lns-cc",
            &Config::default(),
        )
        .unwrap();
        let rewritten = out["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .unwrap();
        assert!(rewritten.starts_with("'/bin/lns-cc' exec python --b64 "));
        let b64 = rewritten.rsplit(' ').next().unwrap();
        assert_eq!(decode_command(b64).as_deref(), Some("python app.py"));
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "allow");
    }

    #[test]
    fn passthrough_command_returns_none() {
        assert!(process_hook(&input_for("ls -la"), "/bin/lns-cc", &Config::default()).is_none());
    }

    #[test]
    fn invalid_json_returns_none() {
        assert!(process_hook("{not json", "/bin/lns-cc", &Config::default()).is_none());
    }

    #[test]
    fn missing_command_field_returns_none() {
        let input = serde_json::json!({ "tool_input": {} }).to_string();
        assert!(process_hook(&input, "/bin/lns-cc", &Config::default()).is_none());
    }

    #[test]
    fn non_string_command_returns_none() {
        let input = serde_json::json!({ "tool_input": { "command": 42 } }).to_string();
        assert!(process_hook(&input, "/bin/lns-cc", &Config::default()).is_none());
    }

    #[test]
    fn auto_allow_disabled_omits_permission_decision() {
        let config = Config {
            auto_allow: false,
            ..Config::default()
        };
        let out = process_hook(&input_for("node x.js"), "/bin/lns-cc", &config).unwrap();
        assert!(out["hookSpecificOutput"]
            .get("permissionDecision")
            .is_none());
    }

    #[test]
    fn encode_helper_matches_embedded_payload() {
        let out = process_hook(
            &input_for("npx cowsay hi"),
            "/bin/lns-cc",
            &Config::default(),
        )
        .unwrap();
        let rewritten = out["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .unwrap();
        assert!(rewritten.contains(&encode_command("npx cowsay hi")));
    }

    #[test]
    fn payload_cwd_extracts_the_field() {
        assert_eq!(payload_cwd(&input_for("ls")).as_deref(), Some("/proj"));
        assert_eq!(payload_cwd("{}"), None);
        assert_eq!(payload_cwd("not json"), None);
    }
}
