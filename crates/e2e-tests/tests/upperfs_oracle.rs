// Layer-1 differential oracles for the pure-Rust ext4 writer: they format an
// image with `lns_service::upperfs::write_ext4` and cross-check it against the
// real e2fsprogs / overlayfs tooling. They are #[ignore]d (real subprocesses +
// docker/e2fsprogs on the host) — run manually with:
//   cargo test -p e2e-tests --test upperfs_oracle -- --ignored

mod host_validation {
    use std::path::PathBuf;
    use std::process::Command;

    use lns_service::upperfs::{Plan, write_ext4};

    fn find_tool(name: &str) -> Option<PathBuf> {
        if let Ok(out) = Command::new("which").arg(name).output()
            && out.status.success()
        {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(PathBuf::from(s));
            }
        }
        for prefix in [
            "/opt/homebrew/opt/e2fsprogs/sbin",
            "/sbin",
            "/usr/sbin",
            "/usr/local/sbin",
        ] {
            let p = PathBuf::from(format!("{prefix}/{name}"));
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    fn produce_image(size_bytes: u64) -> (tempfile::TempDir, PathBuf, Plan) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("upper.img");
        let plan = Plan::new(size_bytes, [0xAA; 16], "lns-upper", 0x12345678).expect("plan");
        write_ext4(&plan, &path).expect("write_ext4");
        (dir, path, plan)
    }

    fn parse_field(out: &str, key: &str) -> Option<String> {
        for line in out.lines() {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix(key)
                && let Some(value) = rest.strip_prefix(':')
            {
                return Some(value.trim().to_string());
            }
        }
        None
    }

    #[test]
    #[ignore]
    fn e2fsck_reports_32_mib_image_clean() {
        let Some(e2fsck) = find_tool("e2fsck") else {
            eprintln!("SKIP: e2fsck not found on host");
            return;
        };
        let (_dir, path, _plan) = produce_image(32 * 1024 * 1024);
        let out = Command::new(&e2fsck)
            .args(["-f", "-n", "-v"])
            .arg(&path)
            .output()
            .expect("run e2fsck");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        eprintln!("e2fsck stdout:\n{stdout}");
        if !stderr.is_empty() {
            eprintln!("e2fsck stderr:\n{stderr}");
        }
        assert!(
            out.status.success(),
            "e2fsck exit status {:?} (non-zero means errors). stdout:\n{stdout}",
            out.status.code()
        );
    }

    #[test]
    #[ignore]
    fn e2fsck_reports_10_gib_image_clean() {
        let Some(e2fsck) = find_tool("e2fsck") else {
            eprintln!("SKIP: e2fsck not found on host");
            return;
        };
        let (_dir, path, _plan) = produce_image(10 * 1024 * 1024 * 1024);
        let out = Command::new(&e2fsck)
            .args(["-f", "-n"])
            .arg(&path)
            .output()
            .expect("run e2fsck");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        eprintln!("e2fsck stdout:\n{stdout}");
        assert!(
            out.status.success(),
            "e2fsck on 10 GiB image failed with status {:?}\n{stdout}",
            out.status.code()
        );
    }

