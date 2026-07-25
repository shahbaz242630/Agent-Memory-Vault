//! macOS backend — a per-user launchd LaunchAgent.
//!
//! Registration writes a plist to `~/Library/LaunchAgents/<label>.plist` and
//! loads it into the user's GUI domain with `launchctl bootstrap gui/<uid>`.
//! Because it is a LaunchAgent (not a system-domain LaunchDaemon), it runs as
//! the logged-in user with **no elevation**.
//!
//! launchd's own behaviour gives us two things for free:
//!
//! - **Missed-run catch-up** — a `StartCalendarInterval` job that was missed
//!   because the Mac was asleep runs once at the next wake, so a laptop that is
//!   shut at 03:00 still gets maintained.
//! - **Native argument handling** — `ProgramArguments` is an array of strings,
//!   so a path containing spaces is one element with no quoting; and
//!   `EnvironmentVariables` is a first-class dict, so unlike the Windows
//!   backend, macOS injects `ScheduleSpec::env` directly.
//!
//! The pure builder [`build_launchd_plist`] is compiled and unit-tested on
//! every platform; only [`MacScheduler`], which shells out to `launchctl`, is
//! `#[cfg(target_os = "macos")]`.

use chrono::Timelike;

use super::xml_escape;
use crate::spec::{Frequency, ScheduleSpec};

/// Build the launchd plist for `spec`.
///
/// Deterministic (no wall-clock read): the recurrence is expressed as a
/// `StartCalendarInterval`, which launchd evaluates itself. `RunAtLoad` is
/// `false` so loading the agent (at login, or on register) does not trigger an
/// immediate maintenance run — only the calendar schedule does.
pub(crate) fn build_launchd_plist(spec: &ScheduleSpec) -> String {
    let mut program_arguments = String::new();
    program_arguments.push_str(&format!(
        "      <string>{}</string>\n",
        xml_escape(&spec.program.to_string_lossy())
    ));
    for arg in &spec.args {
        program_arguments.push_str(&format!("      <string>{}</string>\n", xml_escape(arg)));
    }

    let hour = spec.time_of_day.hour();
    let minute = spec.time_of_day.minute();
    let calendar = match spec.frequency {
        Frequency::Daily => format!(
            "      <key>Hour</key>\n      <integer>{hour}</integer>\n\
             \x20     <key>Minute</key>\n      <integer>{minute}</integer>\n"
        ),
        Frequency::Weekly { day } => format!(
            "      <key>Hour</key>\n      <integer>{hour}</integer>\n\
             \x20     <key>Minute</key>\n      <integer>{minute}</integer>\n\
             \x20     <key>Weekday</key>\n      <integer>{weekday}</integer>\n",
            // launchd Weekday: 0 = Sunday .. 6 = Saturday, matching
            // chrono's num_days_from_sunday().
            weekday = day.num_days_from_sunday()
        ),
    };

    let environment = if spec.env.is_empty() {
        String::new()
    } else {
        let mut e = String::from("  <key>EnvironmentVariables</key>\n  <dict>\n");
        for (key, value) in &spec.env {
            e.push_str(&format!(
                "    <key>{}</key>\n    <string>{}</string>\n",
                xml_escape(key),
                xml_escape(value)
            ));
        }
        e.push_str("  </dict>\n");
        e
    };

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20 <key>Label</key>\n\
         \x20 <string>{label}</string>\n\
         \x20 <key>ProgramArguments</key>\n\
         \x20 <array>\n\
         {program_arguments}\
         \x20 </array>\n\
         \x20 <key>StartCalendarInterval</key>\n\
         \x20 <dict>\n\
         {calendar}\
         \x20 </dict>\n\
         {environment}\
         \x20 <key>RunAtLoad</key>\n\
         \x20 <false/>\n\
         \x20 <key>ProcessType</key>\n\
         \x20 <string>Background</string>\n\
         </dict>\n\
         </plist>\n",
        label = xml_escape(spec.task_id.as_str()),
    )
}

#[cfg(target_os = "macos")]
pub(crate) use imp::MacScheduler;

#[cfg(target_os = "macos")]
mod imp {
    use std::path::PathBuf;
    use std::process::Command;

    use super::build_launchd_plist;
    use crate::error::{SchedulerError, SchedulerResult};
    use crate::spec::{ScheduleSpec, ScheduleStatus, TaskId};
    use crate::Scheduler;

    /// launchd backend driving `launchctl`.
    pub(crate) struct MacScheduler;

    impl Scheduler for MacScheduler {
        fn register(&self, spec: &ScheduleSpec) -> SchedulerResult<()> {
            spec.validate()?;

            let plist = build_launchd_plist(spec);
            let path = plist_path(spec.task_id.as_str())?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(SchedulerError::Io)?;
            }
            std::fs::write(&path, plist).map_err(SchedulerError::Io)?;

            let domain = gui_domain()?;
            // Replace any existing instance: bootout is best-effort (it fails
            // harmlessly if the label is not currently loaded).
            let _ = Command::new("launchctl")
                .args(["bootout", &format!("{domain}/{}", spec.task_id.as_str())])
                .output();

