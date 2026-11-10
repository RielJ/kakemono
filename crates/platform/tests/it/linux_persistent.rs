//! `linux::backends::persistent` (Task 30). Runs on every Unix: the supervisor logic goes through
//! a fake `ProcessControl`, and the real-process tests use `tests/bin/fake-swaybg` with
//! `NativeProcesses` (`/proc` + nix on Linux; test-only `std::process::Child` plumbing elsewhere).
//! Tests marked `linux only` exercise `/proc` lookups of processes this control did not spawn.
#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kakemono_config::schema::Fit;
use kakemono_platform::api::{CommandStatus, SetError, WallpaperSetter};
use kakemono_platform::linux::backends::command::Template;
use kakemono_platform::linux::backends::persistent::{
    Health, NativeProcesses, PID_FILE, PersistentSetter, ProcessControl, Record, SETTLE,
    STABLE_AFTER, Supervisor, backoff, read_record, write_record,
};
use kakemono_platform::linux::detect::resolve::BackendKind;
use tokio::time::Instant;

// ---------------------------------------------------------------------------------------------
// Fake process table
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum Event {
    Spawn {
        pid: u32,
        argv0: String,
        at: Duration,
    },
    Terminate {
        pid: u32,
        at: Duration,
    },
}

#[derive(Debug, Default)]
struct FakeState {
    next_pid: u32,
    alive: HashMap<u32, String>,
    events: Vec<Event>,
    /// The next spawned process exits immediately.
    die_next: bool,
    /// The next spawn fails (binary missing).
    fail_next: bool,
}

#[derive(Debug)]
struct FakeProcesses {
    start: Instant,
    state: Mutex<FakeState>,
}

impl FakeProcesses {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            start: Instant::now(),
            state: Mutex::new(FakeState {
                next_pid: 100,
                ..FakeState::default()
            }),
        })
    }

    fn st(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap()
    }

    fn events(&self) -> Vec<Event> {
        self.st().events.clone()
    }

    fn alive(&self) -> Vec<u32> {
        let mut pids: Vec<u32> = self.st().alive.keys().copied().collect();
        pids.sort_unstable();
        pids
    }

    /// The process crashes on its own.
    fn crash(&self, pid: u32) {
        self.st().alive.remove(&pid);
    }

    /// Another program now has `pid`.
    fn reuse(&self, pid: u32, comm: &str) {
        self.st().alive.insert(pid, comm.to_owned());
    }

    /// A process we did not spawn.
    fn foreign(&self, pid: u32, comm: &str) {
        self.st().alive.insert(pid, comm.to_owned());
    }

    fn terminated(&self) -> Vec<u32> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                Event::Terminate { pid, .. } => Some(pid),
                Event::Spawn { .. } => None,
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl ProcessControl for FakeProcesses {
    fn spawn(&self, argv: &[String]) -> std::io::Result<u32> {
        let at = self.start.elapsed();
        let mut st = self.st();
        if std::mem::take(&mut st.fail_next) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No such file or directory",
            ));
        }
        st.next_pid += 1;
        let pid = st.next_pid;
        let argv0 = argv[0].clone();
        st.events.push(Event::Spawn {
            pid,
            argv0: argv0.clone(),
            at,
        });
        if !std::mem::take(&mut st.die_next) {
            let name = Path::new(&argv0)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            st.alive.insert(pid, name);
        }
        Ok(pid)
    }

    fn comm(&self, pid: u32) -> Option<String> {
        self.st().alive.get(&pid).cloned()
    }

    async fn terminate(&self, pid: u32, _grace: Duration) {
        let at = self.start.elapsed();
        let mut st = self.st();
        st.events.push(Event::Terminate { pid, at });
        st.alive.remove(&pid);
    }
}

fn swaybg(image: &str) -> Vec<String> {
    ["/usr/bin/swaybg", "-i", image, "-m", "fill"]
        .map(String::from)
        .to_vec()
}

fn sup(fake: &Arc<FakeProcesses>, dir: &Path) -> Supervisor {
    Supervisor::new(Arc::clone(fake) as Arc<dyn ProcessControl>, dir)
}

