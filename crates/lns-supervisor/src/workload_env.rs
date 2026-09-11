use std::collections::HashMap;

pub fn resolve(
    mut env: HashMap<String, String>,
    guest: &HashMap<String, String>,
) -> HashMap<String, String> {
    for (key, declared, resolved) in [
        ("HOME", "LENS_SANDBOX_WORKLOAD_HOME", "LENS_RUN_HOME"),
        ("USER", "LENS_SANDBOX_WORKLOAD_USER", "LENS_SANDBOX_USER"),
    ] {
        if let Some(value) = env
            .get(declared)
            .or_else(|| guest.get(declared))
            .filter(|value| !value.is_empty())
            .or_else(|| guest.get(resolved))
        {
            env.insert(key.into(), value.clone());
        }
    }
    env.retain(|key, _| !key.starts_with("LENS_") && !key.starts_with("OPENSHELL_"));
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_identity_survives_numeric_privilege_resolution() {
        let guest = HashMap::from([
            ("LENS_RUN_HOME".into(), "/home/node".into()),
            ("LENS_SANDBOX_USER".into(), "node".into()),
            ("HOME".into(), "/root".into()),
        ]);
        let env = resolve(HashMap::new(), &guest);
        assert_eq!(env.get("HOME").map(String::as_str), Some("/home/node"));
        assert_eq!(env.get("USER").map(String::as_str), Some("node"));
    }

    #[test]
    fn declared_identity_wins_and_internal_markers_do_not_escape() {
        let guest = HashMap::from([
            ("LENS_RUN_HOME".into(), "/home/node".into()),
            ("LENS_SANDBOX_USER".into(), "node".into()),
            ("LENS_SANDBOX_WORKLOAD_USER".into(), "custom".into()),
        ]);
        let policy = HashMap::from([
            ("LENS_SANDBOX_WORKLOAD_HOME".into(), "/custom".into()),
            ("LENS_SANDBOX_TOKEN".into(), "private".into()),
            ("OPENSHELL_INTERNAL".into(), "private".into()),
        ]);
        let env = resolve(policy, &guest);
        assert_eq!(
            env,
            HashMap::from([
                ("HOME".into(), "/custom".into()),
                ("USER".into(), "custom".into())
            ])
        );
    }
}
