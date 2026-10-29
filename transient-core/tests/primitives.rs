use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use transient_core::*;

fn sorted<A: Ord>(mut v: Vec<A>) -> Vec<A> {
    v.sort();
    v
}

// Core.hs:970:  keep' $ (abduce >> return "hello") <|> return "world"   ==>  ["world","hello"]
#[test]
fn abduce_looks_empty_to_the_alternative() {
    let t = tdo! { abduce(); pure("hello") } | pure("world");
    assert_eq!(sorted(keep_all(t)), ["hello", "world"]);
}

// Core.hs:962:  keep' $ sync (abduce >> empty) <|> return "hello"   ==>  ["hello"]   (here without sync)
#[test]
fn alternative_runs_after_abduce_empty() {
    let t = tdo! { abduce(); empty::<&str>() } | pure("hello");
    assert_eq!(keep_all(t), ["hello"]);
}

// two abduce in a row: the code after them runs once (the stack of the new thread is not duplicated)
#[test]
fn nested_abduce_runs_the_rest_once() {
    let n = Arc::new(AtomicUsize::new(0));
    let m = n.clone();
    let t = tdo! { [m]
        abduce();
        abduce();
        lift_io(move || m.fetch_add(1, Ordering::SeqCst))
    };
    assert_eq!(keep_all(t).len(), 1);
    assert_eq!(n.load(Ordering::SeqCst), 1);
}

#[test]
fn applicative_synchronous_terms() {
    assert_eq!(keep_all(pure(1) + pure(2)), [3]);
}

#[test]
fn applicative_parallel_terms_give_the_result() {
    // if both finish at the same time the result may come out twice (as in Haskell), but it never goes missing or differs
    for _ in 0..200 {
        let a = tdo! { abduce(); pure(1) };
        let b = tdo! { abduce(); pure(2) };
        let r = keep_all(a + b);
        assert!((1..=2).contains(&r.len()) && r.iter().all(|&x| x == 3), "{r:?}");
    }
}

// with one of the terms slower there is no race: a single result
#[test]
fn applicative_parallel_terms_with_different_speed_give_one_result() {
    let slow = tdo! { abduce(); lift_io(|| { std::thread::sleep(std::time::Duration::from_millis(20)); 1 }) };
    let fast = tdo! { abduce(); pure(2) };
    assert_eq!(keep_all(slow + fast), [3]);
}

#[test]
fn applicative_operators_compose() {
    let a = tdo! { abduce(); pure(1) };
    let b = tdo! { abduce(); pure(2) };
    let c = pure(10);
    assert_eq!(keep_all(a + b + c), [13]);
}

// `if/else` and `match` are Rust expressions: each branch is a computation of the same type
#[test]
fn if_else_and_match_inside_tdo() {
    let t = |n: i32| {
        tdo! {
            let x = tdo! { abduce(); pure(n) };
            let size = if x > 3 { pure("big") } else { pure("small") };
            if x % 2 == 0 { pure(()) } else { lift_io(|| ()) };
            match size {
                "big" => pure(x * 10),
                _ => pure(x),
            }
        }
    };
    assert_eq!(keep_all(t(5)), [50]);
    assert_eq!(keep_all(t(2)), [2]);
}

// ---- threads, async_, parallel, wait_events, fork

use std::sync::atomic::AtomicI32;
use std::thread;
use std::time::Duration;

#[test]
fn async_runs_in_another_thread() {
    let main = thread::current().id();
    let r = keep_all(async_(move || thread::current().id()));
    assert_eq!(r.len(), 1);
    assert_ne!(r[0], main);
}

// threads 0: the task starts in the current thread
#[test]
fn threads_zero_runs_the_task_in_the_current_thread() {
    let main = thread::current().id();
    let r = keep_all(threads(0, tdo! { abduce(); lift_io(|| thread::current().id()) }));
    assert_eq!(r, [main]);
}

// with threads 0 the `abduce` is synchronous: "hello" always arrives before "world", without depending on the threads
// (without threads 0 the order depends on who arrives first, as in Core.hs:970)
#[test]
fn threads_zero_abduce_is_synchronous() {
    let t = threads(0, tdo! { abduce(); pure("hello") }) | pure("world");
    assert_eq!(keep_all(t), ["hello", "world"]);
}

