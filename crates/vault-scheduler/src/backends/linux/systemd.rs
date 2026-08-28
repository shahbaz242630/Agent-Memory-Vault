//! Linux systemd backend — a per-user `.service` + `.timer` pair.
//!
//! This is the preferred Linux mechanism (the cron sibling is the fallback for
//! systemd-less hosts). Two units are written to
//! `~/.config/systemd/user/`: a `oneshot` service that runs the command, and a
//! timer that triggers it `OnCalendar`. Because they are **user** units
//! (`systemctl --user`), they run as the logged-in user with no elevation.
//!
//! `Persistent=true` on the timer is the missed-run catch-up: a run whose time
//! passed while the machine was off happens at the next login/boot.
//!
//! Escaping (ADR-SEC-005): `%` is a systemd specifier and `$` triggers
//! environment expansion in `ExecStart`, so both are doubled; tokens with
//! whitespace are double-quoted with `\` and `"` escaped. Environment values
//! are placed inside `Environment="KEY=VALUE"` with the same quote-safe
//! escaping. Control characters are already rejected by the injection-safety
//! gate, so a newline can never open a spurious directive.
//!
//! The pure builders are compiled and tested on every platform; only [`imp`],
//! which shells out to `systemctl`, is `#[cfg(target_os = "linux")]`.

use chrono::{Timelike, Weekday};

use crate::spec::{Frequency, ScheduleSpec};

/// Double `%` (specifier escape) — needed wherever a value appears in a unit
/// file, since `%x` sequences are always expanded by systemd.
fn escape_specifiers(s: &str) -> String {
    s.replace('%', "%%")
}

