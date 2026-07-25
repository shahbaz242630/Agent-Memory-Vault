//! Linux backend: a systemd user timer (preferred) with a cron fallback.
//!
//! [`LinuxScheduler`] probes for a reachable systemd user manager and uses the
//! systemd path when present (it gives missed-run catch-up via
//! `Persistent=true`); otherwise it falls back to cron. `unregister` cleans
//! **both** mechanisms so removal is correct regardless of which one installed
//! the task.
//!
//! The two submodules keep their pure builders compiled and tested on every
//! platform; each `imp` (the `systemctl` / `crontab` shell-out) is
//! `#[cfg(target_os = "linux")]`.

mod cron;
mod systemd;

#[cfg(target_os = "linux")]
pub(crate) use imp::LinuxScheduler;

#[cfg(target_os = "linux")]
mod imp {
    use super::{cron, systemd};
    use crate::error::SchedulerResult;
    use crate::spec::{ScheduleSpec, ScheduleStatus, TaskId};
    use crate::Scheduler;

    /// Linux scheduler: systemd user timer preferred, cron as fallback.
    pub(crate) struct LinuxScheduler;

    impl Scheduler for LinuxScheduler {
        fn register(&self, spec: &ScheduleSpec) -> SchedulerResult<()> {
            spec.validate()?;
            if systemd::imp::available() {
                systemd::imp::register(spec)
            } else {
                cron::imp::register(spec)
            }
        }

        fn unregister(&self, task_id: &TaskId) -> SchedulerResult<()> {
            // Clean both mechanisms (each idempotent) so removal is correct no
            // matter which one registered the task. Surface the last real error.
            let mut last_err = None;
            if systemd::imp::available() {
                if let Err(e) = systemd::imp::unregister(task_id.as_str()) {
                    last_err = Some(e);
                }
            }
            if let Err(e) = cron::imp::unregister(task_id.as_str()) {
                last_err = Some(e);
            }
            match last_err {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }

        fn status(&self, task_id: &TaskId) -> SchedulerResult<ScheduleStatus> {
            let via_systemd =
                systemd::imp::available() && systemd::imp::is_enabled(task_id.as_str())?;
            let via_cron = cron::imp::is_present(task_id.as_str())?;
            Ok(ScheduleStatus {
                registered: via_systemd || via_cron,
                detail: None,
            })
        }
    }
}
