use std::net::IpAddr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    Ping,
    Status,
    Shutdown,
    Unknown { method: String },
    RunImage(RunImageArgs),
    CancelRun { run_id: u32 },
    RunStdin { run_id: u32, bytes: Vec<u8> },
    RunResize { run_id: u32, rows: u16, cols: u16 },
    RunSignal { run_id: u32, signal: SignalKind },
    ExecImage(ExecImageArgs),
    Kill { run_id: u32, signal: SignalKind },
    ListRuns,
    StopRun { run_id: u32, timeout_secs: u64 },
    BeginIntegrationSignIn { id: String },
    ListVolumes,
    CreateVolume { name: String },
    InspectVolume { name: String },
    RemoveVolume { name: String },
    PruneVolumes,
    PullImage { image: String },
    ListImages,
    RemoveImage { image: String },
    PruneImages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignalKind {
    Int,
    Term,
    Quit,
    Hup,
    Winch,
    Kill,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Response {
    Pong,
    Status(StatusInfo),
    ShuttingDown,
    Error {
        message: String,
    },
    RunStarted {
        run_id: u32,
    },
    RunLog {
        level: LogLevel,
        verb: Option<String>,
        message: String,
    },
    RunExit {
        code: i32,
    },
    CancelAccepted,
    Acknowledged,
    RunList {
        runs: Vec<RunSummary>,
    },
    RunStopped {
        forced: bool,
    },
    OauthVerification {
        verification_uri: String,
        user_code: String,
        expires_in_secs: u64,
    },
    OauthSignInComplete,
    OauthSignInFailed {
        reason: String,
    },
    VolumeList {
        volumes: Vec<VolumeInfo>,
    },
    VolumeCreated {
        volume: VolumeInfo,
    },
    VolumeInspect {
        volume: VolumeInfo,
    },
    VolumeRemoved {
        name: String,
    },
    VolumesPruned {
        removed: Vec<String>,
        reclaimed_bytes: u64,
        failed: Vec<VolumePruneFailure>,
    },
    ImagePulled {
        image: ImageInfo,
    },
    ImageList {
        images: Vec<ImageInfo>,
    },
    ImageRemoved {
        reference: String,
        reclaimed_bytes: u64,
    },
    ImagesPruned {
        removed: Vec<String>,
        reclaimed_bytes: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub name: String,
    pub size_bytes: u64,
    pub disk_bytes: u64,
    pub created: String,
    pub in_use_by: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumePruneFailure {
    pub name: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageInfo {
    pub reference: String,
    pub digest: String,
    pub size_bytes: u64,
    pub layers: u32,
    pub pulled: String,
    pub in_use_by: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: u32,
    pub image: String,
    pub command: String,
    pub status: RunStatus,
    pub started: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum RunStatus {
    Running,
    Exited { code: i32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusInfo {
    pub pid: u32,
    pub uptime_secs: u64,
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortPublish {
    pub host_ip: IpAddr,
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: Protocol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunImageArgs {
    pub image: Option<String>,
    pub cpus: u8,
    pub mem: usize,
    pub policy_path: Option<String>,
    #[serde(default)]
    pub sandbox_user: Option<String>,
    #[serde(default)]
    pub sandbox_uid: Option<u32>,
    pub cmd: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub workdir: Option<String>,
    pub debug: bool,
    #[serde(default = "default_tty")]
    pub tty: bool,
    #[serde(default = "default_stdin")]
    pub stdin: bool,
    #[serde(default)]
    pub initial_winsize: Option<(u16, u16)>,
    #[serde(default)]
    pub detached: bool,
    #[serde(default)]
    pub published_ports: Vec<PortPublish>,
    #[serde(default)]
    pub volumes: Vec<VolumeMount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeMount {
    pub name: String,
    pub target: String,
    pub read_only: bool,
}

impl VolumeMount {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let read_only = spec.ends_with(":ro");
        let body = spec
            .strip_suffix(":ro")
            .or_else(|| spec.strip_suffix(":rw"))
            .unwrap_or(spec);
        let (name, target) = body
            .split_once(':')
            .ok_or_else(|| format!("invalid volume {spec:?}: expected name:/path[:ro]"))?;
        validate_volume_name(name)?;
        validate_volume_target(target)?;
        Ok(Self {
            name: name.to_string(),
            target: target.to_string(),
            read_only,
        })
    }
}

pub fn validate_volume_target(target: &str) -> Result<(), String> {
    if !target.starts_with('/') {
        return Err(format!(
            "invalid volume target {target:?}: must be an absolute path"
        ));
    }
    if target.chars().any(target_char_forbidden) {
        return Err(format!(
            "invalid volume target {target:?}: must not contain whitespace, quotes, or control characters"
        ));
    }
    if target.split('/').any(|seg| seg == "." || seg == "..") {
        return Err(format!(
            "invalid volume target {target:?}: must not contain `.` or `..` path segments"
        ));
    }
    Ok(())
}

fn target_char_forbidden(c: char) -> bool {
    c.is_whitespace() || c.is_control() || c == '"'
}

pub fn validate_volume_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("invalid volume name: must not be empty".to_string());
    }
    if name == "." || name == ".." {
        return Err(format!("invalid volume name {name:?}: reserved"));
    }
    match name.chars().find(|c| !name_char_allowed(*c)) {
        Some(bad) => Err(format!(
            "invalid volume name {name:?}: character {bad:?} not allowed"
        )),
        None => Ok(()),
    }
}

fn name_char_allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

fn default_tty() -> bool {
    true
}

fn default_stdin() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecImageArgs {
    pub run_id: u32,
    pub argv: Vec<String>,
    pub env: Vec<String>,
    #[serde(default = "default_tty")]
    pub tty: bool,
    #[serde(default = "default_stdin")]
    pub stdin: bool,
    #[serde(default)]
    pub initial_winsize: Option<(u16, u16)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_image_args_tty_defaults_to_true_when_missing() {
        let frame = serde_json::json!({
            "run_id": 1,
            "argv": ["sh"],
            "env": []
        });
        let parsed: ExecImageArgs = serde_json::from_value(frame).unwrap();
        assert!(parsed.tty);
        assert!(parsed.stdin);
    }

    #[test]
    fn run_image_args_env_defaults_to_empty_when_missing() {
        let frame = serde_json::json!({
            "image": "alpine",
            "cpus": 1,
            "mem": 512,
            "policy_path": null,
            "sandbox_user": "sandbox",
            "sandbox_uid": 65534,
            "cmd": [],
            "debug": false
        });
        let parsed: RunImageArgs = serde_json::from_value(frame).unwrap();
        assert!(parsed.env.is_empty());
    }

    #[test]
    fn run_image_args_published_ports_default_empty_when_missing() {
        let frame = serde_json::json!({
            "image": "prism",
            "cpus": 1,
            "mem": 512,
            "policy_path": null,
            "sandbox_user": "sandbox",
            "sandbox_uid": 65534,
            "cmd": [],
            "debug": false
        });
        let parsed: RunImageArgs = serde_json::from_value(frame).unwrap();
        assert!(parsed.published_ports.is_empty());
    }

    #[test]
    fn run_image_args_volumes_default_to_empty_when_missing() {
        let frame = serde_json::json!({
            "image": "ubuntu",
            "cpus": 1,
            "mem": 512,
            "policy_path": null,
            "sandbox_user": "sandbox",
            "sandbox_uid": 65534,
            "cmd": [],
            "debug": false
        });
        let parsed: RunImageArgs = serde_json::from_value(frame).unwrap();
        assert!(parsed.volumes.is_empty());
    }

    #[test]
    fn port_publish_survives_a_request_round_trip() {
        let mapping = PortPublish {
            host_ip: "127.0.0.1".parse().unwrap(),
            host_port: 3003,
            container_port: 3003,
            protocol: Protocol::Tcp,
        };
        let req = Request::RunImage(RunImageArgs {
            image: Some("prism".into()),
            cpus: 1,
            mem: 512,
            policy_path: None,
            sandbox_user: Some("sandbox".into()),
            sandbox_uid: Some(65534),
            cmd: Vec::new(),
            env: Vec::new(),
            workdir: None,
            debug: false,
            tty: true,
            stdin: true,
            initial_winsize: None,
            detached: false,
            published_ports: vec![mapping],
            volumes: Vec::new(),
        });
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn run_image_args_volumes_survive_postcard_round_trip() {
        let args = RunImageArgs {
            image: Some("ubuntu".into()),
            cpus: 1,
            mem: 512,
            policy_path: None,
            sandbox_user: None,
            sandbox_uid: None,
            cmd: vec![],
            env: vec![],
            workdir: None,
            debug: false,
            tty: true,
            stdin: true,
            initial_winsize: None,
            detached: false,
            published_ports: vec![],
            volumes: vec![VolumeMount {
                name: "prism-data".into(),
                target: "/data".into(),
                read_only: true,
            }],
        };
        let frame = crate::encode_frame(&args).unwrap();
        let decoded: RunImageArgs = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, args);
    }

    #[test]
    fn volume_requests_survive_request_round_trips() {
        for req in [
            Request::ListVolumes,
            Request::CreateVolume {
                name: "prism-data".into(),
            },
            Request::InspectVolume {
                name: "prism-data".into(),
            },
            Request::RemoveVolume {
                name: "prism-data".into(),
            },
            Request::PruneVolumes,
        ] {
            let frame = crate::encode_frame(&req).unwrap();
            let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
            assert_eq!(decoded, req);
        }
    }

    #[test]
    fn volume_responses_survive_round_trips() {
        let info = VolumeInfo {
            name: "prism-data".into(),
            size_bytes: 10 * 1024 * 1024 * 1024,
            disk_bytes: 32 * 1024 * 1024,
            created: "2026-06-10T12:00:00Z".into(),
            in_use_by: Some(7),
        };
        for resp in [
            Response::VolumeList {
                volumes: vec![info.clone()],
            },
            Response::VolumeCreated {
                volume: info.clone(),
            },
            Response::VolumeInspect {
                volume: info.clone(),
            },
            Response::VolumeRemoved {
                name: "prism-data".into(),
            },
            Response::VolumesPruned {
                removed: vec!["prism-data".into()],
                reclaimed_bytes: 32 * 1024 * 1024,
                failed: vec![VolumePruneFailure {
                    name: "scratch".into(),
                    error: "permission denied".into(),
                }],
            },
        ] {
            let frame = crate::encode_frame(&resp).unwrap();
            let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
            assert_eq!(decoded, resp);
        }
    }

    #[test]
    fn volume_info_serializes_idle_holder_as_null() {
        let info = VolumeInfo {
            name: "prism-data".into(),
            size_bytes: 4096,
            disk_bytes: 1024,
            created: "2026-06-10T12:00:00Z".into(),
            in_use_by: None,
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["in_use_by"], serde_json::Value::Null);
        assert_eq!(json["name"], "prism-data");
        assert_eq!(json["disk_bytes"], 1024);
    }

    #[test]
    fn stop_run_survives_a_request_round_trip() {
        let req = Request::StopRun {
            run_id: 7,
            timeout_secs: 10,
        };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn run_stopped_survives_a_response_round_trip() {
        for forced in [false, true] {
            let resp = Response::RunStopped { forced };
            let frame = crate::encode_frame(&resp).unwrap();
            let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
            assert_eq!(decoded, resp);
        }
    }

    #[test]
    fn begin_integration_sign_in_survives_a_request_round_trip() {
        let req = Request::BeginIntegrationSignIn {
            id: "some-oauth".into(),
        };
        let frame = crate::encode_frame(&req).unwrap();
        let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn oauth_sign_in_responses_survive_round_trips() {
        for resp in [
            Response::OauthVerification {
                verification_uri: "https://example.com/login/device".into(),
                user_code: "WDJB-MJHT".into(),
                expires_in_secs: 900,
            },
            Response::OauthSignInComplete,
            Response::OauthSignInFailed {
                reason: "access_denied".into(),
            },
        ] {
            let frame = crate::encode_frame(&resp).unwrap();
            let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
            assert_eq!(decoded, resp);
        }
    }

    #[test]
    fn image_requests_survive_round_trips() {
        for req in [
            Request::PullImage {
                image: "registry.example.test/some/image:1.0".into(),
            },
            Request::ListImages,
            Request::RemoveImage {
                image: "registry.example.test/some/image:1.0".into(),
            },
            Request::PruneImages,
        ] {
            let frame = crate::encode_frame(&req).unwrap();
            let decoded: Request = crate::decode_frame(&mut &frame[..]).unwrap();
            assert_eq!(decoded, req);
        }
    }

    #[test]
    fn image_responses_survive_round_trips() {
        let info = ImageInfo {
            reference: "registry.example.test/some/image:1.0".into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            size_bytes: 3 * 1024 * 1024,
            layers: 2,
            pulled: "2026-06-10T12:00:00Z".into(),
            in_use_by: Some(7),
        };
        for resp in [
            Response::ImagePulled {
                image: info.clone(),
            },
            Response::ImageList {
                images: vec![info.clone()],
            },
            Response::ImageRemoved {
                reference: info.reference.clone(),
                reclaimed_bytes: 3 * 1024 * 1024,
            },
            Response::ImagesPruned {
                removed: vec![info.reference.clone()],
                reclaimed_bytes: 3 * 1024 * 1024,
            },
        ] {
            let frame = crate::encode_frame(&resp).unwrap();
            let decoded: Response = crate::decode_frame(&mut &frame[..]).unwrap();
            assert_eq!(decoded, resp);
        }
    }

    #[test]
    fn image_info_serializes_idle_holder_as_null() {
        let info = ImageInfo {
            reference: "registry.example.test/some/image:1.0".into(),
            digest: format!("sha256:{}", "b".repeat(64)),
            size_bytes: 4096,
            layers: 1,
            pulled: "2026-06-10T12:00:00Z".into(),
            in_use_by: None,
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["in_use_by"], serde_json::Value::Null);
        assert_eq!(json["reference"], "registry.example.test/some/image:1.0");
        assert_eq!(json["size_bytes"], 4096);
        assert_eq!(json["layers"], 1);
    }

    #[test]
    fn volume_mount_parse_name_and_absolute_target_defaults_to_rw() {
        let v = VolumeMount::parse("prism-data:/data").unwrap();
        assert_eq!(
            v,
            VolumeMount {
                name: "prism-data".into(),
                target: "/data".into(),
                read_only: false,
            }
        );
    }

    #[test]
    fn volume_mount_parse_honors_ro_and_rw_suffixes() {
        assert!(VolumeMount::parse("v:/data:ro").unwrap().read_only);
        assert!(!VolumeMount::parse("v:/data:rw").unwrap().read_only);
    }

    #[test]
    fn volume_mount_parse_rejects_missing_target_separator() {
        VolumeMount::parse("prism-data").unwrap_err();
    }

    #[test]
    fn volume_mount_parse_rejects_relative_target() {
        VolumeMount::parse("v:data").unwrap_err();
    }

    #[test]
    fn volume_mount_parse_rejects_invalid_name_with_path_separator() {
        let err = VolumeMount::parse("../etc:/data").unwrap_err();
        assert!(err.contains("invalid volume name"), "got: {err}");
    }

    #[test]
    fn volume_mount_parse_rejects_target_that_would_inject_kernel_cmdline_tokens() {
        let err = VolumeMount::parse("data:/d quiet console=ttyS0").unwrap_err();
        assert!(err.contains("must not contain whitespace"), "got: {err}");
        VolumeMount::parse("data:/my data").unwrap_err();
        VolumeMount::parse("data:/d\tx").unwrap_err();
        VolumeMount::parse("data:/d\nx").unwrap_err();
    }

    #[test]
    fn volume_mount_parse_rejects_target_with_a_double_quote() {
        let err = VolumeMount::parse(r#"data:/d"x"#).unwrap_err();
        assert!(err.contains("must not contain whitespace"), "got: {err}");
    }

    #[test]
    fn validate_volume_name_rejects_empty_dots_and_separators() {
        for bad in ["", ".", "..", "a/b", "a:b"] {
            validate_volume_name(bad).unwrap_err();
        }
        validate_volume_name("prism-data.v2_3").unwrap();
    }

    #[test]
    fn volume_mount_parse_rejects_dot_dot_segments_that_escape_the_overlay_root() {
        for bad in [
            "data:/../../proc",
            "data:/data/../../etc",
            "data:/..",
            "data:/foo/..",
        ] {
            let err = VolumeMount::parse(bad).unwrap_err();
            assert!(err.contains("must not contain"), "{bad}: got {err}");
        }
    }

    #[test]
    fn validate_volume_target_accepts_a_dot_inside_a_filename_but_rejects_dot_segments() {
        validate_volume_target("/srv/app.data").unwrap();
        validate_volume_target("/srv/.config").unwrap();
        validate_volume_target("/srv/..").unwrap_err();
        validate_volume_target("/../etc").unwrap_err();
        validate_volume_target("/a/./b").unwrap_err();
    }
}
