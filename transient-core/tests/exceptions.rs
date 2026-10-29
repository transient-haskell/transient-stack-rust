//! Exceptions. Each test reproduces an experiment that was run against the original in Haskell; the expected
//! results are the ones the original gave. What `keep_all` prints at the end of an unstopped exception (the first
//! handler) goes to standard output and is not checked.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;
use transient_core::*;

#[derive(Clone, Debug)]
struct Boom(String);

#[derive(Clone, Debug)]
struct Other;

type Log = Arc<Mutex<Vec<String>>>;

fn say(log: &Log, s: impl Into<String>) -> Trans<()> {
    let (log, s) = (log.clone(), s.into());
    lift_io(move || log.lock().unwrap().push(s.clone()))
}

fn lines(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

fn boom(s: &str) -> Boom {
    Boom(s.to_string())
}

/// Throws `e` as `throw`: a `panic` with the exception, without keeping the state
fn thr<E: Clone + Send + Sync + std::fmt::Debug + 'static>(e: E) -> Trans<()> {
    lift_io(move || throw(e.clone()))
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[test]
fn x1_the_handler_catches_what_happens_below_it() {
    let log = Log::default();
    let l = log.clone();
    let t = tdo! { [log]
        on_exception(move |e: Boom| say(&l, format!("caught {}", e.0)));
        say(&log, "antes");
        thr(boom("boom"));
        say(&log, "no llega")
    };
    assert_eq!(keep_all(t).len(), 0);
    assert_eq!(lines(&log), ["antes", "caught boom"]);
}

#[test]
fn x2_one_registration_catches_every_thread_every_time() {
    let log = Log::default();
    let l = log.clone();
    let branch = |log: Log, i: u32| {
        tdo! { [log]
            abduce();
            if i != 1 { thr(Boom(i.to_string())) } else { pure(()) };
            say(&log, format!("ok {i}"))
        }
    };
    let t = tdo! { [log]
        on_exception(move |e: Boom| say(&l, format!("caught {}", e.0)));
        branch(log.clone(), 1) | branch(log.clone(), 2) | branch(log.clone(), 3)
    };
    assert_eq!(keep_all(t).len(), 1);
    assert_eq!(sorted(lines(&log)), ["caught 2", "caught 3", "ok 1"]);
}

#[test]
fn x2b_a_stream_of_events_fires_the_same_registration_again_and_again() {
    let log = Log::default();
    let l = log.clone();
    let n = std::sync::atomic::AtomicU32::new(0);
    let events = parallel(move || match n.fetch_add(1, Ordering::SeqCst) + 1 {
        i @ 1..=5 => StreamData::More(i),
        _ => StreamData::Done,
    });
    let t = tdo! { [events]
        on_exception(move |e: Boom| say(&l, format!("caught {}", e.0)));
        let ev = events;
        if let StreamData::More(i) = ev { thr(Boom(i.to_string())) } else { pure(()) }
    };
    keep_all(t);
    assert_eq!(sorted(lines(&log)), ["caught 1", "caught 2", "caught 3", "caught 4", "caught 5"]);
}

#[test]
fn x3_handlers_are_filtered_by_type() {
    let log = Log::default();
    let (l1, l2) = (log.clone(), log.clone());
    let t = tdo! { [l2]
        on_exception(move |_: Boom| say(&l1, "Boom"));
        on_exception(move |_: Other| say(&l2, "Other"));
        thr(Other)
    };
    keep_all(t);
    assert_eq!(lines(&log), ["Other"]);
}

#[test]
fn x4_handlers_run_last_registered_first() {
    let log = Log::default();
    let (l1, l2) = (log.clone(), log.clone());
    let t = tdo! { [l2]
        on_exception(move |_: Boom| say(&l1, "primero registrado"));
        on_exception(move |_: Boom| say(&l2, "segundo registrado"));
        thr(boom("x"))
    };
    keep_all(t);
    assert_eq!(lines(&log), ["segundo registrado", "primero registrado"]);
}

#[test]
fn x5_continue_resumes_forward_from_the_point_of_registration() {
    let log = Log::default();
    let (l, n) = (log.clone(), Arc::new(AtomicUsize::new(0)));
    let t = tdo! { [log, n]
        on_exception(move |_: Boom| tdo! { say(&l, "handler"); continue_() });
        let k = lift_io(move || n.fetch_add(1, Ordering::SeqCst) + 1);
        say(&log, format!("k={k}"));
        if k == 1 { thr(boom("x")) } else { pure(()) };
        say(&log, "fin")
    };
    assert_eq!(keep_all(t).len(), 1);
    assert_eq!(lines(&log), ["k=1", "handler", "k=2", "fin"]);
}

// Haskell's `catcht` does NOT catch a `throw` inside its first argument (only `throwt`): it is a bug of that
// version and here it is fixed. The tests x6 and x6c check it.
#[test]
fn x6_catcht_catches_a_throw_inside_its_first_argument() {
    let log = Log::default();
    let t = tdo! { [log]
        let r = catcht(tdo! { thr(boom("in mx")); pure("no".to_string()) }, |e: Boom| pure(format!("recuperado {}", e.0)));
        say(&log, r.clone())
    };
    assert_eq!(keep_all(t).len(), 1);
    assert_eq!(lines(&log), ["recuperado in mx"]);
}

#[test]
fn x6b_catcht_without_an_exception_gives_the_value() {
    let log = Log::default();
    let t = tdo! { [log]
        let r = catcht(pure("normal".to_string()), |_: Boom| pure("no".to_string()));
        say(&log, r.clone())
    };
    assert_eq!(keep_all(t).len(), 1);
    assert_eq!(lines(&log), ["normal"]);
}

#[test]
fn x6c_catcht_catches_it_also_when_it_is_not_the_first_statement() {
    let log = Log::default();
    let t = tdo! { [log]
        let r = catcht(tdo! { say(&log, "dentro"); thr(boom("x")); pure("no".to_string()) }, |_: Boom| pure("recuperado".to_string()));
        say(&log, r.clone())
    };
    assert_eq!(keep_all(t).len(), 1);
    assert_eq!(lines(&log), ["dentro", "recuperado"]);
}

#[test]
fn x6f_catcht_catches_a_panic_too() {
    let log = Log::default();
    let t = tdo! { [log]
        let r = catcht(tdo! { lift_io(|| -> () { panic!("pura") }); pure("no".to_string()) }, |e: Panic| pure(format!("recuperado {}", e.0)));
        say(&log, r.clone())
    };
    keep_all(t);
    assert_eq!(lines(&log), ["recuperado pura"]);
}

#[test]
fn x6g_while_exception_repeats_until_it_works() {
    let log = Log::default();
    let n = Arc::new(AtomicUsize::new(0));
    let l = log.clone();
    let attempt = tdo! { [n]
        let k = lift_io(move || n.fetch_add(1, Ordering::SeqCst) + 1);
        if k < 3 { thr(Boom(k.to_string())) } else { pure(()) };
        pure(k)
    };
    let t = tdo! { [log]
        let k = while_exception(attempt, move |e: Boom| say(&l, format!("fallo {}", e.0)));
        say(&log, format!("ok tras {k}"))
    };
    assert_eq!(keep_all(t).len(), 1);
    assert_eq!(lines(&log), ["fallo 1", "fallo 2", "ok tras 3"]);
}

#[test]
fn x6d_catcht_does_not_cover_what_comes_after_it() {
    let log = Log::default();
    let t = tdo! { [log]
        let r = catcht(pure("mx".to_string()), |_: Boom| pure("recuperado".to_string()));
        say(&log, r.clone());
        thr(boom("despues"))
    };
    keep_all(t);
    assert_eq!(lines(&log), ["mx"]);
}

#[test]
fn x6e_catcht_catches_a_throwt_inside_its_first_argument() {
    let log = Log::default();
    let t = tdo! { [log]
        let r = catcht(
            tdo! { say(&log, "dentro"); throwt::<String, _>(boom("t")) },
            |e: Boom| pure(format!("recuperado {}", e.0)));
        say(&log, r.clone())
    };
    assert_eq!(keep_all(t).len(), 1);
    assert_eq!(lines(&log), ["dentro", "recuperado t"]);
}

#[test]
fn x7_throwt_keeps_the_state_and_throw_gives_the_state_of_the_registration() {
    let log = Log::default();
    let handler = |log: Log| {
        move |_: Boom| {
            let log = log.clone();
            tdo! { [log]
                let v = get_data::<String>();
                say(&log, format!("estado: {:?}", v.as_deref()))
            }
        }
    };
    let l = log.clone();
    let con_throwt = tdo! { [l]
        set_data("inicial".to_string());
        on_exception(handler(l.clone()));
        set_data("cambiado".to_string());
        throwt::<(), _>(boom("t"))
    };
    keep_all(con_throwt);
    let con_throw = tdo! { [l]
        set_data("inicial".to_string());
        on_exception(handler(l.clone()));
        set_data("cambiado".to_string());
        thr(boom("io"))
    };
    keep_all(con_throw);
    assert_eq!(lines(&log), ["estado: Some(\"cambiado\")", "estado: Some(\"inicial\")"]);
}

#[test]
fn x8_an_exception_in_a_thread_launched_after_the_handler_is_caught() {
    let log = Log::default();
    let l = log.clone();
    let t = tdo! {
        on_exception(move |e: Boom| say(&l, format!("caught {}", e.0)));
        abduce();
        lift_io(|| sleep(Duration::from_millis(10)));
        thr(boom("en hilo"))
    };
    // in the original `keep'` did not end after this; here the tree becomes quiet
    assert_eq!(keep_all(t).len(), 0);
    assert_eq!(lines(&log), ["caught en hilo"]);
}

#[test]
fn x9_without_a_handler_the_exception_is_left_to_keep_and_nothing_goes_on() {
    let log = Log::default();
    let t = tdo! { [log] thr(boom("sin manejador")); say(&log, "no llega") };
    assert_eq!(keep_all(t).len(), 0);
    assert_eq!(lines(&log), Vec::<String>::new());
}

#[test]
fn x10_a_handler_registered_after_the_throw_is_not_triggered() {
    let log = Log::default();
    let (l1, l2) = (log.clone(), log.clone());
    let t = tdo! { [log, l2]
        on_exception(move |_: Boom| say(&l1, "antes"));
        say(&log, "a");
        thr(boom("x"));
        on_exception(move |_: Boom| say(&l2, "despues (no deberia)"));
        say(&log, "b")
    };
    keep_all(t);
    assert_eq!(lines(&log), ["a", "antes"]);
}

#[test]
fn x11_local_exceptions_handlers_do_not_apply_outside() {
    let log = Log::default();
    let (l1, l2) = (log.clone(), log.clone());
    let t = tdo! { [l2]
        on_exception(move |_: Boom| say(&l1, "fuera"));
        local_exceptions(on_exception(move |_: Boom| say(&l2, "dentro")));
        thr(boom("x"))
    };
    keep_all(t);
    assert_eq!(lines(&log), ["fuera"]);
}

#[test]
fn x12_cut_exceptions_deletes_the_previous_handlers() {
    let log = Log::default();
    let (l1, l2) = (log.clone(), log.clone());
    let t = tdo! { [l2]
        on_exception(move |_: Boom| say(&l1, "viejo"));
        cut_exceptions();
        on_exception(move |_: Boom| say(&l2, "nuevo"));
        thr(boom("x"))
    };
    keep_all(t);
    assert_eq!(lines(&log), ["nuevo"]);
}

#[test]
fn x13_a_handler_that_stops_cuts_the_chain() {
    let log = Log::default();
    let (l1, l2) = (log.clone(), log.clone());
    let t = tdo! { [l2]
        on_exception(move |_: Boom| say(&l1, "primero"));
        on_exception(move |_: Boom| tdo! { say(&l2, "segundo y paro"); empty() });
        thr(boom("x"))
    };
    keep_all(t);
    assert_eq!(lines(&log), ["segundo y paro"]);
}

#[test]
fn x14_some_exception_catches_everything() {
    let log = Log::default();
    let l = log.clone();
    let t = tdo! {
        on_exception(move |e: SomeException| say(&l, format!("any: {e}")));
        thr(Other)
    };
    keep_all(t);
    assert_eq!(lines(&log), ["any: Other"]);
}

#[test]
fn x15_a_panic_is_caught_as_panic() {
    let log = Log::default();
    let l = log.clone();
    let t = tdo! {
        on_exception(move |e: Panic| say(&l, format!("caught {}", e.0)));
        lift_io(|| -> () { panic!("pura") })
    };
    keep_all(t);
    assert_eq!(lines(&log), ["caught pura"]);
}

// `mx` moves to another thread (`abduce`) before throwing: the `catch` of `catcht` is not on the stack of that thread.
#[test]
fn x6h_catcht_catches_a_throw_after_abduce_inside_mx() {
    let log = Log::default();
    let t = tdo! { [log]
        let r = catcht(tdo! { abduce(); thr(boom("hilo")); pure("no".to_string()) }, |e: Boom| pure(format!("recuperado {}", e.0)));
        say(&log, r.clone())
    };
    keep_all(t);
    assert_eq!(lines(&log), ["recuperado hilo"]);
}

#[test]
fn x6i_catcht_catches_a_panic_after_abduce_inside_mx() {
    let log = Log::default();
    let t = tdo! { [log]
        let r = catcht(tdo! { abduce(); lift_io(|| -> () { panic!("pura") }); pure("no".to_string()) }, |e: Panic| pure(format!("recuperado {}", e.0)));
        say(&log, r.clone())
    };
    keep_all(t);
    assert_eq!(lines(&log), ["recuperado pura"]);
}

#[test]
fn x6j_catcht_catches_a_throw_made_directly_in_the_body() {
    let log = Log::default();
    let t = tdo! { [log]
        let r = catcht(tdo! { let x = lift_io(|| 1u32); thr(boom("directo")); pure(x.to_string()) }, |e: Boom| pure(format!("recuperado {}", e.0)));
        say(&log, r.clone())
    };
    keep_all(t);
    assert_eq!(lines(&log), ["recuperado directo"]);
}

// ---- a live listener keeps `keep_all` waiting, also after an exception

use std::sync::mpsc;

/// The number of results `keep_all` ends with, or `None` if it does not return (something is still alive)
fn ends(t: Trans<()>) -> Option<usize> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || tx.send(keep_all(t).len()).unwrap());
    rx.recv_timeout(Duration::from_millis(700)).ok()
}

