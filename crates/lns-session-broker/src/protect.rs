pub fn environment(
    prctl: impl FnOnce(libc::c_int, libc::c_ulong) -> Result<(), String>,
) -> Result<(), String> {
    const PR_SET_DUMPABLE: libc::c_int = 4;
    prctl(PR_SET_DUMPABLE, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_startup_if_broker_environment_cannot_be_protected() {
        let result = environment(|option, value| {
            assert_eq!(option, 4);
            assert_eq!(value, 0);
            Err("prctl denied".into())
        });
        assert_eq!(result, Err("prctl denied".into()));
    }
}
