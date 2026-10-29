//! The data state, `react`, backtracking (`on_undo`, `retry`, `on_back`) and `on_finish`.

use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, sleep};
use std::time::Duration;
use transient_core::*;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

type Log = Arc<Mutex<Vec<&'static str>>>;

fn push(log: &Log, s: &'static str) -> Trans<()> {
    let log = log.clone();
    lift_io(move || log.lock().unwrap().push(s))
}

// ---- data state

#[test]
fn data_is_copied_to_the_branches_and_rstate_is_shared() {
    let t = tdo! {
        set_data(1i32);
        new_rstate(0i32);
        // the thread of the fork changes both: the data is its own, the cell is everybody's
        fork(tdo! { set_data(7i64); set_rstate(5i32) });
        lift_io(|| sleep(ms(40)));
        let celda = get_rstate::<i32>();
        let dato = get_data::<i64>();
        let previo = get_data::<i32>();
        lift_io(move || (celda, dato.is_none(), previo.as_deref().copied()))
    };
    assert_eq!(keep_all(t), [(5, true, Some(1))]);
}

#[test]
fn rstate_is_empty_when_missing_and_modify_changes_it() {
    assert_eq!(keep_all(get_rstate::<i32>()), Vec::<i32>::new());
    let t = tdo! { new_rstate(1i32); modify_rstate(|x: i32| x + 41); get_rstate::<i32>() };
    assert_eq!(keep_all(t), [42]);
}

// ---- react: the rest runs in the thread that calls the callback

#[test]
fn react_continues_in_the_thread_that_calls_the_callback() {
    let slot: Arc<Mutex<Option<Arc<dyn Fn(i32) + Send + Sync>>>> = Arc::default();
    let seen: Arc<Mutex<Vec<(i32, thread::ThreadId)>>> = Arc::default();

    let (s, sn) = (slot.clone(), seen.clone());
    let t = tdo! { [sn]
        let i = react(move |cb| *s.lock().unwrap() = Some(cb), || ());
        lift_io(move || sn.lock().unwrap().push((i, thread::current().id())));
        if i == 3 { exit(i) } else { pure(()) }
    };

    let external = thread::spawn(move || {
        let cb = loop {
            if let Some(cb) = slot.lock().unwrap().clone() {
                break cb;
            }
            sleep(ms(5));
        };
        (1..=3).for_each(|i| cb(i));
        thread::current().id()
    });
    assert_eq!(keep_with(|| Cursor::new(""), t), Ok(3));
    let external = external.join().unwrap();
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.iter().map(|s| s.0).collect::<Vec<_>>(), [1, 2, 3]);
    assert!(seen.iter().all(|s| s.1 == external));
}

#[test]
fn react_holds_the_tree_until_the_listener_is_removed() {
    let slot: Arc<Mutex<Option<Arc<dyn Fn(i32) + Send + Sync>>>> = Arc::default();
    let s = slot.clone();
    let t = tdo! {
        let i = react_id(true, "ev-del", move |cb| *s.lock().unwrap() = Some(cb), || ());
        del_listener("ev-del");
        pure(i)
    };
    let external = thread::spawn(move || {
        sleep(ms(50));
        let cb = slot.lock().unwrap().clone().unwrap();
        cb(7)
    });
    // without the listener, `keep_all` would have ended at once with []
    assert_eq!(keep_all(t), [7]);
    external.join().unwrap();
}

#[test]
fn react_nokeep_does_not_hold_the_tree() {
    let t = react_nokeep::<i32, ()>(|_cb| (), || ());
    assert_eq!(keep_all(t), Vec::<i32>::new());
}

// ---- undo and retry

#[test]
fn undo_runs_the_handlers_in_reverse_order() {
    let log: Log = Arc::default();
    let t = tdo! { [log]
        on_undo(push(&log, "a"), push(&log, "undo a"));
        on_undo(push(&log, "b"), push(&log, "undo b"));
        undo::<()>()
    };
    keep_all(t);
    assert_eq!(*log.lock().unwrap(), ["a", "b", "undo b", "undo a"]);
}