/// Quote a single `ExecStart` token: double `%` and `$` (so neither a specifier
/// nor an env-var expansion fires), then, if the token has whitespace or a
/// quote/backslash, wrap it in double quotes with `\` and `"` escaped.
fn exec_token(s: &str) -> String {
    let escaped: String = s
        .chars()
        .flat_map(|c| match c {
            '%' => vec!['%', '%'],
            '$' => vec!['$', '$'],
            other => vec![other],
        })
        .collect();
    if !escaped.is_empty() && !escaped.contains([' ', '\t', '"', '\\']) {
        return escaped;
    }
    let mut out = String::from('"');
    for c in escaped.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Escape an `Environment="KEY=VALUE"` value: double `%`, then escape the `\`
/// and `"` that would otherwise close the quoted assignment.
fn env_value(s: &str) -> String {
    escape_specifiers(s)
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

/// The systemd `OnCalendar` expression for `spec`.
fn on_calendar(spec: &ScheduleSpec) -> String {
    let (h, m) = (spec.time_of_day.hour(), spec.time_of_day.minute());
    match spec.frequency {
        Frequency::Daily => format!("*-*-* {h:02}:{m:02}:00"),
        Frequency::Weekly { day } => format!("{} *-*-* {h:02}:{m:02}:00", weekday_abbrev(day)),
    }
}

/// systemd day-of-week abbreviation.
fn weekday_abbrev(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

/// Build the `.service` unit for `spec` (a `oneshot` that runs the command).
pub(crate) fn build_service_unit(spec: &ScheduleSpec) -> String {
    let mut exec = exec_token(&spec.program.to_string_lossy());
    for arg in &spec.args {
        exec.push(' ');
        exec.push_str(&exec_token(arg));
    }

    let mut environment = String::new();
    for (key, value) in &spec.env {
        environment.push_str(&format!("Environment=\"{key}={}\"\n", env_value(value)));
    }

    format!(
        "[Unit]\n\
         Description={description}\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={exec}\n\
         {environment}",
        description = escape_specifiers(&spec.label),
    )
}

/// Build the `.timer` unit for `spec` (`OnCalendar` + `Persistent=true`).
pub(crate) fn build_timer_unit(spec: &ScheduleSpec) -> String {
    format!(
        "[Unit]\n\
         Description={description} (schedule)\n\
         \n\
         [Timer]\n\
         OnCalendar={on_calendar}\n\
         Persistent=true\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n",
        description = escape_specifiers(&spec.label),
        on_calendar = on_calendar(spec),
    )
}

#[cfg(target_os = "linux")]
pub(crate) mod imp {
    use std::path::PathBuf;
    use std::process::Command;

    use super::{build_service_unit, build_timer_unit};
    use crate::error::{SchedulerError, SchedulerResult};
    use crate::spec::ScheduleSpec;

    /// `true` if a systemd user manager is reachable (a user D-Bus session
    /// exists). `systemctl --user show-environment` succeeds only then.
    pub(crate) fn available() -> bool {
        Command::new("systemctl")
            .args(["--user", "show-environment"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn unit_dir() -> SchedulerResult<PathBuf> {
        let home = std::env::var("HOME")
            .map_err(|_| SchedulerError::BackendFailed("HOME is not set".into()))?;
        let mut path = PathBuf::from(home);
        path.push(".config");
        path.push("systemd");
        path.push("user");
        Ok(path)
    }

    fn systemctl(args: &[&str]) -> SchedulerResult<std::process::Output> {
        Command::new("systemctl")
            .arg("--user")
            .args(args)
            .output()
            .map_err(SchedulerError::Io)
    }

    pub(crate) fn register(spec: &ScheduleSpec) -> SchedulerResult<()> {
        let dir = unit_dir()?;
        std::fs::create_dir_all(&dir).map_err(SchedulerError::Io)?;
        let id = spec.task_id.as_str();
        std::fs::write(dir.join(format!("{id}.service")), build_service_unit(spec))
            .map_err(SchedulerError::Io)?;
        std::fs::write(dir.join(format!("{id}.timer")), build_timer_unit(spec))
            .map_err(SchedulerError::Io)?;

        let _ = systemctl(&["daemon-reload"])?;
        let timer = format!("{id}.timer");
        let output = systemctl(&["enable", "--now", &timer])?;
        if !output.status.success() {
            return Err(SchedulerError::BackendFailed(format!(
                "systemctl --user enable --now {timer} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    pub(crate) fn unregister(task_id: &str) -> SchedulerResult<()> {
        let timer = format!("{task_id}.timer");
        // Best-effort disable; it fails harmlessly if not enabled.
        let _ = systemctl(&["disable", "--now", &timer]);

        let dir = unit_dir()?;
        for suffix in ["service", "timer"] {
            let path = dir.join(format!("{task_id}.{suffix}"));
            if path.exists() {
                std::fs::remove_file(&path).map_err(SchedulerError::Io)?;
            }
        }
        let _ = systemctl(&["daemon-reload"]);
        Ok(())
    }

    pub(crate) fn is_enabled(task_id: &str) -> SchedulerResult<bool> {
        let timer = format!("{task_id}.timer");
        Ok(systemctl(&["is-enabled", &timer])?.status.success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::TaskId;
    use chrono::NaiveTime;
    use std::path::PathBuf;

    fn spec() -> ScheduleSpec {
        ScheduleSpec {
            task_id: TaskId::new("com.zaaheen.maintenance").unwrap(),
            label: "Zaaheen automatic maintenance".into(),
            frequency: Frequency::Daily,
            time_of_day: NaiveTime::from_hms_opt(3, 5, 0).unwrap(),
            program: PathBuf::from("/opt/Zaaheen/zaaheen"),
            args: vec!["consolidate".into(), "run".into()],
            env: vec![("LANCE_MEM_POOL_SIZE".into(), "268435456".into())],
        }
    }

    #[test]
    fn service_unit_is_oneshot_with_quoted_exec_and_env() {
        let unit = build_service_unit(&spec());
        assert!(unit.contains("Type=oneshot"));
        // The realistic install path has no whitespace, so `exec_token` leaves
        // it BARE. The quoted case is covered by
        // `a_program_path_containing_a_space_stays_one_token` below.
        assert!(unit.contains("ExecStart=/opt/Zaaheen/zaaheen consolidate run"));
        assert!(unit.contains("Environment=\"LANCE_MEM_POOL_SIZE=268435456\""));
    }

    /// A program path containing a space must survive as ONE token.
    ///
    /// This test exists because the Zaaheen rename nearly deleted the property
    /// silently. The old fixture path was `/opt/Memory Vault/vault-cli`, whose
    /// space was the entire reason the assertion above checked for quoting.
    /// Renaming it to `/opt/Zaaheen/zaaheen` removed the space, so the quoting
    /// rule stopped being exercised — and the tempting fix was to drop the
    /// quotes from the assertion, which would have left it passing while
    /// testing nothing.
    ///
    /// The realistic install path has no space, so the coverage moves here
    /// rather than being contrived back into the main fixture. On Windows the
    /// equivalent case is still covered naturally, because `C:\Program Files`
    /// has a space of its own.
    #[test]
    fn a_program_path_containing_a_space_stays_one_token() {
        let mut spaced = spec();
        spaced.program = PathBuf::from("/opt/Zaaheen Beta/zaaheen");
        let unit = build_service_unit(&spaced);

        assert!(
            unit.contains("ExecStart=\"/opt/Zaaheen Beta/zaaheen\" consolidate run"),
            "a spaced program path must be quoted as a single token; got:\n{unit}"
        );
    }

    #[test]
    fn timer_unit_has_daily_calendar_persistence_and_install() {
        let unit = build_timer_unit(&spec());
        assert!(unit.contains("OnCalendar=*-*-* 03:05:00"));
        // Persistent=true is the missed-run catch-up.
        assert!(unit.contains("Persistent=true"));
        assert!(unit.contains("WantedBy=timers.target"));
    }

    #[test]
    fn weekly_timer_names_the_day() {
        let mut spec = spec();
        spec.frequency = Frequency::Weekly { day: Weekday::Sun };
        assert!(build_timer_unit(&spec).contains("OnCalendar=Sun *-*-* 03:05:00"));
    }

    #[test]
    fn exec_token_doubles_specifiers_and_expansion() {
        // % is a systemd specifier; $ triggers env expansion. Both are doubled
        // so a path containing them is passed literally.
        assert_eq!(exec_token("a%b"), "a%%b");
        assert_eq!(exec_token("a$b"), "a$$b");
        // Whitespace forces double-quoting.
        assert_eq!(exec_token("a b"), "\"a b\"");
    }

    #[test]
    fn a_malicious_argument_cannot_open_a_new_directive_or_expand() {
        let mut spec = spec();
        spec.args.push("$HOME %n".into());
        let unit = build_service_unit(&spec);
        // The whole token is doubled ($ -> $$, % -> %%) and quoted, so systemd
        // treats it as a literal rather than an env expansion or a specifier.
        assert!(unit.contains("\"$$HOME %%n\""));
    }

    #[test]
    fn builds_are_deterministic() {
        assert_eq!(build_service_unit(&spec()), build_service_unit(&spec()));
        assert_eq!(build_timer_unit(&spec()), build_timer_unit(&spec()));
    }
}
