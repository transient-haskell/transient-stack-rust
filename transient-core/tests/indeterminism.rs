//! `for_`, `choose`, `group`, `group_by_time`, `sync`, `sync1`, `timeout`. Each test reproduces an experiment that was
//! run against the original in Haskell (Exp7.hs); the expected results are the ones the original gave.

use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};
use transient_core::*;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

type Log = Arc<Mutex<Vec<String>>>;

fn say(log: &Log, s: impl Into<String>) -> Trans<()> {
    let (log, s) = (log.clone(), s.into());
    lift_io(move || log.lock().unwrap().push(s.clone()))
}

fn lines(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

// ---- for_

#[test]
fn f1_for_runs_the_rest_in_this_thread_in_order() {
    let log = Log::default();
    let ids: Arc<Mutex<Vec<thread::ThreadId>>> = Arc::default();
    let i2 = ids.clone();
    let t = tdo! { [log, i2]
        let x = for_(vec![1, 2, 3]);
        lift_io(move || i2.lock().unwrap().push(thread::current().id()));
        say(&log, format!("for {x}"))
    };
    assert_eq!(keep_all(t).len(), 3);
    assert_eq!(lines(&log), ["for 1", "for 2", "for 3"]);
    assert!(ids.lock().unwrap().iter().all(|id| *id == thread::current().id()));
}

#[test]
fn f2_for_then_the_alternative_runs_after_the_loop() {
    let log = Log::default();
    let t = tdo! { [log] let x = for_(vec![1, 2]); say(&log, format!("a {x}")) } | say(&log, "alternativa");
    assert_eq!(keep_all(t).len(), 3);
    assert_eq!(lines(&log), ["a 1", "a 2", "alternativa"]);
}

#[test]
fn f3_for_does_not_wait_for_the_asynchronous_tasks_inside() {
    let log = Log::default();
    let t = tdo! { [log]
        let x = for_(vec![1u64, 2, 3]);
        async_(move || sleep(ms((4 - x) * 25)));
        say(&log, format!("tras async {x}"))
    };
    assert_eq!(keep_all(t).len(), 3);
    // the loop ends at once and the tasks finish in the order of their waits
    assert_eq!(lines(&log), ["tras async 3", "tras async 2", "tras async 1"]);
}

// ---- choose

#[test]
fn c1_choose_runs_each_element_in_its_own_thread() {
    let log = Log::default();
    let t = tdo! { [log]
        let x = choose(vec![1u64, 2, 3]);
        lift_io(move || sleep(ms(x * 30)));
        say(&log, format!("choose {x}"))
    };
    assert_eq!(keep_all(t).len(), 3);
    assert_eq!(lines(&log), ["choose 1", "choose 2", "choose 3"]);
}

#[test]
fn c2_choose_with_threads_1_goes_one_at_a_time() {
    let log = Log::default();
    let t0 = Instant::now();
    let t = tdo! { [log]
        let x = threads(1, choose(vec![1, 2, 3]));
        lift_io(|| sleep(ms(30)));
        say(&log, format!("choose1 {x}"))
    };
    assert_eq!(keep_all(t).len(), 3);
    assert_eq!(lines(&log), ["choose1 1", "choose1 2", "choose1 3"]);
    assert!(t0.elapsed() >= ms(90), "de uno en uno: {:?}", t0.elapsed());
}

// ---- group

#[test]
fn g2_group_gives_each_group_with_the_last_result_first() {
    let r = keep_all(group(3, for_(vec![1, 2, 3, 4, 5, 6])));
    assert_eq!(r, [vec![3, 2, 1], vec![6, 5, 4]]);
}

#[test]
fn g1_group_of_events_in_several_threads() {
    let r = keep_all(group(2, choose(vec![1, 2, 3, 4, 5])));
    // 5 elements in groups of 2: two groups and the fifth is left incomplete
    assert_eq!(r.len(), 2);
    assert!(r.iter().all(|g| g.len() == 2));
}

// ---- sync

#[test]
fn s1_sync_waits_for_all_the_threads_and_gives_the_list() {
    let log = Log::default();
    let t = tdo! { [log]
        let rs = sync(tdo! { let x = choose(vec![1u64, 2, 3]); lift_io(move || sleep(ms(x * 10))); pure(x) });
        say(&log, format!("sync {}", rs.len()))
    };
    assert_eq!(keep_all(t).len(), 1);
    assert_eq!(lines(&log), ["sync 3"]);
}

#[test]
fn s2_sync_without_results_runs_the_alternative() {
    let log = Log::default();
    let t = tdo! { [log]
        let rs = sync(tdo! { async_(|| ()); empty::<()>() });
        say(&log, format!("sync {}", rs.len()))
    } | say(&log, "alternativa");
    assert_eq!(keep_all(t).len(), 1);
    assert_eq!(lines(&log), ["alternativa"]);
}

#[test]
fn s3_sync1_gives_the_first_result() {
    let log = Log::default();
    let t = tdo! { [log]
        let x = sync1(tdo! { let x = choose(vec![3u64, 1, 2]); lift_io(move || sleep(ms(x * 30))); pure(x) });
        say(&log, format!("sync1 {x}"))
    };
    keep_all(t);
    assert_eq!(lines(&log), ["sync1 1"]);
}

#[test]
fn s4_timeout_that_arrives_in_time() {
    let log = Log::default();
    let t = tdo! { [log]
        let x = timeout(ms(500), tdo! { async_(|| sleep(ms(10))); pure(1) });
        say(&log, format!("timeout ok {x}"))
    };
    keep_all(t);
    assert_eq!(lines(&log), ["timeout ok 1"]);
}

#[test]
fn s5_timeout_that_expires_runs_the_alternative() {
    let log = Log::default();
    let t = tdo! { [log]
        let x = timeout(ms(50), tdo! { async_(|| sleep(ms(300))); pure(1) });
        say(&log, format!("timeout ok {x}"))
    } | say(&log, "timeout vencido");
    keep_all(t);
    assert_eq!(lines(&log), ["timeout vencido"]);
}

// ---- group_by_time

#[test]
fn t1_group_by_time_groups_what_arrives_in_each_interval() {
    let n = Arc::new(AtomicU64::new(0));
    let tandas: Arc<Mutex<Vec<Vec<u64>>>> = Arc::default();
    let (t2, n2) = (tandas.clone(), n.clone());
    let events = wait_events(move || {
        sleep(ms(30));
        vec![n2.fetch_add(1, Ordering::SeqCst) + 1]
    });
    let t = tdo! { [t2]
        let xs = group_by_time(ms(100), events);
        lift_io(move || t2.lock().unwrap().push(xs.clone()));
        if t2.lock().unwrap().len() >= 3 { exit(()) } else { pure(()) }
    };
    assert_eq!(keep_with(|| Cursor::new(""), t), Ok(()));
    let tandas = tandas.lock().unwrap().clone();
    assert!(tandas.len() >= 3);
    // each batch brings between 2 and 5 events and, together and in order, they are the sequence 1, 2, 3... with no gaps or repetitions
    let mut all: Vec<u64> = tandas.iter().flatten().copied().collect();
    all.sort();
    assert_eq!(all, (1..=all.len() as u64).collect::<Vec<_>>());
    assert!(tandas.iter().all(|t| (2..=5).contains(&t.len())), "{tandas:?}");
}

// ---- one_thread. Haskell's `oneThread` was fixed so that they match (see the README of
// original-vs-rust): before it killed the producer of a `waitEvents` stream and cancelled nothing with `option`.
//
// Events every 20 ms and a 50 ms task per event: each task is cancelled by the next event, the producer goes on.
#[test]
fn o1_one_thread_keeps_the_producer_and_cancels_the_branches_of_previous_events() {
    let log = Log::default();
    let produced = Arc::new(AtomicU64::new(0));
    let p = produced.clone();
    let events = wait_events(move || {
        sleep(ms(20));
        p.fetch_add(1, Ordering::SeqCst) + 1
    });
    let work = tdo! { [log]
        let x = one_thread(events);
        async_(|| sleep(ms(50)));
        say(&log, format!("proceso {x}"))
    };
    // the timer can not hang from the node of `one_thread`: the first event would cancel everything that is not on its
    // path, the timer included. A free thread hangs from nobody.
    let stop = free_threads(tdo! { async_(|| sleep(ms(400))); exit(()) });
    let r: Result<(), String> = keep_with(|| Cursor::new(""), work | stop);
    assert_eq!(r, Ok(()));
    assert!(produced.load(Ordering::SeqCst) >= 10, "el productor sigue vivo");
    assert!(lines(&log).is_empty(), "{:?}", lines(&log));
}

// Tasks shorter than the interval between events: all of them finish (the current event is not cancelled).
#[test]
fn o4_one_thread_lets_the_current_event_finish() {
    let log = Log::default();
    let produced = Arc::new(AtomicU64::new(0));
    let p = produced.clone();
    let events = wait_events(move || {
        sleep(ms(80));
        p.fetch_add(1, Ordering::SeqCst) + 1
    });
    let work = tdo! { [log]
        let x = one_thread(events);
        async_(|| sleep(ms(20)));
        say(&log, format!("proceso {x}"))
    };
    let stop = free_threads(tdo! { async_(|| sleep(ms(500))); exit(()) });
    let _: Result<(), String> = keep_with(|| Cursor::new(""), work | stop);
    assert_eq!(&lines(&log)[..4], ["proceso 1", "proceso 2", "proceso 3", "proceso 4"]);
}

// The case of `option` (the one `Services` uses): three events in a row, a 300 ms task per event. Only the one of the last
// survives. Haskell's original let all three finish.
#[test]
fn o5_one_thread_over_option_keeps_only_the_last_event() {
    let log = Log::default();
    let t = tdo! { [log]
        let x = one_thread(option("go", "go"));
        async_(|| sleep(ms(300)));
        say(&log, format!("proceso {x}"));
        exit(())
    };
    let r: Result<(), String> = keep_with(|| Cursor::new("go go go\n"), t);
    assert_eq!(r, Ok(()));
    assert_eq!(lines(&log), ["proceso go"]);
}

// ---- burst (Exp15). Haskell's never emitted `Done`; it was fixed with `<->` and gives this same result.

#[test]
fn b1_burst_gives_done_after_the_silence() {
    let log = Log::default();
    let comp = async_(|| {
        sleep(ms(20));
        1u64
    });
    let t = tdo! { [log]
        let r = burst(ms(100), comp);
        say(&log, format!("burst {r:?}"));
        if r == StreamData::Done { exit(()) } else { pure(()) }
    };
    let res: Result<(), String> = keep_with(|| Cursor::new(""), t);
    assert_eq!(res, Ok(()));
    assert_eq!(lines(&log), ["burst More(1)", "burst Done"]);
}

#[test]
fn b2_burst_with_a_stream_gives_a_single_done_at_the_end() {
    let log = Log::default();
    let n = Arc::new(AtomicU64::new(0));
    let comp = wait_events(move || {
        sleep(ms(30));
        let k = n.fetch_add(1, Ordering::SeqCst) + 1;
        if k > 3 {
            sleep(Duration::from_secs(3600)); // the stream goes quiet
        }
        k
    });
    let t = tdo! { [log]
        let r = burst(ms(100), comp);
        say(&log, format!("burst {r:?}"));
        if r == StreamData::Done { exit(()) } else { pure(()) }
    };
    let res: Result<(), String> = keep_with(|| Cursor::new(""), t);
    assert_eq!(res, Ok(()));
    // the timers of the first two results are cancelled by the next one: only one fires
    assert_eq!(lines(&log), ["burst More(1)", "burst More(2)", "burst More(3)", "burst Done"]);
}
