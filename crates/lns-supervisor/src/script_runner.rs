use crate::pre_start::{self, Identity, Runner};
use lns_openshell_spike::Error;
use lns_openshell_spike::launch::real::{self as launch, Launch, ProcessIo};
use openshell_core::policy::SandboxPolicy;
use std::collections::HashMap;
use std::os::fd::AsFd;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

pub struct Scripts {
    steps: Vec<(lns_session::ScriptManifestStep, Identity)>,
}

impl Scripts {
    pub fn load(uid: u32, gid: u32) -> Result<Self, Error> {
        let manifest: lns_session::ScriptManifest =
            match std::fs::read(lns_session::SCRIPTS_MANIFEST_PATH) {
                Ok(bytes) => serde_json::from_slice(&bytes)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
                Err(error) => return Err(error.into()),
            };
        if manifest.steps.is_empty() {
            return Ok(Self { steps: vec![] });
        }
        let passwd = std::fs::read_to_string("/etc/passwd")?;
        let groups = std::fs::read_to_string("/etc/group")?;
        let default = format!("{uid}:{gid}");
        let steps = manifest
            .steps
            .into_iter()
            .map(|step| {
                let identity =
                    pre_start::resolve(step.user.as_deref().unwrap_or(&default), &passwd, &groups)
                        .map_err(|error| {
                            format!("{}: {error}; the workload did not start", step.label)
                        })?;
                Ok((step, identity))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self { steps })
    }

    pub async fn run(
        self,
        policy: &SandboxPolicy,
        pid: Arc<AtomicU32>,
        env: &HashMap<String, String>,
        namespace: &std::fs::File,
    ) -> Result<(), Error> {
        if self.steps.is_empty() {
            return Ok(());
        }
        let labels = self
            .steps
            .iter()
            .map(|(step, _)| step.label.clone())
            .collect::<Vec<_>>();
        let mut runner = ScriptRunner {
            steps: self.steps,
            policy,
            namespace,
            pid,
            env,
        };
        pre_start::execute(&labels, &mut runner).await?;
        Ok(())
    }
}

struct ScriptRunner<'a> {
    steps: Vec<(lns_session::ScriptManifestStep, Identity)>,
    policy: &'a SandboxPolicy,
    namespace: &'a std::fs::File,
    pid: Arc<AtomicU32>,
    env: &'a HashMap<String, String>,
}

impl Runner for ScriptRunner<'_> {
    async fn step(&mut self, index: usize) -> Result<i32, String> {
        let (step, identity) = &self.steps[index];
        let mut policy = self.policy.clone();
        policy.process.run_as_user = Some(identity.uid.to_string());
        policy.process.run_as_group = Some(identity.gid.to_string());
        let mut env = self.env.clone();
        env.insert("HOME".into(), identity.home.clone());
        env.insert("USER".into(), identity.user.clone());
        env.extend(
            openshell_supervisor_process::child_env::proxy_env_vars("http://10.200.0.1:3128")
                .map(|(k, v)| (k.to_string(), v)),
        );
        let mut signals = crate::signals::Signals::new().map_err(|error| error.to_string())?;
        let (mut child, io) = launch::spawn(Launch {
            program: "/bin/sh",
            args: &["-e".into(), step.script.clone()],
            cwd: &identity.home,
            env: &env,
            uid: identity.uid,
            gid: identity.gid,
            terminal: false,
            namespace: Some(self.namespace.as_fd()),
            policy: &policy,
        })
        .map_err(|error| error.to_string())?;
        self.pid.store(child.pid(), Ordering::Release);
        let ProcessIo::Pipes {
            stdin,
            stdout,
            stderr,
        } = io
        else {
            return Err("expected script pipes".into());
        };
        drop(stdin);
        let prefix = format!(
            "[script {}/{} {}] ",
            index + 1,
            self.steps.len(),
            step.label
        );
        let status = tokio::try_join!(
            crate::signals::wait(&mut child, &mut signals),
            stream(stdout, &prefix),
            stream(stderr, &prefix)
        )
        .map_err(|error| error.to_string())?
        .0;
        Ok(crate::lifecycle::exit_code(
            status.exit_code(),
            status.signal(),
        ))
    }
}

async fn stream(mut input: impl AsyncRead + Unpin, prefix: &str) -> std::io::Result<()> {
    let mut buffer = [0; 4096];
    let mut output = tokio::io::stderr();
    let mut line_start = true;
    loop {
        let count = input.read(&mut buffer).await?;
        if count == 0 {
            return Ok(());
        }
        for part in buffer[..count].split_inclusive(|byte| *byte == b'\n') {
            if line_start {
                output.write_all(prefix.as_bytes()).await?;
            }
            output.write_all(part).await?;
            line_start = part.ends_with(b"\n");
        }
        output.flush().await?;
    }
}
