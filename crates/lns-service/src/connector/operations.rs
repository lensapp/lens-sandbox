use super::handler::Connected;
use super::mechanism::oauth::Pending;
use super::session::Session;
use super::store::Connection;
use anyhow::{Result, bail};
use lns_ipc::OAuthProgress;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Pending(OAuthProgress),
    Completed(Connected),
    Failed(String),
}

#[derive(Clone)]
pub struct Work {
    pub session: Session,
    pub previous: Option<Connection>,
}

#[derive(Default)]
pub struct Operations {
    inner: std::sync::Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    serial: u64,
    entries: std::collections::BTreeMap<String, Entry>,
}

struct Entry {
    connector: String,
    label: String,
    work: Option<Work>,
    status: Status,
    next: u64,
    busy: bool,
    expires: u64,
}

impl Operations {
    pub fn open(&self, mut work: Work, pending: Pending, now: u64) -> Result<(String, Vec<Work>)> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut replaced = expire(&mut inner, now);
        for entry in inner.entries.values_mut().filter(|entry| {
            entry.connector == work.session.connector && entry.label == work.session.label
        }) {
            if let Some(old) = entry.work.take() {
                replaced.push(old);
                entry.status = Status::Pending(OAuthProgress::Canceled);
            }
        }
        let active: Vec<_> = inner
            .entries
            .values()
            .filter(|entry| entry.work.is_some())
            .collect();
        if active.len() >= 8
            || active
                .iter()
                .filter(|entry| entry.connector == work.session.connector)
                .count()
                >= 2
        {
            bail!("too many OAuth operations are open; finish or cancel an authorization first");
        }
        if inner.entries.len() >= 64
            && let Some(key) = inner
                .entries
                .iter()
                .find(|(_, entry)| entry.work.is_none())
                .map(|(key, _)| key.clone())
        {
            inner.entries.remove(&key);
        }
        inner.serial = inner
            .serial
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("OAuth operation handles exhausted"))?;
        let handle = format!("oauth/{}", inner.serial);
        work.session.state = pending.state;
        work.session.expires_at_millis = work
            .session
            .expires_at_millis
            .min(pending.expires_at_millis);
        inner.entries.insert(
            handle.clone(),
            Entry {
                connector: work.session.connector.clone(),
                label: work.session.label.clone(),
                expires: work.session.expires_at_millis,
                work: Some(work),
                status: Status::Pending(pending.progress),
                next: pending.next_at_millis,
                busy: false,
            },
        );
        Ok((handle, replaced))
    }

    pub fn status(&self, handle: &str) -> Option<(String, Status)> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .get(handle)
            .map(|entry| (entry.connector.clone(), entry.status.clone()))
    }

    pub fn due(&self, now: u64) -> Vec<String> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .iter()
            .filter(|(_, entry)| entry.work.is_some() && !entry.busy && entry.next <= now)
            .map(|(key, _)| key.clone())
            .collect()
    }

    pub fn claim(&self, handle: &str, now: u64) -> Option<Work> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let entry = inner.entries.get_mut(handle)?;
        if entry.busy || entry.next > now || entry.expires <= now {
            return None;
        }
        let work = entry.work.clone()?;
        entry.busy = true;
        Some(work)
    }

    pub fn advance(&self, handle: &str, now: u64, next: Pending) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = inner
            .entries
            .get_mut(handle)
            .filter(|entry| entry.work.is_some() && entry.busy && entry.expires > now)
        else {
            return false;
        };
        entry.busy = false;
        entry.expires = entry.expires.min(next.expires_at_millis);
        if let Some(work) = &mut entry.work {
            work.session.state = next.state;
            work.session.expires_at_millis = entry.expires;
        }
        entry.next = next.next_at_millis;
        entry.status = Status::Pending(next.progress);
        true
    }

    pub fn complete(
        &self,
        handle: &str,
        now: u64,
        finish: impl FnOnce() -> Result<Status>,
    ) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = inner
            .entries
            .get_mut(handle)
            .filter(|entry| entry.work.is_some() && entry.busy && entry.expires > now)
        else {
            return false;
        };
        entry.busy = false;
        entry.work = None;
        entry.status = finish().unwrap_or_else(|e| Status::Failed(format!("{e:#}")));
        true
    }

    pub fn cancel(&self, handle: &str) -> Option<Work> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let entry = inner.entries.get_mut(handle)?;
        let work = entry.work.take()?;
        entry.status = Status::Pending(OAuthProgress::Canceled);
        Some(work)
    }

    pub fn cancel_connector(&self, name: &str, label: Option<&str>) -> Vec<Work> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner
            .entries
            .values_mut()
            .filter(|entry| {
                entry.connector == name && label.is_none_or(|label| entry.label == label)
            })
            .filter_map(|entry| {
                let work = entry.work.take()?;
                entry.status = Status::Pending(OAuthProgress::Canceled);
                Some(work)
            })
            .collect()
    }

    pub fn sweep(&self, now: u64) -> Vec<Work> {
        expire(
            &mut self.inner.lock().unwrap_or_else(|e| e.into_inner()),
            now,
        )
    }
    pub fn inspect(&self, handle: &str) -> Option<Work> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .get(handle)?
            .work
            .clone()
    }
}

