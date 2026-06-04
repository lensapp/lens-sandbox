#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

pub fn clamp_exit(code: i32) -> u8 {
    if !(0..=255).contains(&code) {
        255
    } else {
        code as u8
    }
}

pub fn translate_exit(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        255
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_exit_passes_through_in_range_codes() {
        assert_eq!(clamp_exit(0), 0);
        assert_eq!(clamp_exit(42), 42);
        assert_eq!(clamp_exit(255), 255);
    }

    #[test]
    fn clamp_exit_saturates_out_of_range_codes_to_255() {
        assert_eq!(clamp_exit(-1), 255);
        assert_eq!(clamp_exit(256), 255);
    }

    #[test]
    fn translate_exit_returns_the_code_for_a_normal_exit() {
        let normal_exit_with_code_42 = 42 << 8;
        assert_eq!(translate_exit(normal_exit_with_code_42), 42);
    }

    #[test]
    fn translate_exit_returns_128_plus_signal_for_a_killed_child() {
        assert_eq!(translate_exit(libc::SIGKILL), 128 + libc::SIGKILL);
    }

    #[test]
    fn translate_exit_returns_255_for_a_stopped_or_unknown_status() {
        let stopped_status = 0x7f;
        assert_eq!(translate_exit(stopped_status), 255);
    }
}
