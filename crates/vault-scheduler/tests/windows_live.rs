//! Live round-trip against the real Windows Task Scheduler.
//!
//! `#[ignore]` + `#![cfg(windows)]`: this test registers, queries, and removes
//! a REAL per-user scheduled task via `schtasks`, so it never runs in ordinary
//! CI (it compiles to nothing off Windows, and is skipped unless `--ignored`).
//! It is the runtime confirmation for the Windows backend — the executable
//! proof that `build_task_xml` produces XML Task Scheduler accepts and that
//! register/status/unregister behave against the live OS.
//!
//! Run explicitly:
//! `cargo test -p vault-scheduler --test windows_live -- --ignored --nocapture`
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use chrono::NaiveTime;
use vault_scheduler::{platform_scheduler, Frequency, ScheduleSpec, TaskId};

#[test]
#[ignore = "registers a real Windows scheduled task; run explicitly with --ignored"]
fn windows_scheduler_round_trip() {
    let scheduler = platform_scheduler().expect("a Windows backend on this target");
    let task_id = TaskId::new("com.memoryvault.livetest").unwrap();

    // A harmless command: cmd.exe /c exit. We register then delete it; it is
    // scheduled for 03:00 and never actually fires during the test.
    let spec = ScheduleSpec {
        task_id: task_id.clone(),
        label: "Memory Vault live test (safe to delete)".into(),
        frequency: Frequency::Daily,
        time_of_day: NaiveTime::from_hms_opt(3, 0, 0).unwrap(),
        program: PathBuf::from(r"C:\Windows\System32\cmd.exe"),
        args: vec!["/c".into(), "exit".into()],
        env: vec![],
    };

    // Clean slate — unregister is idempotent, so this succeeds even if absent.
    scheduler
        .unregister(&task_id)
        .expect("clean-slate unregister");
    assert!(
        !scheduler.status(&task_id).unwrap().registered,
        "precondition: the test task must not exist before we register it"
    );

    // Register, and confirm the OS reports it.
    scheduler.register(&spec).expect("register the task");
    assert!(
        scheduler.status(&task_id).unwrap().registered,
        "after register, the OS should report the task as registered"
    );

    // Print Task Scheduler's own view so a human can eyeball the real task
    // (next run time, run level, command) under --nocapture.
    let query = Command::new("schtasks")
        .args(["/Query", "/TN", task_id.as_str(), "/FO", "LIST", "/V"])
        .output()
        .expect("schtasks query");
    eprintln!(
        "\n----- Task Scheduler's view of the registered task -----\n{}\n--------------------------------------------------------",
        String::from_utf8_lossy(&query.stdout)
    );

    // Remove it, and confirm it is gone.
    scheduler.unregister(&task_id).expect("unregister the task");
    assert!(
        !scheduler.status(&task_id).unwrap().registered,
        "after unregister, the OS should no longer report the task"
    );
}
