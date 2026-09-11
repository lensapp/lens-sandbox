#[derive(Debug, PartialEq)]
pub struct Identity {
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub user: String,
}

pub trait Runner {
    async fn step(&mut self, index: usize) -> Result<i32, String>;
}

pub async fn execute(labels: &[String], runner: &mut impl Runner) -> Result<(), String> {
    for (index, label) in labels.iter().enumerate() {
        let refusal = |reason: String| {
            format!(
                "script {}/{}, {label}: {reason}; the workload did not start",
                index + 1,
                labels.len()
            )
        };
        let code = runner.step(index).await.map_err(&refusal)?;
        if code != 0 {
            return Err(refusal(format!("exited with code {code}")));
        }
    }
    Ok(())
}

pub fn resolve(user: &str, passwd: &str, groups: &str) -> Result<Identity, String> {
    let (name, group) = user
        .split_once(':')
        .map_or((user, None), |(u, g)| (u, Some(g)));
    let numeric = name.parse::<u32>().ok();
    let entry = passwd
        .lines()
        .map(|line| line.split(':').collect::<Vec<_>>())
        .find(|fields| {
            fields.len() >= 7
                && (fields[0] == name
                    || numeric.is_some_and(|uid| fields[2].parse::<u32>() == Ok(uid)))
        });
    let mut identity = match entry {
        Some(fields) => Identity {
            uid: fields[2].parse().map_err(|_| "invalid passwd uid")?,
            gid: fields[3].parse().map_err(|_| "invalid passwd gid")?,
            home: if fields[5].is_empty() { "/" } else { fields[5] }.into(),
            user: fields[0].into(),
        },
        None => Identity {
            uid: numeric.ok_or_else(|| format!("unknown script user {name:?}"))?,
            gid: numeric.ok_or_else(|| format!("unknown script user {name:?}"))?,
            home: "/".into(),
            user: name.into(),
        },
    };
    if let Some(group) = group {
        identity.gid = group
            .parse::<u32>()
            .ok()
            .or_else(|| {
                groups
                    .lines()
                    .filter_map(|line| {
                        let fields: Vec<_> = line.split(':').collect();
                        (fields.len() >= 3 && fields[0] == group)
                            .then(|| fields[2].parse().ok())
                            .flatten()
                    })
                    .next()
            })
            .ok_or_else(|| format!("unknown script group {group:?}"))?;
    }
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    const PASSWD: &str =
        "root:x:0:0:root:/root:/bin/bash\nnode:x:1000:1000:node:/home/node:/bin/sh\n";

    struct FakeRunner {
        codes: Vec<i32>,
        calls: Vec<usize>,
    }
    impl Runner for FakeRunner {
        async fn step(&mut self, index: usize) -> Result<i32, String> {
            self.calls.push(index);
            Ok(self.codes[index])
        }
    }
    #[tokio::test]
    async fn scripts_complete_in_declaration_order_before_workload_can_start() {
        let mut runner = FakeRunner {
            codes: vec![0, 0],
            calls: vec![],
        };
        execute(&["one".into(), "two".into()], &mut runner)
            .await
            .unwrap();
        assert_eq!(runner.calls, [0, 1]);
    }
    #[tokio::test]
    async fn failed_script_refuses_later_steps_and_names_the_failure() {
        let mut runner = FakeRunner {
            codes: vec![17, 0],
            calls: vec![],
        };
        let error = execute(&["install curl".into(), "two".into()], &mut runner)
            .await
            .unwrap_err();
        assert_eq!(runner.calls, [0]);
        assert!(
            error.contains("install curl")
                && error.contains("1/2")
                && error.contains("17")
                && error.contains("workload did not start"),
            "{error}"
        );
    }

    #[test]
    fn root_script_uses_root_identity_and_home() {
        assert_eq!(
            resolve("root", PASSWD, "").unwrap(),
            Identity {
                uid: 0,
                gid: 0,
                home: "/root".into(),
                user: "root".into(),
            }
        );
    }

    #[test]
    fn numeric_identity_uses_passwd_home_and_explicit_group() {
        assert_eq!(
            resolve("1000:staff", PASSWD, "staff:x:42:\n").unwrap(),
            Identity {
                uid: 1000,
                gid: 42,
                home: "/home/node".into(),
                user: "node".into(),
            }
        );
    }

    #[test]
    fn unknown_numeric_identity_uses_root_directory() {
        assert_eq!(
            resolve("123:456", PASSWD, "").unwrap(),
            Identity {
                uid: 123,
                gid: 456,
                home: "/".into(),
                user: "123".into(),
            }
        );
    }

    #[test]
    fn unknown_names_are_rejected() {
        assert!(resolve("missing", PASSWD, "").is_err());
        assert!(resolve("root:missing", PASSWD, "").is_err());
    }
}
