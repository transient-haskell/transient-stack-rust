//! EVars and mailboxes. Each test reproduces an experiment that was run against the original in Haskell
//! (the expected results are the ones the original gave).

use std::io::Cursor;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, sleep};
use std::time::Duration;
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

/// The number of results `keep_all` ends with, or `None` if it does not end (because listeners remain)
fn ends(t: Trans<()>) -> Option<usize> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || tx.send(keep_all(t).len()).unwrap());
    rx.recv_timeout(Duration::from_millis(700)).ok()
}

// ---- EVars

#[test]
fn e1_the_doc_example_ends_with_last_write() {
    let log = Log::default();
    let t = tdo! { [log]
        let ev = new_evar::<String>();
        let r = read_evar(&ev) | tdo! {
            lift_io({ let ev = ev.clone(); move || { ev.write("hello".into()); ev.last_write("world".into()) } });
            empty::<String>()
        };
        say(&log, r.clone())
    };
    assert_eq!(ends(t), Some(2));
    assert_eq!(lines(&log), ["hello", "world"]);
}

#[test]
fn e1b_without_last_write_the_reader_stays_alive() {
    let log = Log::default();
    let t = tdo! { [log]
        let ev = new_evar::<String>();
        let r = read_evar(&ev) | tdo! {
            lift_io({ let ev = ev.clone(); move || { ev.write("hello".into()); ev.write("world".into()) } });
            empty::<String>()
        };
        say(&log, r.clone())
    };
    assert_eq!(ends(t), None);
    assert_eq!(lines(&log), ["hello", "world"]);
}

#[test]
fn e2_readers_are_called_last_in_first_and_write_is_synchronous() {
    let (log, ev) = (Log::default(), EVar::<String>::new());
    let a = tdo! { [log, ev] let r = read_evar(&ev); say(&log, format!("A {r}")) };
    let b = tdo! { [log, ev] let r = read_evar(&ev); say(&log, format!("B {r}")) };
    let c = tdo! { [log, ev]
        lift_io({ let ev = ev.clone(); move || ev.write("x".to_string()) });
        say(&log, "escrito");
        lift_io({ let ev = ev.clone(); move || ev.clean() });
        empty::<()>()
    };
    assert_eq!(ends(a | b | c), Some(2));
    // both readers finish before `write` returns
    assert_eq!(lines(&log), ["B x", "A x", "escrito"]);
}

#[test]
fn e3_read_evar1_reads_a_single_value() {
    let (log, ev) = (Log::default(), EVar::<i32>::new());
    let a = tdo! { [log, ev] let r = read_evar1(&ev); say(&log, format!("uno {r}")) };
    let b = tdo! { [log, ev]
        lift_io({ let ev = ev.clone(); move || { ev.write(1); ev.write(2) } });
        say(&log, "escritos");
        empty::<()>()
    };
    assert_eq!(ends(a | b), Some(1));
    assert_eq!(lines(&log), ["uno 1", "escritos"]);
}

#[test]
fn e4_del_read_evar_inside_the_reader() {
    let (log, ev) = (Log::default(), EVar::<i32>::new());
    let a = tdo! { [log, ev] let r = read_evar(&ev); say(&log, format!("leido {r}")); del_read_evar(&ev) };
    let b = tdo! { [log, ev]
        lift_io({ let ev = ev.clone(); move || { ev.write(1); ev.write(2) } });
        say(&log, "escritos");
        empty::<()>()
    };
    assert_eq!(ends(a | b), Some(1));
    assert_eq!(lines(&log), ["leido 1", "escritos"]);
}

#[test]
fn e5_a_reader_that_goes_on_in_its_own_thread_does_not_hold_the_writer() {
    let (log, ev) = (Log::default(), EVar::<i32>::new());
    let a = tdo! { [log, ev]
        let r = read_evar(&ev);
        abduce();
        lift_io(|| sleep(ms(100)));
        say(&log, format!("lento {r}"));
        del_read_evar(&ev)
    };
    let b = tdo! { [log, ev]
        lift_io({ let ev = ev.clone(); move || ev.write(1) });
        say(&log, "escrito");
        empty::<()>()
    };
    assert_eq!(ends(a | b), Some(1));
    assert_eq!(lines(&log), ["escrito", "lento 1"]);
}

