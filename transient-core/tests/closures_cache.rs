//! The policy of the closure cache (`SyncPolicy`): when the closures are written and which ones stay in memory, as in
//! TCache (`syncWrite`, `clearSyncCacheProc`, `defaultCheck`).

use std::sync::Arc;
use std::time::Duration;
use transient_core::*;

fn closure(i: usize, live: bool) -> LocalClosure {
    LocalClosure {
        session: "0".into(),
        clos: i.to_string(),
        prev: "0-0".into(),
        local_log: LogB::from("e/1/2/3/"),
        collected: None,
        source: None,
        evar: live.then(EVar::new),
        live,
        caller: None,
    }
}

fn dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("transient-cache-{name}-{}", std::process::id()))
}

fn files(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir).unwrap().filter(|e| e.as_ref().unwrap().path().is_file()).count()
}

fn policy(frequency: Option<Duration>) -> SyncPolicy {
    SyncPolicy { frequency, ..SyncPolicy::default() }
}

/// A closure that changes all the time (the one of a stream) is serialised once per period, not once per change
#[test]
fn a_closure_that_changes_all_the_time_is_written_once_per_period() {
    let d = dir("coalesce");
    let table = Arc::new(Closures::open_with(&d, policy(Some(Duration::from_millis(300)))));
    table.start_flusher();
    for _ in 0..5000 {
        table.write("7-0".into(), closure(7, false));
    }
    assert_eq!(table.serialised(), 0, "nothing is written on the path of the program");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(table.serialised(), 1);
    assert!(d.join("7-0").exists());
    drop(table);
    let _ = std::fs::remove_dir_all(d);
}

/// In manual mode nothing is written until `flush` (`SyncManual` and `syncCache`)
#[test]
fn in_manual_mode_the_closures_are_written_when_asked() {
    let d = dir("manual");
    let table = Arc::new(Closures::open_with(&d, policy(None)));
    table.start_flusher();
    table.write("1-0".into(), closure(1, false));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(files(&d), 0);
    table.flush();
    assert_eq!(files(&d), 1);
    // and nothing changed since: nothing is written again
    table.flush();
    assert_eq!(table.serialised(), 1);
    drop(table);
    let _ = std::fs::remove_dir_all(d);
}

/// Past `cache_size`, the cold closures that are written leave memory and come back from disk when they are needed; the
/// live ones never leave, since they are the way to their continuation
#[test]
fn the_cache_drops_the_cold_closures_and_reads_them_back() {
    let d = dir("evict");
    let always_cold = SyncPolicy { frequency: None, cache_size: 10, check: |_, _, _| true, flush_on_job: true };
    let table = Closures::open_with(&d, always_cold);
    for i in 0..100 {
        table.write(format!("{i}-0"), closure(i, i == 42));
    }
    assert_eq!(table.cached(), 100);
    table.flush();
    assert_eq!(table.cached(), 1, "only the live closure stays");
    assert!(table.read("42-0").unwrap().live);
    // a dropped closure is read from its file
    let c = table.read("7-0").unwrap();
    assert_eq!((c.clos.as_str(), c.local_log.render().as_str(), c.live), ("7", "e/1/2/3/", false));
    assert_eq!(table.cached(), 2);
    assert!(table.read("999-0").is_none());
    drop(table);
    let _ = std::fs::remove_dir_all(d);
}

/// The default check keeps what was used recently (`defaultCheck`)
#[test]
fn the_default_check_keeps_what_was_used_recently() {
    use std::time::Instant;
    let (last_sync, now) = (Instant::now(), Instant::now() + Duration::from_secs(10));
    assert!(!default_check(now, now - Duration::from_secs(1), last_sync), "used a second ago");
    assert!(default_check(now, last_sync + Duration::from_secs(1), last_sync), "not used since the first second");
}