    #[test]
    #[ignore]
    fn tune2fs_lists_expected_features() {
        let Some(tune2fs) = find_tool("tune2fs") else {
            eprintln!("SKIP: tune2fs not found on host");
            return;
        };
        let (_dir, path, _plan) = produce_image(32 * 1024 * 1024);
        let out = Command::new(&tune2fs)
            .arg("-l")
            .arg(&path)
            .output()
            .expect("run tune2fs");
        assert!(
            out.status.success(),
            "tune2fs failed: stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        eprintln!("tune2fs -l output:\n{stdout}");

        let features =
            parse_field(&stdout, "Filesystem features").expect("Filesystem features line");
        let feat_set: Vec<&str> = features.split_whitespace().collect();

        for required in [
            "has_journal",
            "ext_attr",
            "filetype",
            "extent",
            "sparse_super",
            "large_file",
        ] {
            assert!(
                feat_set.contains(&required),
                "missing required feature {required:?}; saw {feat_set:?}"
            );
        }
        for forbidden in [
            "64bit",
            "metadata_csum",
            "huge_file",
            "flex_bg",
            "resize_inode",
            "dir_index",
            "extra_isize",
            "inline_data",
        ] {
            assert!(
                !feat_set.contains(&forbidden),
                "forbidden feature {forbidden:?} is present; saw {feat_set:?}"
            );
        }
    }

    #[test]
    #[ignore]
    fn dumpe2fs_structural_fields_match_layout() {
        let Some(dumpe2fs) = find_tool("dumpe2fs") else {
            eprintln!("SKIP: dumpe2fs not found on host");
            return;
        };
        let (_dir, path, _plan) = produce_image(32 * 1024 * 1024);
        let out = Command::new(&dumpe2fs)
            .arg("-h")
            .arg(&path)
            .output()
            .expect("run dumpe2fs");
        assert!(out.status.success(), "dumpe2fs failed");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        eprintln!("dumpe2fs -h output:\n{stdout}");

        assert_eq!(parse_field(&stdout, "Block count").as_deref(), Some("8192"));
        assert_eq!(parse_field(&stdout, "Inode count").as_deref(), Some("2048"));
        assert_eq!(parse_field(&stdout, "Block size").as_deref(), Some("4096"));
        assert_eq!(parse_field(&stdout, "Inode size").as_deref(), Some("256"));
        assert_eq!(
            parse_field(&stdout, "Inodes per group").as_deref(),
            Some("2048")
        );
        let rev = parse_field(&stdout, "Filesystem revision #").expect("revision line");
        assert!(rev.starts_with('1'), "revision should be 1.x, got {rev:?}");
        assert!(rev.contains("dynamic"), "expected dynamic rev, got {rev:?}");
        assert_eq!(parse_field(&stdout, "First inode").as_deref(), Some("11"));
        assert_eq!(
            parse_field(&stdout, "Filesystem volume name").as_deref(),
            Some("lns-upper")
        );
    }

    #[test]
    #[ignore]
    fn dumpe2fs_reports_a_journal_of_the_planned_size() {
        let Some(dumpe2fs) = find_tool("dumpe2fs") else {
            eprintln!("SKIP: dumpe2fs not found on host");
            return;
        };
        let (_dir, path, plan) = produce_image(32 * 1024 * 1024);
        let out = Command::new(&dumpe2fs)
            .arg("-h")
            .arg(&path)
            .output()
            .expect("run dumpe2fs");
        assert!(out.status.success(), "dumpe2fs failed");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        eprintln!("dumpe2fs -h output:\n{stdout}");

        assert_eq!(parse_field(&stdout, "Journal inode").as_deref(), Some("8"));
        assert_eq!(
            parse_field(&stdout, "Total journal blocks").as_deref(),
            Some(plan.journal_blocks().to_string().as_str()),
            "dumpe2fs only reports this after it parses our big-endian JBD2 superblock"
        );
        assert_eq!(
            parse_field(&stdout, "Journal sequence").as_deref(),
            Some("0x00000001")
        );
        assert_eq!(parse_field(&stdout, "Journal start").as_deref(), Some("0"));
        assert_eq!(
            parse_field(&stdout, "Journal features").as_deref(),
            Some("(none)"),
            "a journal with no optional features is recoverable by any kernel"
        );
    }

    #[test]
    #[ignore]
    fn compare_with_mke2fs_reference_image() {
        let Some(mke2fs) = find_tool("mke2fs") else {
            eprintln!("SKIP: mke2fs not found on host");
            return;
        };
        let Some(dumpe2fs) = find_tool("dumpe2fs") else {
            eprintln!("SKIP: dumpe2fs not found on host");
            return;
        };

        let dir = tempfile::TempDir::new().expect("tempdir");
        let ours = dir.path().join("ours.img");
        let theirs = dir.path().join("theirs.img");

        let plan = Plan::new(32 * 1024 * 1024, [0xAA; 16], "lns-upper", 0).expect("plan");
        write_ext4(&plan, &ours).expect("write ours");

        let theirs_file = std::fs::File::create(&theirs).expect("create theirs");
        theirs_file
            .set_len(32 * 1024 * 1024)
            .expect("set_len theirs");
        drop(theirs_file);

        let mk = Command::new(&mke2fs)
            .args([
                "-t",
                "ext4",
                "-F",
                "-b",
                "4096",
                "-N",
                "2048",
                "-I",
                "256",
                "-O",
                "sparse_super,filetype,extents,ext_attr,large_file,has_journal,\
                 ^64bit,^huge_file,^flex_bg,^metadata_csum,\
                 ^dir_index,^resize_inode,^inline_data,^extra_isize",
                "-J",
                "size=4",
                "-U",
                "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                "-L",
                "lns-upper",
                "-m",
                "0",
                "-E",
                "lazy_itable_init=0",
            ])
            .arg(&theirs)
            .output()
            .expect("run mke2fs");
        assert!(
            mk.status.success(),
            "mke2fs failed: stderr={}",
            String::from_utf8_lossy(&mk.stderr)
        );

        let our_dump = Command::new(&dumpe2fs)
            .arg("-h")
            .arg(&ours)
            .output()
            .expect("dumpe2fs ours");
        let their_dump = Command::new(&dumpe2fs)
            .arg("-h")
            .arg(&theirs)
            .output()
            .expect("dumpe2fs theirs");
        let our_str = String::from_utf8_lossy(&our_dump.stdout).to_string();
        let their_str = String::from_utf8_lossy(&their_dump.stdout).to_string();

        eprintln!("==== OURS (dumpe2fs -h) ====\n{our_str}");
        eprintln!("==== THEIRS (mke2fs reference) ====\n{their_str}");

        let structural_keys = [
            "Filesystem revision #",
            "Filesystem OS type",
            "Inode count",
            "Block count",
            "Block size",
            "Inode size",
            "Inodes per group",
            "Blocks per group",
            "First inode",
        ];
        for key in structural_keys {
            let ours = parse_field(&our_str, key);
            let theirs = parse_field(&their_str, key);
            eprintln!("{key}: ours={ours:?} theirs={theirs:?}");
            assert_eq!(
                ours, theirs,
                "structural divergence at {key}: ours={ours:?} theirs={theirs:?}"
            );
        }

        let our_feat_line = parse_field(&our_str, "Filesystem features").unwrap_or_default();
        let their_feat_line = parse_field(&their_str, "Filesystem features").unwrap_or_default();
        let our_features: std::collections::HashSet<&str> =
            our_feat_line.split_whitespace().collect();
        let their_features: std::collections::HashSet<&str> =
            their_feat_line.split_whitespace().collect();
        let our_extra: Vec<_> = our_features.difference(&their_features).collect();
        let their_extra: Vec<_> = their_features.difference(&our_features).collect();
        eprintln!("features in ours but not theirs: {our_extra:?}");
        eprintln!("features in theirs but not ours: {their_extra:?}");
        assert!(
            our_extra.is_empty(),
            "our formatter sets features mke2fs doesn't — likely a bug"
        );
    }
}

mod overlayfs_validation {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    use lns_service::upperfs::{Plan, write_ext4};

