#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

mod real;
#[cfg(target_os = "linux")]
pub use real::release_volumes;

/// The volume targets lns-init mounted, so the broker can release the ones it must and nothing else.
pub const TARGETS_ENV: &str = "LENS_VOLUME_TARGETS";

pub trait VolumeSyscalls {
    fn umount(&self, target: &str) -> std::io::Result<()>;
    fn remount_read_only(&self, target: &str) -> std::io::Result<()>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Released {
    Unmounted,
    RemountedReadOnly,
    Failed(String),
}

/// A mounted ext4 keeps its clean bit cleared until the kernel puts the superblock back, and `sync` alone never does that.
pub fn release_volumes_with(
    sys: &dyn VolumeSyscalls,
    targets: &[String],
) -> Vec<(String, Released)> {
    // Reverse order, so a volume nested under another is released before its parent and plain umount succeeds for both.
    targets
        .iter()
        .rev()
        .map(|target| (target.clone(), release_one(sys, target)))
        .collect()
}

fn release_one(sys: &dyn VolumeSyscalls, target: &str) -> Released {
    match sys.umount(target) {
        Ok(()) => Released::Unmounted,
        // A workload can leave a process holding the mount; the read-only remount also re-marks the partition valid.
        Err(unmount) => match sys.remount_read_only(target) {
            Ok(()) => Released::RemountedReadOnly,
            Err(remount) => Released::Failed(format!(
                "umount({target}) failed: {unmount}; remount read-only failed: {remount}"
            )),
        },
    }
}

pub fn targets_from_env(raw: Option<String>) -> Vec<String> {
    raw.unwrap_or_default()
        .split(':')
        .filter(|target| !target.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeVolumeSyscalls {
        calls: Mutex<Vec<String>>,
        busy: Vec<String>,
        unremountable: Vec<String>,
    }

    impl VolumeSyscalls for FakeVolumeSyscalls {
        fn umount(&self, target: &str) -> std::io::Result<()> {
            self.calls.lock().unwrap().push(format!("umount({target})"));
            if self.busy.iter().any(|b| b == target) {
                return Err(std::io::Error::from(std::io::ErrorKind::ResourceBusy));
            }
            Ok(())
        }

        fn remount_read_only(&self, target: &str) -> std::io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("remount_ro({target})"));
            if self.unremountable.iter().any(|b| b == target) {
                return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
            }
            Ok(())
        }
    }

    #[test]
    fn every_volume_is_unmounted_before_the_guest_powers_off() {
        let sys = FakeVolumeSyscalls::default();

        let released = release_volumes_with(&sys, &["/data".into(), "/data/cache".into()]);

        assert_eq!(
            released,
            vec![
                ("/data/cache".to_string(), Released::Unmounted),
                ("/data".to_string(), Released::Unmounted),
            ],
            "a nested volume is released before its parent, so neither needs the busy fallback"
        );
        assert_eq!(
            *sys.calls.lock().unwrap(),
            vec!["umount(/data/cache)", "umount(/data)"],
            "a synced-but-still-mounted volume stays marked unchecked forever"
        );
    }

    #[test]
    fn a_busy_volume_falls_back_to_a_read_only_remount() {
        let sys = FakeVolumeSyscalls {
            busy: vec!["/data".into()],
            ..Default::default()
        };

        let released = release_volumes_with(&sys, &["/data".into()]);

        assert_eq!(
            released,
            vec![("/data".to_string(), Released::RemountedReadOnly)]
        );
        assert_eq!(
            *sys.calls.lock().unwrap(),
            vec!["umount(/data)", "remount_ro(/data)"],
            "a workload holding the mount must not cost the clean bit"
        );
    }

    #[test]
    fn a_volume_that_can_be_neither_unmounted_nor_remounted_reports_both_reasons() {
        let sys = FakeVolumeSyscalls {
            busy: vec!["/data".into()],
            unremountable: vec!["/data".into()],
            ..Default::default()
        };

        let released = release_volumes_with(&sys, &["/data".into()]);

        let reported: Vec<String> = released.iter().map(|r| format!("{r:?}")).collect();
        assert!(
            reported
                .iter()
                .any(|r| r.contains("umount(/data) failed")
                    && r.contains("remount read-only failed")),
            "a volume nobody could release must say what it tried, or the dirty image has no explanation: {reported:?}"
        );
    }

    #[test]
    fn a_run_with_no_volumes_releases_nothing() {
        let sys = FakeVolumeSyscalls::default();

        assert!(release_volumes_with(&sys, &[]).is_empty());
        assert!(sys.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn the_exported_target_list_decodes_without_inventing_a_target() {
        assert_eq!(
            targets_from_env(Some("/data:/cache".to_string())),
            vec!["/data".to_string(), "/cache".to_string()]
        );
        assert!(targets_from_env(None).is_empty());
        assert!(
            targets_from_env(Some(String::new())).is_empty(),
            "an empty export must not release the root filesystem"
        );
        assert_eq!(
            targets_from_env(Some(":/data:".to_string())),
            vec!["/data".to_string()],
            "a stray separator must not become an empty target"
        );
    }
}
