pub trait Children {
    fn exited(&mut self) -> std::io::Result<Option<i32>>;
    fn managed(&self, pid: i32) -> bool;
    fn reap(&mut self, pid: i32) -> std::io::Result<()>;
}

pub fn drain(children: &mut impl Children) -> std::io::Result<()> {
    while let Some(pid) = children.exited()? {
        if children.managed(pid) {
            break;
        }
        children.reap(pid)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub mod real;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Fake {
        exited: VecDeque<i32>,
        managed: Vec<i32>,
        reaped: Vec<i32>,
    }
    impl Children for Fake {
        fn exited(&mut self) -> std::io::Result<Option<i32>> {
            Ok(self.exited.pop_front())
        }
        fn managed(&self, pid: i32) -> bool {
            self.managed.contains(&pid)
        }
        fn reap(&mut self, pid: i32) -> std::io::Result<()> {
            self.reaped.push(pid);
            Ok(())
        }
    }
    #[test]
    fn incidental_children_are_reaped_without_stealing_managed_exit_status() {
        let mut children = Fake {
            exited: VecDeque::from([10, 11, 12]),
            managed: vec![11],
            reaped: vec![],
        };
        drain(&mut children).unwrap();
        assert_eq!(children.reaped, [10]);
        assert_eq!(children.exited, [12]);
    }
    #[test]
    fn drains_all_available_orphans_and_stops_at_no_children() {
        let mut children = Fake {
            exited: VecDeque::from([10, 12]),
            managed: vec![],
            reaped: vec![],
        };
        drain(&mut children).unwrap();
        assert_eq!(children.reaped, [10, 12]);
    }

    struct Broken {
        lookup: bool,
    }
    impl Children for Broken {
        fn exited(&mut self) -> std::io::Result<Option<i32>> {
            if self.lookup {
                Err(std::io::Error::other("waitid failed"))
            } else {
                Ok(Some(10))
            }
        }
        fn managed(&self, _: i32) -> bool {
            false
        }
        fn reap(&mut self, _: i32) -> std::io::Result<()> {
            Err(std::io::Error::other("waitpid failed"))
        }
    }

    #[test]
    fn reaper_errors_surface_instead_of_spinning_or_claiming_success() {
        assert_eq!(
            drain(&mut Broken { lookup: true }).unwrap_err().to_string(),
            "waitid failed"
        );
        assert_eq!(
            drain(&mut Broken { lookup: false })
                .unwrap_err()
                .to_string(),
            "waitpid failed"
        );
    }
}