// ---------------------------------------------------------------------------------------------
// Supervisor logic (fake control, paused clock): runs on every OS
// ---------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn persistent_replace_spawns_new_then_terminates_old_after_settle() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeProcesses::new();
    let sup = sup(&fake, dir.path());

    let first = sup.replace(swaybg("/a.jpg")).await.unwrap();
    let second = sup.replace(swaybg("/b.jpg")).await.unwrap();

    assert_eq!(
        fake.events(),
        vec![
            Event::Spawn {
                pid: first,
                argv0: "/usr/bin/swaybg".into(),
                at: Duration::ZERO
            },
            Event::Spawn {
                pid: second,
                argv0: "/usr/bin/swaybg".into(),
                at: SETTLE
            },
            // The old one goes only after the new one has been up for SETTLE: no black flash.
            Event::Terminate {
                pid: first,
                at: SETTLE * 2
            },
        ]
    );
    assert_eq!(fake.alive(), vec![second]);
    let record = read_record(&dir.path().join(PID_FILE)).unwrap();
    assert_eq!(
        record,
        Record {
            pid: second,
            argv: swaybg("/b.jpg")
        }
    );
    assert_eq!(sup.current().await, Some(record));
}

#[tokio::test(start_paused = true)]
async fn persistent_new_process_dying_during_settle_keeps_old() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeProcesses::new();
    let sup = sup(&fake, dir.path());
    let first = sup.replace(swaybg("/a.jpg")).await.unwrap();

    fake.st().die_next = true;
    let err = sup.replace(swaybg("/bad.jpg")).await.unwrap_err();
    match err {
        SetError::Command {
            argv0,
            status,
            stderr,
        } => {
            assert_eq!(argv0, "/usr/bin/swaybg");
            assert_eq!(status, CommandStatus::FailedToStart);
            assert!(stderr.contains("exited within 300 ms"), "{stderr}");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(fake.terminated().is_empty(), "old setter must stay up");
    assert_eq!(fake.alive(), vec![first]);
    assert_eq!(read_record(&dir.path().join(PID_FILE)).unwrap().pid, first);
    assert_eq!(sup.current().await.unwrap().pid, first);
}

#[tokio::test(start_paused = true)]
async fn persistent_spawn_failure_is_failed_to_start() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeProcesses::new();
    let sup = sup(&fake, dir.path());
    fake.st().fail_next = true;
    let err = sup.replace(swaybg("/a.jpg")).await.unwrap_err();
    assert!(
        matches!(&err, SetError::Command { argv0, status: CommandStatus::FailedToStart, .. } if argv0 == "/usr/bin/swaybg"),
        "{err:?}"
    );
    assert!(!dir.path().join(PID_FILE).exists());
    assert_eq!(sup.current().await, None);
}

#[tokio::test(start_paused = true)]
async fn persistent_empty_argv_is_unconfigured() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeProcesses::new();
    let err = sup(&fake, dir.path())
        .replace(Vec::new())
        .await
        .unwrap_err();
    assert!(matches!(err, SetError::Unconfigured { .. }), "{err:?}");
}

#[tokio::test(start_paused = true)]
async fn persistent_reused_pid_is_not_killed() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeProcesses::new();
    let sup = sup(&fake, dir.path());
    let first = sup.replace(swaybg("/a.jpg")).await.unwrap();

    // swaybg died and its PID went to an unrelated program.
    fake.reuse(first, "firefox");
    let second = sup.replace(swaybg("/b.jpg")).await.unwrap();

    assert!(fake.terminated().is_empty());
    assert_eq!(fake.alive(), vec![first, second]);
}

#[tokio::test(start_paused = true)]
async fn persistent_adopts_recorded_setter_on_start() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeProcesses::new();
    fake.foreign(42, "swaybg");
    write_record(
        &dir.path().join(PID_FILE),
        &Record {
            pid: 42,
            argv: swaybg("/old.jpg"),
        },
    )
    .unwrap();

    let sup = sup(&fake, dir.path());
    assert_eq!(sup.current().await.unwrap().pid, 42);

    let new = sup.replace(swaybg("/new.jpg")).await.unwrap();
    assert_eq!(fake.terminated(), vec![42]);
    assert_eq!(fake.alive(), vec![new]);
}