            let output = Command::new("launchctl")
                .args(["bootstrap", &domain, &path.to_string_lossy()])
                .output()
                .map_err(SchedulerError::Io)?;
            if !output.status.success() {
                return Err(SchedulerError::BackendFailed(format!(
                    "launchctl bootstrap failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            Ok(())
        }

        fn unregister(&self, task_id: &TaskId) -> SchedulerResult<()> {
            // Idempotent: bootout of an unloaded label and removal of an absent
            // file are both non-fatal, so "make sure it's gone" always succeeds.
            let domain = gui_domain()?;
            let _ = Command::new("launchctl")
                .args(["bootout", &format!("{domain}/{}", task_id.as_str())])
                .output();

            let path = plist_path(task_id.as_str())?;
            if path.exists() {
                std::fs::remove_file(&path).map_err(SchedulerError::Io)?;
            }
            Ok(())
        }

        fn status(&self, task_id: &TaskId) -> SchedulerResult<ScheduleStatus> {
            // `launchctl list <label>` exits non-zero when the label is not
            // loaded; that is "not registered", not a backend failure.
            let output = Command::new("launchctl")
                .args(["list", task_id.as_str()])
                .output()
                .map_err(SchedulerError::Io)?;
            Ok(ScheduleStatus {
                registered: output.status.success(),
                detail: None,
            })
        }
    }

    /// `~/Library/LaunchAgents/<label>.plist`. The label is charset-validated,
    /// so it is a safe filename component.
    fn plist_path(task_id: &str) -> SchedulerResult<PathBuf> {
        let home = std::env::var("HOME")
            .map_err(|_| SchedulerError::BackendFailed("HOME is not set".into()))?;
        let mut path = PathBuf::from(home);
        path.push("Library");
        path.push("LaunchAgents");
        path.push(format!("{task_id}.plist"));
        Ok(path)
    }

    /// The current user's GUI launchd domain, `gui/<uid>`. `uid` comes from
    /// `id -u` to avoid a `libc` dependency for `getuid`.
    fn gui_domain() -> SchedulerResult<String> {
        let output = Command::new("id")
            .arg("-u")
            .output()
            .map_err(SchedulerError::Io)?;
        if !output.status.success() {
            return Err(SchedulerError::BackendFailed(
                "could not determine uid via `id -u`".into(),
            ));
        }
        let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(format!("gui/{uid}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::TaskId;
    use chrono::{NaiveTime, Weekday};
    use std::path::PathBuf;

    fn daily_spec_with_env() -> ScheduleSpec {
        ScheduleSpec {
            task_id: TaskId::new("com.memoryvault.maintenance").unwrap(),
            label: "Memory Vault automatic maintenance".into(),
            frequency: Frequency::Daily,
            time_of_day: NaiveTime::from_hms_opt(3, 5, 0).unwrap(),
            // A mac-style path with a space — launchd takes it as one array
            // element with no quoting.
            program: PathBuf::from("/Applications/Memory Vault.app/Contents/MacOS/vault-cli"),
            args: vec!["consolidate".into(), "run".into()],
            env: vec![("LANCE_MEM_POOL_SIZE".into(), "268435456".into())],
        }
    }

    #[test]
    fn daily_plist_has_label_arguments_and_calendar_time() {
        let plist = build_launchd_plist(&daily_spec_with_env());
        assert!(plist.contains("<key>Label</key>"));
        assert!(plist.contains("<string>com.memoryvault.maintenance</string>"));
        // Program and each argument are distinct <string> elements.
        assert!(plist
            .contains("<string>/Applications/Memory Vault.app/Contents/MacOS/vault-cli</string>"));
        assert!(plist.contains("<string>consolidate</string>"));
        assert!(plist.contains("<string>run</string>"));
        // 03:05.
        assert!(plist.contains("<key>Hour</key>\n      <integer>3</integer>"));
        assert!(plist.contains("<key>Minute</key>\n      <integer>5</integer>"));
        assert!(!plist.contains("<key>Weekday</key>"));
        // Does not run on load — only on schedule.
        assert!(plist.contains("<key>RunAtLoad</key>\n  <false/>"));
    }

    #[test]
    fn env_is_emitted_as_a_dict_when_present_and_omitted_when_empty() {
        let with_env = build_launchd_plist(&daily_spec_with_env());
        assert!(with_env.contains("<key>EnvironmentVariables</key>"));
        assert!(with_env.contains("<key>LANCE_MEM_POOL_SIZE</key>"));
        assert!(with_env.contains("<string>268435456</string>"));

        let mut no_env = daily_spec_with_env();
        no_env.env.clear();
        let without = build_launchd_plist(&no_env);
        assert!(!without.contains("EnvironmentVariables"));
    }

    #[test]
    fn weekly_plist_carries_the_weekday_integer() {
        let mut spec = daily_spec_with_env();
        spec.frequency = Frequency::Weekly { day: Weekday::Sun };
        let plist = build_launchd_plist(&spec);
        // Sunday = 0 in both launchd and chrono's num_days_from_sunday().
        assert!(plist.contains("<key>Weekday</key>\n      <integer>0</integer>"));

        spec.frequency = Frequency::Weekly { day: Weekday::Wed };
        let plist = build_launchd_plist(&spec);
        assert!(plist.contains("<key>Weekday</key>\n      <integer>3</integer>"));
    }

    #[test]
    fn a_label_with_markup_cannot_break_out_of_the_plist() {
        let mut spec = daily_spec_with_env();
        spec.label = "harmless".into();
        // The task_id is charset-restricted, so the injection vector to test
        // here is an argument (validation lets arbitrary non-control text
        // through, and it must still be XML-escaped).
        spec.args.push("</string><key>Evil</key><string>x".into());
        let plist = build_launchd_plist(&spec);
        assert!(!plist.contains("<key>Evil</key>"));
        assert!(plist.contains("&lt;key&gt;Evil&lt;/key&gt;"));
    }

    #[test]
    fn build_is_deterministic() {
        assert_eq!(
            build_launchd_plist(&daily_spec_with_env()),
            build_launchd_plist(&daily_spec_with_env())
        );
    }
}