#[test]
fn a_react_whose_handler_is_not_finalized_keeps_keep_all_waiting() {
    let slot: Arc<Mutex<Option<Arc<dyn Fn(i32) + Send + Sync>>>> = Arc::default();
    let s = slot.clone();
    let t = tdo! { let _i = react(move |cb| *s.lock().unwrap() = Some(cb), || ()); pure(()) };
    // nobody calls the callback or removes the listener: `keep_all` does not return, like `keep'` in Haskell
    assert_eq!(ends(t), None);
}

#[test]
fn an_exception_does_not_release_a_live_listener() {
    let slot: Arc<Mutex<Option<Arc<dyn Fn(i32) + Send + Sync>>>> = Arc::default();
    let s = slot.clone();
    let t = tdo! {
        let _i = react(move |cb| *s.lock().unwrap() = Some(cb), || ());
        pure(())
    };
    let with_exception = tdo! { [t] on_exception(|_: Boom| pure(())); tdo! { abduce(); thr(boom("x")) } | t };
    // the thread that throws dies, but the listener of `react` is still waiting: `keep_all` does not return
    assert_eq!(ends(with_exception), None);
}

#[test]
fn once_the_listener_is_removed_keep_all_returns() {
    let slot: Arc<Mutex<Option<Arc<dyn Fn(i32) + Send + Sync>>>> = Arc::default();
    let s = slot.clone();
    let t = tdo! {
        let i = react_id(true, "react-que-termina", move |cb| *s.lock().unwrap() = Some(cb), || ());
        del_listener("react-que-termina");
        pure(i).map(|_| ())
    };
    let external = std::thread::spawn(move || {
        sleep(Duration::from_millis(100));
        let cb = slot.lock().unwrap().clone().unwrap();
        cb(1)
    });
    assert_eq!(ends(t), Some(1));
    external.join().unwrap();
}