#[test]
fn parallel_gives_each_value_of_the_stream() {
    let n = AtomicI32::new(0);
    let stream = parallel(move || match n.fetch_add(1, Ordering::SeqCst) + 1 {
        i @ 1..=2 => StreamData::More(i),
        3 => StreamData::Last(3),
        _ => StreamData::Done,
    })
    .map(|s| match s {
        StreamData::More(x) | StreamData::Last(x) => x,
        StreamData::Done => -1,
    });
    assert_eq!(sorted(keep_all(stream)), [1, 2, 3]);
}

#[test]
fn parallel_done_gives_nothing() {
    assert_eq!(keep_all(parallel(|| StreamData::<i32>::Done)), []);
}

// threads n limits the simultaneous threads: the loop of the stream takes one slot and each event another
#[test]
fn threads_n_limits_concurrent_tasks() {
    let (running, peak) = (Arc::new(AtomicI32::new(0)), Arc::new(AtomicI32::new(0)));
    let n = Arc::new(AtomicI32::new(0));
    let t = threads(3, tdo! { [running, peak, n]
        let i = parallel(move || {
            let i = n.fetch_add(1, Ordering::SeqCst) + 1;
            if i <= 6 { StreamData::More(i) } else { StreamData::Done }
        });
        lift_io(move || {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(20));
            running.fetch_sub(1, Ordering::SeqCst);
            i.clone()
        })
    });
    assert_eq!(keep_all(t).len(), 6);
    assert_eq!(peak.load(Ordering::SeqCst), 2);
}

#[test]
fn wait_events_continues_each_event_in_its_own_thread() {
    let ids = Arc::new(std::sync::Mutex::new(Vec::new()));
    let n = AtomicI32::new(0);
    let src = move || {
        let i = n.fetch_add(1, Ordering::SeqCst);
        if i >= 3 { thread::park(); }       // the stream does not end: wait_events is infinite
        i
    };
    // wait_events never ends, so it is observed from outside with a channel
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = Arc::new(std::sync::Mutex::new(tx));
    let ids2 = ids.clone();
    thread::spawn(move || {
        let t = tdo! { [ids2]
            let i = wait_events(src);
            lift_io(move || { ids2.lock().unwrap().push(thread::current().id()); i })
        };
        let mut ctx = Ctx::new();
        tdo! {
            [tx]
            let i = t;
            lift_io(move || tx.lock().unwrap().send(i).unwrap());
            empty::<()>()
        }
        .run(&mut ctx);
    });
    let got = sorted((0..3).map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap()).collect());
    assert_eq!(got, [0, 1, 2]);
    let distinct: std::collections::HashSet<_> = ids.lock().unwrap().iter().cloned().collect();
    assert_eq!(distinct.len(), 3);
}

#[test]
fn fork_runs_in_parallel_and_continues() {
    let n = Arc::new(AtomicUsize::new(0));
    let t = tdo! { [n]
        fork(lift_io({ let n = n.clone(); move || { thread::sleep(Duration::from_millis(20)); n.fetch_add(1, Ordering::SeqCst); } }));
        pure("sigue")
    };
    assert_eq!(keep_all(t), ["sigue"]);
    assert_eq!(n.load(Ordering::SeqCst), 1);      // keep_all waits for the thread of the fork
}

// ---- keep and the console

use std::io::Cursor;

fn console(text: &'static str) -> impl FnOnce() -> Cursor<&'static str> + Send + 'static {
    move || Cursor::new(text)
}

#[test]
fn option_continues_when_typed_and_exit_returns_the_result() {
    let t = tdo! { option("go", "run"); exit(42) };
    assert_eq!(keep_with(console("go\n"), t), Ok(42));
}

#[test]
fn end_is_always_available() {
    let t = tdo! { option("never", "x"); exit(1) };
    assert_eq!(keep_with(console("end\n"), t), Err::<(), String>("end".to_string()));
}

#[test]
fn input_reads_a_typed_value_and_rejects_a_bad_one() {
    let t = tdo! { let n = input::<i32>("number? "); exit(n * 2) };
    assert_eq!(keep_with(console("abc 21\n"), t), Ok(42));
}

#[test]
fn option_stays_active_and_runs_the_rest_each_time() {
    let c = Arc::new(AtomicUsize::new(0));
    let t = tdo! { [c]
        option("inc", "add one");
        let n = lift_io(move || c.fetch_add(1, Ordering::SeqCst) + 1);
        if n == 3 { exit(n) } else { pure(()) }
    };
    assert_eq!(keep_with(console("inc inc inc\n"), t), Ok(3usize));
}