    fn find_docker() -> Option<PathBuf> {
        if let Ok(out) = Command::new("which").arg("docker").output()
            && out.status.success()
        {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(PathBuf::from(s));
            }
        }
        for p in ["/usr/local/bin/docker", "/opt/homebrew/bin/docker"] {
            let pb = PathBuf::from(p);
            if pb.exists() {
                return Some(pb);
            }
        }
        None
    }

    fn produce_image() -> (tempfile::TempDir, PathBuf) {
        let parent = dirs::home_dir()
            .map(|h| h.join(".cache/lns-overlayfs-test"))
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&parent).expect("create parent tempdir");
        let dir = tempfile::TempDir::new_in(&parent).expect("tempdir");
        let path = dir.path().join("upper.img");
        let plan = Plan::new(32 * 1024 * 1024, [0xBB; 16], "lns-upper", 0).expect("plan");
        write_ext4(&plan, &path).expect("write_ext4");
        (dir, path)
    }

    fn run_in_alpine(docker: &Path, host_work: &Path, script: &str) -> Output {
        Command::new(docker)
            .args([
                "run",
                "--rm",
                "--privileged",
                "-v",
                &format!("{}:/work", host_work.display()),
                "alpine:3.20",
                "sh",
                "-eu",
                "-c",
                script,
            ])
            .output()
            .expect("run docker")
    }

    const PREAMBLE: &str = r#"
apk add --no-cache attr >/dev/null 2>&1 || true
mkdir -p /mnt/lower /mnt/upper /mnt/merged
mount -o loop /work/upper.img /mnt/upper
mkdir -p /mnt/upper/upper /mnt/upper/work
"#;

    const OVERLAY_MOUNT: &str = r#"
mount -t overlay overlay \
    -o lowerdir=/mnt/lower,upperdir=/mnt/upper/upper,workdir=/mnt/upper/work \
    /mnt/merged