fn expire(inner: &mut Inner, now: u64) -> Vec<Work> {
    let mut expired = Vec::new();
    for entry in inner
        .entries
        .values_mut()
        .filter(|entry| entry.expires <= now)
    {
        if let Some(work) = entry.work.take() {
            expired.push(work);
            entry.status = Status::Pending(OAuthProgress::Expired);
        }
    }
    inner
        .entries
        .retain(|_, entry| entry.expires.saturating_add(30_000) > now);
    expired
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pending() -> Pending {
        Pending {
            progress: OAuthProgress::Starting {
                destinations: vec![],
                scopes: vec![],
            },
            state: b"private".to_vec(),
            next_at_millis: 5,
            expires_at_millis: 100,
        }
    }
    fn work(name: &str, label: &str) -> Work {
        Work {
            session: Session {
                connector: name.into(),
                digest: "digest".into(),
                method: "sign-in".into(),
                label: label.into(),
                state: vec![],
                expires_at_millis: 100,
            },
            previous: None,
        }
    }
    #[test]
    fn terminal_status_retention_is_bounded_and_unclaimed_results_cannot_advance() {
        let operations = Operations::default();
        let (first, _) = operations
            .open(work("provider", "work"), pending(), 0)
            .unwrap();
        assert!(operations.inspect(&first).is_some());
        assert!(!operations.advance(&first, 1, pending()));
        operations.cancel(&first).unwrap();
        assert!(operations.inspect(&first).is_none());
        for _ in 0..70 {
            let (handle, _) = operations
                .open(work("provider", "work"), pending(), 0)
                .unwrap();
            operations.cancel(&handle).unwrap();
        }
        assert_eq!(operations.inner.lock().unwrap().entries.len(), 64);
        operations.sweep(30_100);
        assert!(operations.inner.lock().unwrap().entries.is_empty());
    }

    #[test]
    fn status_reads_cannot_claim_work_and_cancellation_prevents_a_late_save() {
        let operations = Operations::default();
        let (handle, _) = operations
            .open(work("provider", "work"), pending(), 0)
            .unwrap();
        let first = operations.status(&handle).unwrap();
        for _ in 0..20 {
            assert_eq!(operations.status(&handle).unwrap(), first);
        }
        assert!(operations.claim(&handle, 4).is_none());
        assert!(operations.claim(&handle, 5).is_some());
        assert!(operations.claim(&handle, 5).is_none());
        assert!(operations.cancel(&handle).is_some());
        assert!(!operations.complete(&handle, 6, || panic!("a canceled result must never save")));
        assert_eq!(
            operations.status(&handle).unwrap().1,
            Status::Pending(OAuthProgress::Canceled)
        );
    }
    #[test]
    fn native_operations_have_bounded_lifetimes_capacity_and_supersession() {
        let operations = Operations::default();
        let (first, _) = operations
            .open(work("provider", "work"), pending(), 0)
            .unwrap();
        let (second, old) = operations
            .open(work("provider", "work"), pending(), 0)
            .unwrap();
        assert_eq!(old.len(), 1);
        assert!(operations.claim(&first, 5).is_none());
        operations
            .open(work("provider", "other"), pending(), 0)
            .unwrap();
        assert!(
            operations
                .open(work("provider", "third"), pending(), 0)
                .is_err()
        );
        for n in 0..6 {
            operations
                .open(work(&format!("provider-{n}"), "work"), pending(), 0)
                .unwrap();
        }
        assert!(
            operations
                .open(work("ninth", "work"), pending(), 0)
                .is_err()
        );
        assert_eq!(operations.due(5).len(), 8);
        assert_eq!(operations.sweep(100).len(), 8);
        assert_eq!(
            operations.status(&second).unwrap().1,
            Status::Pending(OAuthProgress::Expired)
        );
        assert!(operations.claim(&second, 100).is_none());
    }
}