#[tokio::test(start_paused = true)]
async fn persistent_does_not_adopt_mismatched_or_gone_pid() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join(PID_FILE);
    let fake = FakeProcesses::new();

    // PID reused by another program.
    fake.foreign(42, "bash");
    write_record(
        &pid_file,
        &Record {
            pid: 42,
            argv: swaybg("/old.jpg"),
        },
    )
    .unwrap();
    let sup1 = sup(&fake, dir.path());
    assert_eq!(sup1.current().await, None);
    sup1.replace(swaybg("/new.jpg")).await.unwrap();
    assert!(
        fake.terminated().is_empty(),
        "bash (pid 42) must not be signalled"
    );
    assert!(fake.alive().contains(&42));

    // PID gone.
    write_record(
        &pid_file,
        &Record {
            pid: 77,
            argv: swaybg("/old.jpg"),
        },
    )
    .unwrap();
    assert_eq!(sup(&fake, dir.path()).current().await, None);

    // Garbage.
    std::fs::write(&pid_file, "12345\n").unwrap();
    assert_eq!(sup(&fake, dir.path()).current().await, None);
}

#[tokio::test(start_paused = true)]
async fn persistent_restarts_dead_setter_with_backoff() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeProcesses::new();
    let sup = sup(&fake, dir.path());
    assert_eq!(sup.ensure_running().await, Health::Idle);

    let first = sup.replace(swaybg("/a.jpg")).await.unwrap();
    assert_eq!(sup.ensure_running().await, Health::Running { pid: first });

    // First death: restart at once.
    fake.crash(first);
    let Health::Restarted {
        pid: p1,
        attempt: 1,
    } = sup.ensure_running().await
    else {
        panic!("expected restart 1");
    };
    assert_eq!(
        read_record(&dir.path().join(PID_FILE)).unwrap(),
        Record {
            pid: p1,
            argv: swaybg("/a.jpg")
        }
    );

    // Dies again at once: wait backoff(1) = 1 s.
    fake.crash(p1);
    let t0 = Instant::now();
    assert_eq!(
        sup.ensure_running().await,
        Health::Waiting {
            until: t0 + backoff(1)
        }
    );
    tokio::time::advance(backoff(1) - Duration::from_millis(1)).await;
    assert!(matches!(sup.ensure_running().await, Health::Waiting { .. }));
    tokio::time::advance(Duration::from_millis(1)).await;
    let Health::Restarted {
        pid: p2,
        attempt: 2,
    } = sup.ensure_running().await
    else {
        panic!("expected restart 2");
    };

    // Again: wait backoff(2) = 2 s.
    fake.crash(p2);
    tokio::time::advance(backoff(2) - Duration::from_millis(1)).await;
    assert!(matches!(sup.ensure_running().await, Health::Waiting { .. }));
    tokio::time::advance(Duration::from_millis(1)).await;
    let Health::Restarted {
        pid: p3,
        attempt: 3,
    } = sup.ensure_running().await
    else {
        panic!("expected restart 3");
    };

    // Stays up for STABLE_AFTER: backoff resets, the next death restarts at once as attempt 1.
    tokio::time::advance(STABLE_AFTER).await;
    assert_eq!(sup.ensure_running().await, Health::Running { pid: p3 });
    fake.crash(p3);
    assert!(matches!(
        sup.ensure_running().await,
        Health::Restarted { attempt: 1, .. }
    ));

    // Nothing was ever signalled: restarts only replace dead processes.
    assert!(fake.terminated().is_empty());
}