// ---- mailboxes. Each test uses types or keys of its own: the registry is application-wide and the tests run at the same time

#[derive(Clone)]
struct N1(i32);

#[test]
fn m1_put_and_get_by_type_and_delete_mailbox() {
    let log = Log::default();
    let reader = tdo! { [log]
        let x = get_mailbox::<N1>();
        say(&log, format!("got {}", x.0));
        if x.0 == 3 { lift_io(|| delete_mailbox::<N1>()) } else { pure(()) }
    };
    let writer = tdo! { lift_io(|| (1..=3).for_each(|i| put_mailbox(N1(i)))); empty::<()>() };
    assert_eq!(ends(reader | writer), Some(3));
    assert_eq!(lines(&log), ["got 1", "got 2", "got 3"]);
}

#[derive(Clone)]
struct M2Int(i32);
#[derive(Clone)]
struct M2Str(&'static str);

#[test]
fn m2_mailboxes_are_distinguished_by_type_and_by_key() {
    let log = Log::default();
    let r1 = tdo! { [log] let x = get_mailbox::<M2Int>(); say(&log, format!("int {}", x.0)); lift_io(|| delete_mailbox::<M2Int>()) };
    let r2 = tdo! { [log] let x = get_mailbox::<M2Str>(); say(&log, format!("string {}", x.0)); lift_io(|| delete_mailbox::<M2Str>()) };
    let r3 = tdo! { [log] let x = get_mailbox_key::<&str, M2Int>("a"); say(&log, format!("clave a {}", x.0)); lift_io(|| delete_mailbox_key::<&str, M2Int>("a")) };
    let r4 = tdo! { [log] let x = get_mailbox_key::<&str, M2Int>("b"); say(&log, format!("clave b {}", x.0)); lift_io(|| delete_mailbox_key::<&str, M2Int>("b")) };
    let w = tdo! {
        lift_io(|| {
            put_mailbox(M2Int(1));
            put_mailbox(M2Str("s"));
            put_mailbox_key("a", M2Int(10));
            put_mailbox_key("b", M2Int(20));
        });
        empty::<()>()
    };
    assert_eq!(ends(r1 | r2 | r3 | r4 | w), Some(4));
    assert_eq!(lines(&log), ["int 1", "string s", "clave a 10", "clave b 20"]);
}

#[derive(Clone)]
struct M3(i32);

#[test]
fn m3_last_put_mailbox_ends_the_readers() {
    let log = Log::default();
    let reader = tdo! { [log] let x = get_mailbox::<M3>(); say(&log, format!("got {}", x.0)) };
    let writer = tdo! { lift_io(|| { put_mailbox(M3(1)); last_put_mailbox(M3(2)) }); empty::<()>() };
    assert_eq!(ends(reader | writer), Some(2));
    assert_eq!(lines(&log), ["got 1", "got 2"]);
}

#[derive(Clone)]
struct M4(i32);

#[test]
fn m4_a_put_without_readers_is_lost() {
    let log = Log::default();
    let lost = tdo! { [log] lift_io(|| put_mailbox(M4(1))); say(&log, "put 1 sin lector"); empty::<()>() };
    let reader = tdo! { [log]
        lift_io(|| sleep(ms(50)));
        let x = get_mailbox::<M4>();
        say(&log, format!("got {}", x.0));
        lift_io(|| delete_mailbox::<M4>())
    };
    let late = tdo! { lift_io(|| sleep(ms(100))); lift_io(|| put_mailbox(M4(2))); empty::<()>() };
    assert_eq!(ends(lost | reader | late), Some(1));
    assert_eq!(lines(&log), ["put 1 sin lector", "got 2"]);
}

#[derive(Clone)]
struct M5(i32);

#[test]
fn m5_two_readers_of_a_mailbox_are_called_last_in_first() {
    let log = Log::default();
    let a = tdo! { [log] let x = get_mailbox::<M5>(); say(&log, format!("A {}", x.0)) };
    let b = tdo! { [log] let x = get_mailbox::<M5>(); say(&log, format!("B {}", x.0)) };
    let w = tdo! { lift_io(|| last_put_mailbox(M5(1))); empty::<()>() };
    assert_eq!(ends(a | b | w), Some(2));
    assert_eq!(lines(&log), ["B 1", "A 1"]);
}

#[derive(Clone)]
struct M6(i32);

#[test]
fn m6_write_directly_to_the_evar_of_a_mailbox() {
    let log = Log::default();
    let reader = tdo! { [log] let x = get_mailbox::<M6>(); say(&log, format!("got {}", x.0)); lift_io(|| delete_mailbox::<M6>()) };
    let writer = tdo! { lift_io(|| get_evar_from_mailbox::<M6>().write(M6(5))); empty::<()>() };
    assert_eq!(ends(reader | writer), Some(1));
    assert_eq!(lines(&log), ["got 5"]);
}

#[derive(Clone)]
struct M7(i32);

// `putMailbox` can be called from any thread, also from outside Transient: the reader goes on in that thread
#[test]
fn a_mailbox_can_be_written_from_an_external_thread() {
    let seen: Arc<Mutex<Vec<thread::ThreadId>>> = Arc::default();
    let s = seen.clone();
    let t = tdo! { [s]
        let x = get_mailbox::<M7>();
        lift_io(move || s.lock().unwrap().push(thread::current().id()));
        exit(x.0)
    };
    let external = thread::spawn(|| {
        sleep(ms(50));
        put_mailbox(M7(9));
        thread::current().id()
    });
    assert_eq!(keep_with(|| Cursor::new(""), t), Ok(9));
    assert_eq!(*seen.lock().unwrap(), [external.join().unwrap()]);
}

#[test]
fn delete_this_mailbox_reader_leaves_the_other_readers() {
    #[derive(Clone)]
    struct M8(i32);
    let log = Log::default();
    let a = tdo! { [log] let x = get_mailbox::<M8>(); say(&log, format!("A {}", x.0)) };
    let b = tdo! { [log] let x = get_mailbox::<M8>(); say(&log, format!("B {}", x.0)); delete_this_mailbox_reader::<M8>() };
    let w = tdo! {
        lift_io(|| { put_mailbox(M8(1)); put_mailbox(M8(2)); delete_mailbox::<M8>() });
        empty::<()>()
    };
    assert_eq!(ends(a | b | w), Some(3));
    // B is removed after the first one; A keeps reading
    assert_eq!(lines(&log), ["B 1", "A 1", "A 2"]);
}

// ---- on_finish: the experiments of the original

fn work(log: &Log, i: u64) -> Trans<()> {
    tdo! { [log] abduce(); lift_io(move || sleep(ms(i * 20))); say(&log, format!("work {i}")) }
}

#[test]
fn on_finish_one_registration_runs_once_when_all_the_threads_are_done() {
    let log = Log::default();
    let l = log.clone();
    let t = tdo! { [log]
        on_finish(move |_| say(&l, "FINISH"));
        work(&log, 1) | work(&log, 2) | work(&log, 3)
    };
    assert_eq!(keep_all(t).len(), 3);
    assert_eq!(lines(&log), ["work 1", "work 2", "work 3", "FINISH"]);
}

#[test]
fn on_finish_runs_once_for_each_time_its_statement_is_executed() {
    let log = Log::default();
    let n = std::sync::atomic::AtomicI32::new(0);
    let events = parallel(move || match n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1 {
        i @ 1..=3 => StreamData::More(i as u64),
        _ => StreamData::Done,
    })
    .map(|s| if let StreamData::More(i) = s { i } else { 0 });
    let t = tdo! { [log]
        let i = events;
        on_finish({ let l = log.clone(); move |_| say(&l, format!("FINISH {i}")) });
        lift_io(move || sleep(ms(i * 20)));
        say(&log, format!("work {i}"))
    };
    keep_all(t);
    assert_eq!(lines(&log), ["work 1", "FINISH 1", "work 2", "FINISH 2", "work 3", "FINISH 3"]);
}
