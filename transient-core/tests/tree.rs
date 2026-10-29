//! The thread tree: `collect`, cancellation, `one_thread`, `free_threads`.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};
use transient_core::*;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn sorted<A: Ord>(mut v: Vec<A>) -> Vec<A> {
    v.sort();
    v
}

/// A stream of integers 0, 1, 2... one every `every` ms, that never ends
fn ticks(every: u64) -> Trans<i32> {
    let n = AtomicI32::new(0);
    wait_events(move || {
        sleep(ms(every));
        n.fetch_add(1, Ordering::SeqCst)
    })
}

#[test]
fn collect_gathers_the_results_of_all_the_threads() {
    let a = tdo! { abduce(); pure(1) };
    let b = tdo! { abduce(); lift_io(|| { sleep(ms(20)); 2 }) };
    let t = tdo! { let v = collect(0, a | b); pure(sorted(v)) };
    // the rest of the program goes on only once, with the list
    assert_eq!(keep_all(t), [vec![1, 2]]);
}

#[test]
fn collect_n_stops_the_producer() {
    let t = tdo! { let v = collect(3, ticks(5)); pure(v.len()) };
    // the list is delivered when 3 are gathered, and `keep_all` ends: the producer, which never ends, has been cancelled
    assert_eq!(keep_all(t), [3]);
}

#[test]
fn collect_for_stops_at_the_time() {
    let t0 = Instant::now();
    let t = tdo! { let v = collect_for(0, Some(ms(100)), ticks(10)); pure(v.len()) };
    let r = keep_all(t);
    assert!(t0.elapsed() >= ms(100));
    assert!(r[0] >= 3 && r[0] <= 12, "{r:?}");
}

#[test]
fn end_collect_ends_it_with_what_has_been_gathered() {
    let events = tdo! { let i = ticks(10); if i == 3 { tdo! { end_collect(); pure(i) } } else { pure(i) } };
    let t = tdo! { let v = collect(0, events); pure(sorted(v)) };
    assert_eq!(keep_all(t), [vec![0, 1, 2]]);
}

#[test]
fn kill_children_cancels_the_branches() {
    // from result 3 on, everything that was born from the main thread is cancelled, the producer itself included
    let t = tdo! {
        let h = thread_handle();
        let i = ticks(20);
        lift_io(move || { if i == 3 { h.kill_children() }; i })
    };
    assert_eq!(sorted(keep_all(t)), [0, 1, 2]);
}

/// Each event launches a slow task that records its number when it ends
fn slow_tasks(log: Arc<Mutex<Vec<i32>>>, one: bool) -> Trans<()> {
    let n = AtomicI32::new(0);
    let events = parallel(move || {
        sleep(ms(20));
        match n.fetch_add(1, Ordering::SeqCst) + 1 {
            i @ 1..=3 => StreamData::More(i),
            _ => StreamData::Done,
        }
    })
    .map(|s| if let StreamData::More(i) = s { i } else { 0 });
    let events = if one { one_thread(events) } else { events };
    tdo! { [log]
        let i = events;
        async_(|| sleep(ms(70)));
        lift_io(move || log.lock().unwrap().push(i))
    }
}

#[test]
fn without_one_thread_every_event_is_processed() {
    let log = Arc::new(Mutex::new(Vec::new()));
    keep_all(slow_tasks(log.clone(), false));
    assert_eq!(sorted(log.lock().unwrap().clone()), [1, 2, 3]);
}

#[test]
fn one_thread_cancels_the_processing_of_the_previous_results() {
    let log = Arc::new(Mutex::new(Vec::new()));
    keep_all(slow_tasks(log.clone(), true));
    // events arrive every 20 ms and each task takes 70: only the last one gets to finish
    assert_eq!(*log.lock().unwrap(), [3]);
}

#[test]
fn free_threads_are_not_waited_for() {
    let t0 = Instant::now();
    let r = keep_all(free_threads(async_(|| { sleep(ms(300)); 1 })));
    assert!(t0.elapsed() < ms(150));
    assert!(r.is_empty());
}
