use crate::cmdline::CmdlineParams;
use crate::mount;

pub(crate) trait CmdlineSource {
    fn read_cmdline(&self) -> std::io::Result<String>;
}

const PROC_CMDLINE: &str = "/proc/cmdline";

pub(crate) fn run_host(src: &dyn CmdlineSource) -> Result<(), String> {
    let cmdline = src
        .read_cmdline()
        .map_err(|e| format!("reading {PROC_CMDLINE}: {e}"))?;
    let params = CmdlineParams::parse(&cmdline);
    eprintln!(
        "lns-init: parsed cmdline params on non-Linux host: \
         upper.dev={:?} composefs.descriptor.dev={:?} content.tag={:?} \
         composefs.descriptor.sha256={:?}",
        params.upper_dev,
        params.composefs_descriptor_dev,
        params.content_tag,
        params.composefs_descriptor_sha256,
    );
    mount::validate_cmdline(&params).map_err(|e| format!("{e}"))?;
    Err(
        "lns-init's mount sequence is Linux-only — host build is for cargo check / test only"
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeCmdline(std::io::Result<String>);
    impl CmdlineSource for FakeCmdline {
        fn read_cmdline(&self) -> std::io::Result<String> {
            match &self.0 {
                Ok(s) => Ok(s.clone()),
                Err(e) => Err(std::io::Error::new(e.kind(), e.to_string())),
            }
        }
    }

    #[test]
    fn surfaces_cmdline_read_failure() {
        let src = FakeCmdline(Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "nope",
        )));
        let err = run_host(&src).unwrap_err();
        assert!(err.contains("reading /proc/cmdline"), "got: {err}");
    }

    #[test]
    fn rejects_incomplete_cmdline_before_linux_sentinel() {
        let src = FakeCmdline(Ok("upper.dev=/dev/vda".to_string()));
        let err = run_host(&src).unwrap_err();
        assert!(err.contains("composefs.descriptor.dev"), "got: {err}");
    }

    #[test]
    fn returns_linux_only_sentinel_when_cmdline_valid() {
        let src = FakeCmdline(Ok(
            "upper.dev=/dev/vda composefs.descriptor.dev=/dev/vdb content.tag=lns".to_string(),
        ));
        let err = run_host(&src).unwrap_err();
        assert!(err.contains("Linux-only"), "got: {err}");
    }
}