"#;

    #[test]
    #[ignore]
    fn overlayfs_basic_mount_and_read_through() {
        let Some(docker) = find_docker() else {
            eprintln!("SKIP: docker not on host");
            return;
        };
        let (dir, _) = produce_image();

        let script = format!(
            r#"
{preamble}
echo "hello from lower" > /mnt/lower/file_in_lower.txt
{overlay}
cat /mnt/merged/file_in_lower.txt
ls /mnt/merged/ | grep -v lost+found | sort | tr '\n' ' '
echo
echo BASIC_MOUNT_OK
"#,
            preamble = PREAMBLE,
            overlay = OVERLAY_MOUNT,
        );

        let out = run_in_alpine(&docker, dir.path(), &script);
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        eprintln!("stdout:\n{stdout}");
        if !stderr.is_empty() {
            eprintln!("stderr:\n{stderr}");
        }
        assert!(out.status.success(), "docker exited non-zero");
        assert!(
            stdout.contains("hello from lower"),
            "lower-file content didn't read through"
        );
        assert!(
            stdout.contains("BASIC_MOUNT_OK"),
            "overlay mount + read-through didn't reach the sentinel"
        );
    }

    #[test]
    #[ignore]
    fn trusted_xattr_persists_on_upper() {
        let Some(docker) = find_docker() else {
            eprintln!("SKIP: docker not on host");
            return;
        };
        let (dir, _) = produce_image();

        let script = format!(
            r#"
{preamble}
{overlay}
touch /mnt/merged/marked
setfattr -n trusted.lns_test -v "from_overlayfs_phase4" /mnt/merged/marked
echo "---getfattr-on-merged---"
getfattr -d -m '^trusted' --absolute-names /mnt/merged/marked || true
echo "---getfattr-on-upper-directly---"
getfattr -d -m '^trusted' --absolute-names /mnt/upper/upper/marked || true
echo XATTR_OK
"#,
            preamble = PREAMBLE,
            overlay = OVERLAY_MOUNT,
        );

        let out = run_in_alpine(&docker, dir.path(), &script);
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        eprintln!("stdout:\n{stdout}");
        if !stderr.is_empty() {
            eprintln!("stderr:\n{stderr}");
        }
        assert!(out.status.success(), "docker exited non-zero");
        assert!(
            stdout.contains("trusted.lns_test=\"from_overlayfs_phase4\""),
            "trusted.* xattr didn't persist (the overlay-upper precondition)"
        );
        assert!(stdout.contains("XATTR_OK"));
    }

    #[test]
    #[ignore]
    fn whiteout_char_device_hides_lower_file() {
        let Some(docker) = find_docker() else {
            eprintln!("SKIP: docker not on host");
            return;
        };
        let (dir, _) = produce_image();

        let script = format!(
            r#"
{preamble}
echo "will be deleted" > /mnt/lower/target.txt
echo "kept" > /mnt/lower/keep.txt
{overlay}
ls /mnt/merged | sort | tr '\n' ' '
echo
rm /mnt/merged/target.txt
ls /mnt/merged | sort | tr '\n' ' '
echo
echo "---whiteout-on-upper---"
ls -la /mnt/upper/upper/target.txt
echo "---stat-major-minor---"
stat -c '%t,%T,%F' /mnt/upper/upper/target.txt
echo WHITEOUT_OK
"#,
            preamble = PREAMBLE,
            overlay = OVERLAY_MOUNT,
        );

        let out = run_in_alpine(&docker, dir.path(), &script);
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        eprintln!("stdout:\n{stdout}");
        if !stderr.is_empty() {
            eprintln!("stderr:\n{stderr}");
        }
        assert!(out.status.success(), "docker exited non-zero");
        let after_lines: Vec<&str> = stdout.lines().collect();
        let after = after_lines
            .iter()
            .filter(|l| !l.is_empty())
            .nth(1)
            .copied()
            .unwrap_or("");
        assert!(
            !after.contains("target.txt"),
            "deleted file should vanish from merged listing; saw: {after:?}"
        );
        assert!(after.contains("keep.txt"));
        assert!(
            stdout.contains("0,0,character special file"),
            "expected major,minor 0,0 char device on upper; full stdout was:\n{stdout}"
        );
        assert!(stdout.contains("WHITEOUT_OK"));
    }

    #[test]
    #[ignore]
    fn copy_up_of_large_file_lands_in_upper() {
        let Some(docker) = find_docker() else {
            eprintln!("SKIP: docker not on host");
            return;
        };
        let (dir, _) = produce_image();

        let script = format!(
            r#"
{preamble}
dd if=/dev/zero of=/mnt/lower/big.bin bs=1M count=10 2>/dev/null
ls -la /mnt/lower/big.bin
{overlay}
ls -la /mnt/upper/upper/ | wc -l
echo "before modify: upper had ${{?}} files"
printf 'X' | dd of=/mnt/merged/big.bin bs=1 seek=5242880 count=1 conv=notrunc 2>/dev/null
echo "---upper-after-copyup---"
ls -la /mnt/upper/upper/big.bin
stat -c 'size=%s blocks=%b' /mnt/upper/upper/big.bin
echo COPYUP_OK
"#,
            preamble = PREAMBLE,
            overlay = OVERLAY_MOUNT,
        );

        let out = run_in_alpine(&docker, dir.path(), &script);
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        eprintln!("stdout:\n{stdout}");
        if !stderr.is_empty() {
            eprintln!("stderr:\n{stderr}");
        }
        assert!(out.status.success(), "docker exited non-zero");
        assert!(stdout.contains("COPYUP_OK"), "copy-up flow didn't complete");
        assert!(
            stdout.contains("size=10485760"),
            "copy-up should land the full 10 MiB file in upper; stdout:\n{stdout}"
        );
    }
}
