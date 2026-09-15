#[cfg(target_os = "linux")]
#[test]
fn a_real_directory_listing_refuses_non_utf8_names() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let root = tempfile::tempdir().expect("fileset directory");
    std::fs::write(root.path().join("ordinary.txt"), b"ordinary").expect("ordinary file");
    let name = OsStr::from_bytes(&[0xff, 0xfe]);
    std::fs::write(root.path().join(name), b"invalid name").expect("non-UTF-8 file");
    let error = lns_artifact::walk::real_dir_entries(root.path())
        .expect_err("the real directory reader must not pack a lossy filename");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("non-utf8"), "{error}");
}
