//! Linux cron fallback — used when no systemd user session is available.
//!
//! cron executes each entry through `/bin/sh`, so this is the most
//! injection-sensitive backend. Every token of the command is wrapped in POSIX
//! single quotes ([`sh_quote`]), which disables **all** shell interpretation
//! (expansion, globbing, word-splitting) — the only special case is an embedded
//! single quote. Combined with the injection-safety gate's rejection of control
//! characters (so a newline can never inject a second crontab line), a
//! malicious path or argument is inert.
//!
//! cron has **no missed-run catch-up** (unlike systemd's `Persistent=true` or
//! launchd's wake behaviour): a job whose time passed while the machine was off
//! is simply skipped until the next occurrence. The app-level catch-up-on-
//! launch covers that gap.
//!
//! The pure builders and the crontab block-editing logic are compiled and
//! tested on every platform; only [`imp`], which shells out to `crontab`, is
//! `#[cfg(target_os = "linux")]`.

use chrono::Timelike;

use crate::spec::{Frequency, ScheduleSpec};

/// The comment that opens our managed block in the user's crontab.
pub(crate) fn begin_marker(task_id: &str) -> String {
    format!("# BEGIN vault-scheduler:{task_id}")
}

/// The comment that closes our managed block in the user's crontab.
pub(crate) fn end_marker(task_id: &str) -> String {
    format!("# END vault-scheduler:{task_id}")
}

/// POSIX single-quote a token so `/bin/sh` treats it as one literal argument
/// with no expansion, globbing, or word-splitting. A single quote inside the
/// value is closed, backslash-escaped, and reopened (`'\''`).
pub(crate) fn sh_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Build the managed crontab block for `spec`: the begin marker, one schedule
/// line, and the end marker.
///
/// The schedule line is `minute hour * * dow <command>`, where the command is
/// the env assignments (as `KEY='value'` prefixes) followed by the
/// single-quoted program and arguments.
pub(crate) fn build_cron_block(spec: &ScheduleSpec) -> String {
    let minute = spec.time_of_day.minute();
    let hour = spec.time_of_day.hour();
    let dow = match spec.frequency {
        Frequency::Daily => "*".to_string(),
        // cron day-of-week: 0-6 with 0 = Sunday, matching num_days_from_sunday.
        Frequency::Weekly { day } => day.num_days_from_sunday().to_string(),
    };

    let mut command = String::new();
    for (key, value) in &spec.env {
        // `KEY='value' ` — sh applies the assignment to the command's env. The
        // key is charset-validated; the value is single-quoted.
        command.push_str(&format!("{key}={} ", sh_quote(value)));
    }
    command.push_str(&sh_quote(&spec.program.to_string_lossy()));
    for arg in &spec.args {
        command.push(' ');
        command.push_str(&sh_quote(arg));
    }

    format!(
        "{begin}\n{minute} {hour} * * {dow} {command}\n{end}\n",
        begin = begin_marker(spec.task_id.as_str()),
        end = end_marker(spec.task_id.as_str()),
    )
}