#[tokio::test(start_paused = true)]
async fn persistent_failed_restart_is_retried_after_backoff() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeProcesses::new();
    let sup = sup(&fake, dir.path());
    let first = sup.replace(swaybg("/a.jpg")).await.unwrap();
    fake.crash(first);
    fake.st().fail_next = true;
    assert!(matches!(
        sup.ensure_running().await,
        Health::RestartFailed { attempt: 1, .. }
    ));
    assert!(matches!(sup.ensure_running().await, Health::Waiting { .. }));
    tokio::time::advance(backoff(1)).await;
    assert!(matches!(
        sup.ensure_running().await,
        Health::Restarted { attempt: 2, .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn persistent_replace_resets_backoff() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeProcesses::new();
    let sup = sup(&fake, dir.path());
    let first = sup.replace(swaybg("/a.jpg")).await.unwrap();
    fake.crash(first);
    let Health::Restarted { pid, .. } = sup.ensure_running().await else {
        panic!()
    };
    fake.crash(pid);
    assert!(matches!(sup.ensure_running().await, Health::Waiting { .. }));

    let fresh = sup.replace(swaybg("/b.jpg")).await.unwrap();
    fake.crash(fresh);
    assert!(matches!(
        sup.ensure_running().await,
        Health::Restarted { attempt: 1, .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn persistent_watch_restarts_in_background() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeProcesses::new();
    let sup = Arc::new(sup(&fake, dir.path()));
    let first = sup.replace(swaybg("/a.jpg")).await.unwrap();
    let watcher = sup.watch(Duration::from_secs(2));

    fake.crash(first);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let current = sup.current().await.unwrap();
    assert_ne!(current.pid, first);
    assert_eq!(fake.alive(), vec![current.pid]);
    watcher.abort();
}

#[tokio::test(start_paused = true)]
async fn persistent_stop_terminates_and_removes_pid_file() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeProcesses::new();
    let sup = sup(&fake, dir.path());
    let first = sup.replace(swaybg("/a.jpg")).await.unwrap();
    sup.stop().await;
    assert_eq!(fake.terminated(), vec![first]);
    assert!(fake.alive().is_empty());
    assert!(!dir.path().join(PID_FILE).exists());
    assert_eq!(sup.ensure_running().await, Health::Idle);
}

#[tokio::test(start_paused = true)]
async fn persistent_setter_renders_preset_and_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeProcesses::new();
    let sup = Arc::new(sup(&fake, dir.path()));
    let setter = PersistentSetter::new(
        "swaybg",
        Template::Preset(BackendKind::Swaybg),
        Arc::clone(&sup),
    )
    .unwrap();

    let image = PathBuf::from("/home/u/Pictures/a b.jpg");
    setter.set(&image, Fit::Center).await.unwrap();
    let record = sup.current().await.unwrap();
    assert_eq!(
        record.argv,
        ["swaybg", "-i", "/home/u/Pictures/a b.jpg", "-m", "center"]
    );
    assert_eq!(setter.info().name, "swaybg");
    assert!(
        setter.info().detail.starts_with("swaybg -i {path} -m fill"),
        "{}",
        setter.info().detail
    );

    setter.set(&image, Fit::Fill).await.unwrap();
    assert_eq!(fake.terminated(), vec![record.pid]);

    // Relative paths are refused before anything is spawned.
    let before = fake.events().len();
    assert!(setter.set(Path::new("rel.jpg"), Fit::Fill).await.is_err());
    assert_eq!(fake.events().len(), before);
}

// ---------------------------------------------------------------------------------------------
// Real processes (tests/bin/fake-swaybg, NativeProcesses)
// ---------------------------------------------------------------------------------------------

fn fake_swaybg() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/bin/fake-swaybg")
        .to_string_lossy()
        .into_owned()
}

fn fake_argv(image: &str) -> Vec<String> {
    vec![
        fake_swaybg(),
        "-i".into(),
        image.into(),
        "-m".into(),
        "fill".into(),
    ]
}

/// Wait (real time) until `pred` holds, up to 5 s.
async fn eventually(mut pred: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if pred() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    pred()
}

#[tokio::test]
async fn persistent_real_two_sets_leave_only_the_second_alive() {
    let dir = tempfile::tempdir().unwrap();
    let control: Arc<NativeProcesses> = Arc::new(NativeProcesses::new());
    let sup = Supervisor::new(Arc::clone(&control) as Arc<dyn ProcessControl>, dir.path())
        .with_grace(Duration::from_secs(2));

    let first = sup.replace(fake_argv("/a.jpg")).await.unwrap();
    assert_eq!(control.comm(first).as_deref(), Some("fake-swaybg"));
    let second = sup.replace(fake_argv("/b.jpg")).await.unwrap();

    assert!(
        eventually(|| control.comm(first).is_none()).await,
        "first setter still running"
    );
    assert_eq!(control.comm(second).as_deref(), Some("fake-swaybg"));
    assert_eq!(read_record(&dir.path().join(PID_FILE)).unwrap().pid, second);

    sup.stop().await;
    assert!(
        eventually(|| control.comm(second).is_none()).await,
        "second setter still running"
    );
}

#[tokio::test]
async fn persistent_real_setter_dying_on_start_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    // A long settle: under a loaded test run, `sh` can take a while to start and exit.
    let sup = Supervisor::new(Arc::new(NativeProcesses::new()), dir.path())
        .with_settle(Duration::from_secs(3));
    let err = sup
        .replace(vec![fake_swaybg(), "--die".into()])
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            SetError::Command {
                status: CommandStatus::FailedToStart,
                ..
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn persistent_real_missing_binary_is_failed_to_start() {
    let dir = tempfile::tempdir().unwrap();
    let sup = Supervisor::new(Arc::new(NativeProcesses::new()), dir.path());
    let err = sup
        .replace(vec!["/nonexistent/kakemono-swaybg".into()])
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            SetError::Command {
                status: CommandStatus::FailedToStart,
                ..
            }
        ),
        "{err:?}"
    );
}

/// A recorded PID whose name doesn't match (here: a plain `sleep`) is neither adopted nor killed.
#[tokio::test]
async fn persistent_real_mismatched_pid_is_not_killed() {
    let dir = tempfile::tempdir().unwrap();
    let mut foreign = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    write_record(
        &dir.path().join(PID_FILE),
        &Record {
            pid: foreign.id(),
            argv: fake_argv("/old.jpg"),
        },
    )
    .unwrap();

    let control = Arc::new(NativeProcesses::new());
    let sup = Supervisor::new(Arc::clone(&control) as Arc<dyn ProcessControl>, dir.path());
    assert_eq!(sup.current().await, None);
    let new = sup.replace(fake_argv("/new.jpg")).await.unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        foreign.try_wait().unwrap().is_none(),
        "foreign process was killed"
    );
    sup.stop().await;
    assert!(eventually(|| control.comm(new).is_none()).await);
    foreign.kill().unwrap();
    foreign.wait().unwrap();
}