#[test]
fn retry_resumes_forward_from_the_point_of_registration() {
    let log: Log = Arc::default();
    let n = Arc::new(AtomicUsize::new(0));
    let t = tdo! { [log, n]
        on_undo(push(&log, "start"), tdo! { push(&log, "undo"); retry() });
        let k = lift_io(move || n.fetch_add(1, Ordering::SeqCst) + 1);
        if k == 1 { undo::<()>() } else { push(&log, "done") }
    };
    keep_all(t);
    // the first time it is undone, the handler retries and resumes after `on_undo`
    assert_eq!(*log.lock().unwrap(), ["start", "undo", "done"]);
}

#[test]
fn handlers_registered_before_a_thread_are_inherited_by_it() {
    let log: Log = Arc::default();
    let t = tdo! { [log]
        on_undo(pure(()), push(&log, "undo"));
        abduce();
        undo::<()>()
    };
    keep_all(t);
    assert_eq!(*log.lock().unwrap(), ["undo"]);
}

#[test]
fn a_handler_that_stops_ends_the_backtracking() {
    let log: Log = Arc::default();
    let t = tdo! { [log]
        on_undo(push(&log, "a"), push(&log, "undo a"));
        on_undo(push(&log, "b"), tdo! { push(&log, "undo b"); empty() });
        undo::<()>()
    };
    keep_all(t);
    assert_eq!(*log.lock().unwrap(), ["a", "b", "undo b"]);
}

// the original (checked by running it): `localBack` does not empty the stack on entry, it only restores the previous state on
// exit. Inside, the handlers from outside are also seen, and they fire after those inside.
#[test]
fn local_back_sees_the_outer_handlers_after_the_inner_ones() {
    let log: Log = Arc::default();
    let t = tdo! { [log]
        on_undo(pure(()), push(&log, "outer"));
        local_back::<(), ()>(tdo! { on_undo(pure(()), push(&log, "inner")); undo::<()>() })
    };
    keep_all(t);
    assert_eq!(*log.lock().unwrap(), ["inner", "outer"]);
}

#[test]
fn local_back_discards_the_handlers_registered_inside_when_it_ends() {
    let log: Log = Arc::default();
    let t = tdo! { [log]
        on_undo(pure(()), push(&log, "outer"));
        local_back::<(), ()>(on_undo(pure(()), push(&log, "inner")));
        undo::<()>()
    };
    keep_all(t);
    assert_eq!(*log.lock().unwrap(), ["outer"]);
}

#[test]
fn local_back_with_a_thread_inside_still_fires_inner_then_outer() {
    let log: Log = Arc::default();
    let t = tdo! { [log]
        on_undo(pure(()), push(&log, "outer"));
        local_back::<(), ()>(tdo! { on_undo(pure(()), push(&log, "inner")); abduce(); undo::<()>() })
    };
    keep_all(t);
    assert_eq!(*log.lock().unwrap(), ["inner", "outer"]);
}

#[test]
fn on_back_with_a_typed_reason() {
    #[derive(Clone, Debug)]
    struct Boom(u32);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let t = tdo! {
        on_back1(move |Boom(n)| { let s = s.clone(); lift_io(move || s.lock().unwrap().push(n)) });
        back::<(), _>(Boom(42))
    };
    keep_all(t);
    assert_eq!(*seen.lock().unwrap(), [42]);
}

// ---- on_finish

