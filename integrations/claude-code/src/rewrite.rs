use base64::Engine;
use serde_json::{json, Value};

const REASON: &str =
    "Routed into a Lens Sandbox microVM; network and credential access is gated by lns policy.";

pub fn encode_command(original: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(original.as_bytes())
}

pub fn decode_command(encoded: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.as_bytes())
        .ok()?;
    String::from_utf8(bytes).ok()
}

pub fn rewritten_command(exe: &str, runtime_key: &str, original: &str) -> String {
    format!(
        "{} exec {} --b64 {}",
        single_quote(exe),
        runtime_key,
        encode_command(original)
    )
}

pub fn hook_output(rewritten: &str, auto_allow: bool) -> Value {
    let mut out = json!({
        "hookEventName": "PreToolUse",
        "updatedInput": { "command": rewritten },
        "permissionDecisionReason": REASON,
    });
    if auto_allow {
        out["permissionDecision"] = json!("allow");
    }
    json!({ "hookSpecificOutput": out })
}

fn single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_including_quotes_and_newlines() {
        let original = "python -c \"print('a')\" && ls\n# done";
        let encoded = encode_command(original);
        assert!(!encoded.contains(' '));
        assert_eq!(decode_command(&encoded).as_deref(), Some(original));
    }

    #[test]
    fn decode_rejects_invalid_base64() {
        assert_eq!(decode_command("not valid base64!!!"), None);
    }

    #[test]
    fn decode_rejects_non_utf8() {
        let encoded = base64::engine::general_purpose::STANDARD.encode([0xff, 0xfe]);
        assert_eq!(decode_command(&encoded), None);
    }

    #[test]
    fn rewritten_command_quotes_exe_and_encodes_original() {
        let cmd = rewritten_command("/plugin/bin/lns-cc", "python", "python app.py");
        let expected_b64 = encode_command("python app.py");
        assert_eq!(
            cmd,
            format!("'/plugin/bin/lns-cc' exec python --b64 {expected_b64}")
        );
    }

    #[test]
    fn rewritten_command_escapes_single_quotes_in_path() {
        let cmd = rewritten_command("/o'brien/lns-cc", "node", "node x.js");
        assert!(cmd.starts_with("'/o'\\''brien/lns-cc' exec node --b64 "));
    }

    #[test]
    fn hook_output_with_auto_allow_sets_permission_decision() {
        let out = hook_output("lns-cc exec python --b64 QQ==", true);
        let inner = &out["hookSpecificOutput"];
        assert_eq!(inner["hookEventName"], "PreToolUse");
        assert_eq!(
            inner["updatedInput"]["command"],
            "lns-cc exec python --b64 QQ=="
        );
        assert_eq!(inner["permissionDecision"], "allow");
        assert_eq!(inner["permissionDecisionReason"], REASON);
    }

    #[test]
    fn hook_output_without_auto_allow_omits_permission_decision() {
        let out = hook_output("lns-cc exec python --b64 QQ==", false);
        assert!(out["hookSpecificOutput"]
            .get("permissionDecision")
            .is_none());
        assert_eq!(
            out["hookSpecificOutput"]["updatedInput"]["command"],
            "lns-cc exec python --b64 QQ=="
        );
    }
}