/// Daemon restart: a fresh supervisor and a fresh `NativeProcesses` find the running setter
/// through `setter.pid` and `/proc`, and replace it. Linux only: the non-Linux test plumbing
/// can only see processes it spawned itself.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn persistent_real_adopts_after_daemon_restart() {
    let dir = tempfile::tempdir().unwrap();
    let first = {
        let sup = Supervisor::new(Arc::new(NativeProcesses::new()), dir.path());
        sup.replace(fake_argv("/a.jpg")).await.unwrap()
    };
    let control = Arc::new(NativeProcesses::new());
    assert_eq!(control.comm(first).as_deref(), Some("fake-swaybg"));

    let sup = Supervisor::new(Arc::clone(&control) as Arc<dyn ProcessControl>, dir.path());
    assert_eq!(sup.current().await.map(|r| r.pid), Some(first));
    let second = sup.replace(fake_argv("/b.jpg")).await.unwrap();
    assert!(
        eventually(|| control.comm(first).is_none()).await,
        "adopted setter still running"
    );
    sup.stop().await;
    assert!(eventually(|| control.comm(second).is_none()).await);
}

/// SIGTERM goes to the setter's whole process group, so the script's `sleep` child dies too.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn persistent_real_terminate_signals_the_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let control = Arc::new(NativeProcesses::new());
    let sup = Supervisor::new(Arc::clone(&control) as Arc<dyn ProcessControl>, dir.path());
    let pid = sup.replace(fake_argv("/a.jpg")).await.unwrap();
    let group_members = || -> Vec<u32> {
        std::fs::read_dir("/proc")
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
            .filter(|p| {
                std::fs::read_to_string(format!("/proc/{p}/stat")).is_ok_and(|s| {
                    let rest = &s[s.rfind(')').unwrap() + 2..];
                    let fields: Vec<&str> = rest.split_whitespace().collect();
                    // fields: state ppid pgrp ...; skip zombies.
                    fields[0] != "Z" && fields[2] == pid.to_string()
                })
            })
            .collect()
    };
    assert!(
        eventually(|| group_members().len() >= 2).await,
        "expected sh + sleep in the group"
    );
    sup.stop().await;
    assert!(
        eventually(|| group_members().is_empty()).await,
        "group survivors: {:?}",
        group_members()
    );
}
