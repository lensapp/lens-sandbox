use std::path::Path;

use anyhow::Result;

use super::{ManifestEntry, UpdateArgs};

pub(super) trait Host {
    async fn latest(&self) -> Result<ManifestEntry>;
    async fn download(&self, entry: &ManifestEntry) -> Result<Vec<u8>>;
    async fn install(&self, entry: &ManifestEntry, bytes: &[u8], executable: &Path) -> Result<()>;
}

pub(super) async fn run_with(
    host: &impl Host,
    args: UpdateArgs,
    version: &str,
    executable: &Path,
) -> Result<i32> {
    let entry = host.latest().await?;
    if entry.version == version && !args.force {
        println!("LNS {version} is already up to date.");
        return Ok(0);
    }
    let bytes = host.download(&entry).await?;
    host.install(&entry, &bytes, executable).await?;
    println!("Updated LNS to {}.", entry.version);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        calls: RefCell<Vec<&'static str>>,
        fail: &'static str,
    }

    impl Fake {
        fn called(&self, step: &'static str) -> Result<()> {
            self.calls.borrow_mut().push(step);
            anyhow::ensure!(self.fail != step, "{step} failed");
            Ok(())
        }
    }

    impl Host for Fake {
        async fn latest(&self) -> Result<ManifestEntry> {
            self.called("manifest")?;
            Ok(ManifestEntry {
                version: "0.26.0".into(),
                url: "https://get.lns.run/app.zip".into(),
                sha256: "digest".into(),
            })
        }
        async fn download(&self, _: &ManifestEntry) -> Result<Vec<u8>> {
            self.called("download")?;
            Ok(vec![1, 2, 3])
        }
        async fn install(
            &self,
            entry: &ManifestEntry,
            bytes: &[u8],
            executable: &Path,
        ) -> Result<()> {
            assert_eq!(entry.version, "0.26.0");
            assert_eq!(bytes, [1, 2, 3]);
            assert_eq!(
                executable,
                Path::new("/Applications/LNS.app/Contents/Helpers/lns")
            );
            self.called("install")
        }
    }

    async fn run(fake: &Fake, version: &str, force: bool) -> Result<i32> {
        run_with(
            fake,
            UpdateArgs {
                force,
                dry_run: false,
            },
            version,
            Path::new("/Applications/LNS.app/Contents/Helpers/lns"),
        )
        .await
    }

    #[tokio::test]
    async fn update_installs_one_verified_release_as_a_complete_app() {
        let fake = Fake {
            calls: RefCell::new(vec![]),
            fail: "",
        };
        assert_eq!(run(&fake, "0.25.0", false).await.unwrap(), 0);
        assert_eq!(*fake.calls.borrow(), ["manifest", "download", "install"]);
    }

    #[tokio::test]
    async fn a_current_app_is_only_replaced_when_forced() {
        let fake = Fake {
            calls: RefCell::new(vec![]),
            fail: "",
        };
        run(&fake, "0.26.0", false).await.unwrap();
        assert_eq!(*fake.calls.borrow(), ["manifest"]);
        fake.calls.borrow_mut().clear();
        run(&fake, "0.26.0", true).await.unwrap();
        assert_eq!(*fake.calls.borrow(), ["manifest", "download", "install"]);
    }

    #[tokio::test]
    async fn each_failed_stage_stops_the_update_and_reports_failure() {
        for (fail, expected) in [
            ("manifest", vec!["manifest"]),
            ("download", vec!["manifest", "download"]),
            ("install", vec!["manifest", "download", "install"]),
        ] {
            let fake = Fake {
                calls: RefCell::new(vec![]),
                fail,
            };
            assert!(
                run(&fake, "0.25.0", false)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains(fail)
            );
            assert_eq!(*fake.calls.borrow(), expected);
        }
    }
}