#[test]
fn nothing_left_to_happen_ends_keep() {
    assert_eq!(keep_with(console(""), pure(())), Err::<(), String>("No threads left".to_string()));
}

#[test]
fn keep_waits_for_a_task_that_calls_exit_later() {
    let t = tdo! { abduce(); lift_io(|| thread::sleep(Duration::from_millis(30))); exit("listo") };
    assert_eq!(keep_with(console(""), t), Ok("listo"));
}

// ---- the command line, as in Haskell

#[test]
fn only_the_old_notation_is_transformed_when_the_program_starts() {
    let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let line = |a: &[&str]| command_line(&args(a));
    assert_eq!(line(&["-p", "start/localhost/8080"]), Some("start/localhost/8080/".into()));
    // several arguments: they are joined
    assert_eq!(line(&["-p", "start/localhost/8080", "go"]), Some("start/localhost/8080/go/".into()));
    // up to the next option
    assert_eq!(line(&["-p", "start/localhost/8080", "--cookie", "abc"]), Some("start/localhost/8080/".into()));
    // a URL is one value
    assert_eq!(line(&["-p", "http://h:1/x"]), Some("\"http://h:1/x\"/".into()));
    // the options are not: each one finds its arguments when it is executed
    assert_eq!(line(&["--start", "localhost:8080"]), None);
    assert_eq!(line(&["--start", "localhost/8080"]), None);
    assert_eq!(line(&[]), None);
}

/// The options of `keep` are options like the others: the command line gives them too
#[test]
fn the_options_of_keep_can_be_given_in_the_command_line() {
    let args: Vec<String> = ["--options", "--end"].iter().map(|s| s.to_string()).collect();
    let t = tdo! { option("never", "x"); exit(1) };
    assert_eq!(keep_args_with(&args, || Cursor::new(""), t), Err::<i32, String>("end".to_string()));
}

/// An option looks for itself in the command line when it is executed: wherever it is in the command line and whenever it is executed
#[test]
fn an_option_finds_its_arguments_when_it_is_executed_late_or_out_of_order() {
    let args: Vec<String> = ["--second", "b:2", "--first", "a"].iter().map(|s| s.to_string()).collect();
    let t = tdo! {
        option1("first", "first");
        let x = input::<String>("x? ");
        lift_io(|| thread::sleep(Duration::from_millis(300)));
        option1("second", "second");
        let y = input::<String>("y? ");
        let n = input::<i32>("n? ");
        exit(format!("{x} {y} {n}"))
    };
    assert_eq!(keep_args_with(&args, || Cursor::new(""), t), Ok("a b 2".to_string()));
}

/// The two notations together, and an option that is not in the command line waits for the console as always
#[test]
fn the_old_notation_and_the_options_work_together() {
    let args: Vec<String> = ["-p", "first/a", "--second", "b", "--other", "x"].iter().map(|s| s.to_string()).collect();
    let t = tdo! {
        option1("first", "first");
        let x = input::<String>("x? ");
        option1("second", "second");
        let y = input::<String>("y? ");
        option1("third", "third");
        exit(format!("{x} {y}"))
    };
    // the third one is typed after a while, as a person does
    struct Typed(Option<&'static [u8]>);
    impl std::io::Read for Typed {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let Some(text) = self.0.take() else { return Ok(0) };
            thread::sleep(Duration::from_millis(1500));
            buf[..text.len()].copy_from_slice(text);
            Ok(text.len())
        }
    }
    assert_eq!(keep_args_with(&args, || std::io::BufReader::new(Typed(Some(b"third\n"))), t), Ok("a b".to_string()));
}

/// A console that gets those command lines starts the node at the same address
#[test]
fn the_forms_of_the_command_line_give_the_same_node() {
    for form in [&["-p", "start/localhost/9051"][..], &["--start", "localhost:9051"], &["--start", "localhost/9051"]] {
        let args: Vec<String> = form.iter().map(|s| s.to_string()).collect();
        let node: Result<Node, String> = keep_args_with(&args, || std::io::Cursor::new(""), tdo! { let n = get_node_params(); exit(n); empty::<()>() });
        assert_eq!(node, Ok(create_node("localhost", 9051)), "{form:?}");
    }
}
