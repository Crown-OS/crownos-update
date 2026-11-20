use crate::{DownloadStatus, Snapshot};

/// The four calls that change the system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Check,
    Download,
    Apply,
    Restart,
}

impl Snapshot {
    /// Why `verb` does not fit this state, or `None` when it does. The daemon
    /// refuses with this text; a client can use it to disable a button.
    #[must_use]
    pub fn refusal(&self, verb: Verb) -> Option<&'static str> {
        if !self.is_idle() {
            return Some("An update step is already running.");
        }
        let pending_restart = self.pending_restart.is_some();
        match verb {
            Verb::Restart if !pending_restart => Some("No restart is pending."),
            Verb::Restart => None,
            _ if pending_restart => Some("Restart to finish the last update first."),
            Verb::Check | Verb::Download if self.in_progress.is_some() => {
                Some("Finish the current update first.")
            }
            Verb::Check => None,
            Verb::Download | Verb::Apply => match &self.next {
                None => Some("No update is available."),
                Some(next)
                    if verb == Verb::Download && next.download == DownloadStatus::Complete =>
                {
                    Some("The update is already downloaded.")
                }
                Some(_) => None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use update_manifest::RestartKind;

    use super::*;
    use crate::{NextRelease, PendingRestart, Phase};

    fn next(download: DownloadStatus) -> Result<NextRelease, Box<dyn std::error::Error>> {
        Ok(NextRelease {
            version: "0.4.1".parse()?,
            restart: RestartKind::Reboot,
            notes: None,
            new_packages: Vec::new(),
            download,
        })
    }

    #[test]
    fn an_idle_system_without_updates_only_checks() {
        let snapshot = Snapshot::default();
        assert_eq!(snapshot.refusal(Verb::Check), None);
        for verb in [Verb::Download, Verb::Apply, Verb::Restart] {
            assert!(snapshot.refusal(verb).is_some(), "{verb:?}");
        }
    }

    #[test]
    fn a_downloaded_update_applies_but_does_not_download_again()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut snapshot = Snapshot {
            next: Some(next(DownloadStatus::Pending)?),
            ..Snapshot::default()
        };
        assert_eq!(snapshot.refusal(Verb::Download), None);
        assert_eq!(snapshot.refusal(Verb::Apply), None);
        snapshot.next = Some(next(DownloadStatus::Complete)?);
        assert!(snapshot.refusal(Verb::Download).is_some());
        assert_eq!(snapshot.refusal(Verb::Apply), None);
        Ok(())
    }

    #[test]
    fn a_pending_restart_allows_only_the_restart() -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = Snapshot {
            next: Some(next(DownloadStatus::Pending)?),
            pending_restart: Some(PendingRestart {
                version: "0.4.1".parse()?,
                from: "0.4.0".parse()?,
                kind: RestartKind::Session,
            }),
            ..Snapshot::default()
        };
        assert_eq!(snapshot.refusal(Verb::Restart), None);
        for verb in [Verb::Check, Verb::Download, Verb::Apply] {
            assert!(snapshot.refusal(verb).is_some(), "{verb:?}");
        }
        Ok(())
    }

    #[test]
    fn an_unfinished_update_allows_only_apply() -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = Snapshot {
            next: Some(next(DownloadStatus::Pending)?),
            in_progress: Some(crate::InProgress {
                from: "0.4.0".parse()?,
                to: "0.4.1".parse()?,
                completed: crate::ApplyStep::KeepKernel,
            }),
            ..Snapshot::default()
        };
        assert_eq!(snapshot.refusal(Verb::Apply), None);
        for verb in [Verb::Check, Verb::Download, Verb::Restart] {
            assert!(snapshot.refusal(verb).is_some(), "{verb:?}");
        }
        Ok(())
    }

    #[test]
    fn a_running_step_refuses_everything() {
        let snapshot = Snapshot {
            phase: Phase::Checking,
            ..Snapshot::default()
        };
        for verb in [Verb::Check, Verb::Download, Verb::Apply, Verb::Restart] {
            assert!(snapshot.refusal(verb).is_some(), "{verb:?}");
        }
    }
}