/// Remove our managed block (everything between and including the begin/end
/// markers) from `crontab`, leaving every other line untouched. Idempotent: a
/// crontab without our block is returned unchanged (modulo trailing newline).
pub(crate) fn strip_block(crontab: &str, task_id: &str) -> String {
    let begin = begin_marker(task_id);
    let end = end_marker(task_id);
    let mut out = String::new();
    let mut skipping = false;
    for line in crontab.lines() {
        if line == begin {
            skipping = true;
            continue;
        }
        if line == end {
            skipping = false;
            continue;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Produce the crontab that results from installing `spec` into `current`:
/// strip any existing block for this task, then append the fresh block.
pub(crate) fn upsert_block(current: &str, spec: &ScheduleSpec) -> String {
    let mut next = strip_block(current, spec.task_id.as_str());
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(&build_cron_block(spec));
    next
}

#[cfg(target_os = "linux")]
pub(crate) mod imp {
    use std::io::Write;
    use std::process::{Command, Stdio};

    use super::{begin_marker, upsert_block};
    use crate::error::{SchedulerError, SchedulerResult};
    use crate::spec::ScheduleSpec;

    /// Read the user's current crontab, or an empty string if none exists.
    fn read_crontab() -> SchedulerResult<String> {
        let output = Command::new("crontab")
            .arg("-l")
            .output()
            .map_err(SchedulerError::Io)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            // `crontab -l` exits non-zero when there is no crontab yet — that is
            // an empty crontab, not a failure.
            Ok(String::new())
        }
    }

    /// Install `content` as the user's crontab via `crontab -` (reads stdin).
    fn write_crontab(content: &str) -> SchedulerResult<()> {
        let mut child = Command::new("crontab")
            .arg("-")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(SchedulerError::Io)?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(content.as_bytes())
                .map_err(SchedulerError::Io)?;
        }
        let status = child.wait().map_err(SchedulerError::Io)?;
        if status.success() {
            Ok(())
        } else {
            Err(SchedulerError::BackendFailed(format!(
                "`crontab -` exited with {status}"
            )))
        }
    }

    pub(crate) fn register(spec: &ScheduleSpec) -> SchedulerResult<()> {
        let current = read_crontab()?;
        write_crontab(&upsert_block(&current, spec))
    }

    pub(crate) fn unregister(task_id: &str) -> SchedulerResult<()> {
        let current = read_crontab()?;
        if !current.contains(&begin_marker(task_id)) {
            return Ok(());
        }
        write_crontab(&super::strip_block(&current, task_id))
    }

    pub(crate) fn is_present(task_id: &str) -> SchedulerResult<bool> {
        Ok(read_crontab()?.contains(&begin_marker(task_id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::TaskId;
    use chrono::{NaiveTime, Weekday};
    use std::path::PathBuf;

    fn spec() -> ScheduleSpec {
        ScheduleSpec {
            task_id: TaskId::new("com.zaaheen.maintenance").unwrap(),
            label: "Zaaheen automatic maintenance".into(),
            frequency: Frequency::Daily,
            time_of_day: NaiveTime::from_hms_opt(3, 0, 0).unwrap(),
            program: PathBuf::from("/opt/Zaaheen/zaaheen"),
            args: vec!["consolidate".into(), "run".into()],
            env: vec![("LANCE_MEM_POOL_SIZE".into(), "268435456".into())],
        }
    }

    #[test]
    fn sh_quote_wraps_and_neutralises_shell_metacharacters() {
        assert_eq!(sh_quote("simple"), "'simple'");
        assert_eq!(sh_quote("a b"), "'a b'");
        // A single quote is closed/escaped/reopened.
        assert_eq!(sh_quote("a'b"), "'a'\\''b'");
        // Dangerous shell text is fully single-quoted, so it is inert.
        assert_eq!(sh_quote("$(rm -rf ~)"), "'$(rm -rf ~)'");
        assert_eq!(sh_quote("a; rm -rf /"), "'a; rm -rf /'");
        assert_eq!(sh_quote("`whoami`"), "'`whoami`'");
    }

    #[test]
    fn daily_block_has_markers_schedule_env_and_quoted_command() {
        let block = build_cron_block(&spec());
        assert!(block.contains("# BEGIN vault-scheduler:com.zaaheen.maintenance"));
        assert!(block.contains("# END vault-scheduler:com.zaaheen.maintenance"));
        assert!(block.contains("0 3 * * * "));
        assert!(block.contains("LANCE_MEM_POOL_SIZE='268435456' "));
        // Program path has a space, so it is single-quoted as one token.
        assert!(block.contains("'/opt/Zaaheen/zaaheen' 'consolidate' 'run'"));
    }

    #[test]
    fn weekly_block_sets_the_day_of_week_field() {
        let mut spec = spec();
        spec.frequency = Frequency::Weekly { day: Weekday::Sun };
        assert!(build_cron_block(&spec).contains("0 3 * * 0 "));
        spec.frequency = Frequency::Weekly { day: Weekday::Wed };
        assert!(build_cron_block(&spec).contains("0 3 * * 3 "));
    }

    #[test]
    fn a_malicious_argument_cannot_break_out_of_the_command() {
        let mut spec = spec();
        spec.args.push("; rm -rf $HOME".into());
        let block = build_cron_block(&spec);
        // The payload appears only inside single quotes — never as live shell.
        assert!(block.contains("'; rm -rf $HOME'"));
    }

    #[test]
    fn strip_block_removes_only_our_block() {
        let crontab = "\
# a user's own job
0 9 * * * /usr/bin/backup
# BEGIN vault-scheduler:com.zaaheen.maintenance
0 3 * * * '/opt/zaaheen' 'run'
# END vault-scheduler:com.zaaheen.maintenance
30 8 * * * /usr/bin/other
";
        let stripped = strip_block(crontab, "com.zaaheen.maintenance");
        assert!(stripped.contains("/usr/bin/backup"));
        assert!(stripped.contains("/usr/bin/other"));
        assert!(!stripped.contains("vault-scheduler"));
        assert!(!stripped.contains("zaaheen"));
    }

    #[test]
    fn upsert_replaces_an_existing_block_rather_than_duplicating() {
        let once = upsert_block("", &spec());
        let twice = upsert_block(&once, &spec());
        // Re-installing must not stack a second copy.
        assert_eq!(twice.matches("# BEGIN vault-scheduler").count(), 1);
        // A pre-existing user job survives the upsert.
        let with_user = upsert_block("0 9 * * * /usr/bin/backup\n", &spec());
        assert!(with_user.contains("/usr/bin/backup"));
        assert_eq!(with_user.matches("# BEGIN vault-scheduler").count(), 1);
    }
}