#[test]
fn on_finish_runs_once_when_no_thread_is_left_under_it() {
    let log: Log = Arc::default();
    let n = Arc::new(AtomicUsize::new(0));
    let c = n.clone();
    let l = log.clone();
    let t = tdo! { [log, n]
        on_finish(move |_| { c.fetch_add(1, Ordering::SeqCst); push(&l, "finish") });
        let i = parallel({ let k = AtomicUsize::new(0); move || {
            sleep(ms(10));
            match k.fetch_add(1, Ordering::SeqCst) { 0..=2 => StreamData::More(()), _ => StreamData::Done }
        }});
        lift_io(|| sleep(ms(30)));
        push(&log, "work")
    };
    keep_all(t);
    let log = log.lock().unwrap();
    assert_eq!(log.iter().filter(|s| **s == "work").count(), 3);
    assert_eq!(*log.last().unwrap(), "finish");
    assert_eq!(n.load(Ordering::SeqCst), 1);
}

#[test]
fn nested_finalizers_run_from_the_last_registered_to_the_first() {
    let log: Log = Arc::default();
    let t = tdo! { [log]
        on_finish({ let l = log.clone(); move |_| push(&l, "first") });
        on_finish({ let l = log.clone(); move |_| push(&l, "second") });
        async_(|| sleep(ms(10)))
    };
    keep_all(t);
    assert_eq!(*log.lock().unwrap(), ["second", "first"]);
}

// ---- more experiments against the original (B7-B11)

#[test]
fn every_thread_that_calls_back_fires_the_handler() {
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let l = log.clone();
    let branch = |i: u32| tdo! { abduce(); back::<(), String>(format!("hilo{i}")) };
    let t = tdo! {
        on_back1(move |s: String| { let l = l.clone(); lift_io(move || l.lock().unwrap().push(format!("handler {s}"))) });
        branch(1) | branch(2) | branch(3)
    };
    keep_all(t);
    let mut got = log.lock().unwrap().clone();
    got.sort();
    assert_eq!(got, ["handler hilo1", "handler hilo2", "handler hilo3"]);
}

#[test]
fn the_same_thread_can_undo_and_retry_several_times() {
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let n = Arc::new(AtomicUsize::new(0));
    let (l1, l2) = (log.clone(), log.clone());
    let t = tdo! { [n, l1, l2]
        on_undo(pure(()), tdo! { lift_io({ let l = l1.clone(); move || l.lock().unwrap().push("undo".into()) }); retry() });
        let k = lift_io(move || n.fetch_add(1, Ordering::SeqCst) + 1);
        lift_io({ let l = l2.clone(); move || l.lock().unwrap().push(format!("k={k}")) });
        if k < 3 { undo::<()>() } else { pure(()) };
        lift_io({ let l = l2.clone(); move || l.lock().unwrap().push("fin".into()) })
    };
    assert_eq!(keep_all(t).len(), 1);
    assert_eq!(*log.lock().unwrap(), ["k=1", "undo", "k=2", "undo", "k=3", "fin"]);
}

#[test]
fn undo_cut_deletes_the_registered_handlers() {
    let log: Log = Arc::default();
    let t = tdo! { [log]
        on_undo(pure(()), push(&log, "viejo"));
        undo_cut();
        on_undo(pure(()), push(&log, "nuevo"));
        undo::<()>()
    };
    keep_all(t);
    assert_eq!(*log.lock().unwrap(), ["nuevo"]);
}

#[test]
fn undo_without_handlers_just_stops() {
    let log: Log = Arc::default();
    let t = tdo! { [log] push(&log, "antes"); undo::<()>(); push(&log, "no llega") };
    assert_eq!(keep_all(t).len(), 0);
    assert_eq!(*log.lock().unwrap(), ["antes"]);
}

#[test]
fn a_handler_gives_the_value_with_which_the_flow_resumes() {
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let l = log.clone();
    let t = tdo! {
        let r = on_back(pure("normal"), |()| tdo! { retry(); pure("del manejador") });
        lift_io({ let l = l.clone(); move || l.lock().unwrap().push(r.to_string()) });
        if r == "normal" { undo::<()>() } else { pure(()) }
    };
    keep_all(t);
    assert_eq!(*log.lock().unwrap(), ["normal", "del manejador"]);
}
