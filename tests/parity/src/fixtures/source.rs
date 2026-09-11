use super::remote::RemoteFixtures;
use super::{Activity, FixtureReport, Fixtures, HARNESS_VERSION, Role};
use crate::result::{FixturesMode, FixturesRecord};
use anyhow::{Context, Result};
use std::net::Ipv4Addr;
use std::path::Path;

/// Where this run's fixtures live: in this process, or on another machine behind its report server.
pub enum FixtureSource {
    InProcess(Fixtures),
    Remote(Box<RemoteFixtures>),
}

impl FixtureSource {
    pub fn bind(&self) -> Ipv4Addr {
        match self {
            FixtureSource::InProcess(fixtures) => fixtures.bind(),
            FixtureSource::Remote(remote) => remote.bind(),
        }
    }

    pub fn port(&self, role: Role) -> u16 {
        match self {
            FixtureSource::InProcess(fixtures) => fixtures.port(role),
            FixtureSource::Remote(remote) => remote.port(role),
        }
    }

    pub fn report(&self) -> FixtureReport {
        match self {
            FixtureSource::InProcess(fixtures) => fixtures.report(),
            FixtureSource::Remote(remote) => remote.report(),
        }
    }

    pub fn guest_destinations(&self) -> Vec<String> {
        match self {
            FixtureSource::InProcess(fixtures) => fixtures.guest_destinations(),
            FixtureSource::Remote(remote) => remote.guest_destinations(),
        }
    }

    pub fn activity_since(&self, mark: u64) -> Activity {
        match self {
            FixtureSource::InProcess(fixtures) => fixtures.activity_since(mark),
            FixtureSource::Remote(remote) => remote.activity_since(mark),
        }
    }

    /// Clears what the fixtures saw before a case starts; the in-process fixtures keep counting as they always have, because the runner holds them and nothing else reads them.
    pub fn reset(&self) -> Result<()> {
        match self {
            FixtureSource::InProcess(_) => Ok(()),
            FixtureSource::Remote(remote) => remote.reset(),
        }
    }

    pub fn write_report(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.report())?;
        std::fs::write(path, json).with_context(|| format!("write {}", path.display()))
    }

    pub fn shutdown(&self) {
        if let FixtureSource::InProcess(fixtures) = self {
            fixtures.shutdown();
        }
    }

    pub fn record(&self) -> FixturesRecord {
        match self {
            FixtureSource::InProcess(fixtures) => FixturesRecord {
                host: fixtures.bind().to_string(),
                mode: FixturesMode::InProcess,
                version: HARNESS_VERSION.to_string(),
            },
            FixtureSource::Remote(remote) => FixturesRecord {
                host: remote.endpoint().to_string(),
                mode: FixturesMode::Remote,
                version: remote.version().to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::Sizes;

    #[test]
    fn in_process_fixtures_record_the_host_they_bound_and_this_harnesss_version() {
        let source = FixtureSource::InProcess(
            Fixtures::start(Ipv4Addr::LOCALHOST, 0, Sizes::default()).unwrap(),
        );
        let record = source.record();

        assert_eq!(record.mode, FixturesMode::InProcess);
        assert_eq!(record.host, "127.0.0.1");
        assert_eq!(record.version, HARNESS_VERSION);
        source
            .reset()
            .expect("the in-process fixtures keep counting");
        source.shutdown();
    }

    #[test]
    fn the_report_the_runner_writes_is_the_one_the_source_holds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixtures.json");
        let source = FixtureSource::InProcess(
            Fixtures::start(Ipv4Addr::LOCALHOST, 0, Sizes::default()).unwrap(),
        );
        source.write_report(&path).unwrap();

        let parsed: FixtureReport =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.bind, "127.0.0.1");
        assert_eq!(source.guest_destinations().len(), 8);
        assert!(source.port(Role::Echo) > 0);
        assert_eq!(source.activity_since(0), Activity::default());
    }
}
