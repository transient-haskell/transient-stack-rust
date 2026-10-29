//! State (`modify_data`, `try_`, `sandbox`, indexed data), operators (`seq_right`, `at_end`, `at_end_again`,
//! `both`), threads (`add_threads`, `hooked_threads`, `on_wait_threads`) and utilities (`open_close`, `local_back`).
//! Each test reproduces an experiment that was run against the original in Haskell (Exp11.hs and Exp12.hs).

use std::io::Cursor;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
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

// ---- state

#[test]
fn s1_modify_data_with_inserts_or_applies() {
    let log = Log::default();
    let t = tdo! { [log]
        let r1 = modify_data_with(|h: String| h + " world", "hello new".to_string());
        let v1 = get_sdata::<String>();
        say(&log, format!("sin previo: devuelve {r1:?}, guardado {:?}", *v1));
        set_data("hello".to_string());
        let r2 = modify_data_with(|h: String| h + " world", "hello new".to_string());
        let v2 = get_sdata::<String>();
        say(&log, format!("con previo: devuelve {r2:?}, guardado {:?}", *v2))
    };
    keep_all(t);
    assert_eq!(
        lines(&log),
        ["sin previo: devuelve \"hello new\", guardado \"hello new\"", "con previo: devuelve \"hello world\", guardado \"hello world\""]
    );
}

#[test]
fn s2_modify_data_updates_or_deletes() {
    let log = Log::default();
    let t = tdo! { [log]
        set_data(5i32);
        modify_data(|m: Option<i32>| m.map(|x| x + 1));
        let a = get_sdata::<i32>();
        modify_data(|_: Option<i32>| None);
        let b = get_data::<i32>();
        modify_data(|m: Option<i32>| Some(m.unwrap_or(100)));
        let c = get_sdata::<i32>();
        say(&log, format!("{:?}", (*a, b.as_deref().copied(), *c)))
    };
    keep_all(t);
    assert_eq!(lines(&log), ["(6, None, 100)"]);
}

#[test]
fn s3_try_restores_on_failure_and_sandbox_always() {
    let log = Log::default();
    let t = tdo! { [log]
        set_data("base".to_string());
        try_(tdo! { set_data("cambiado".to_string()); empty::<()>() }) | pure(());
        let a = get_sdata::<String>();
        sandbox(set_data("dentro".to_string()));
        let b = get_sdata::<String>();
        try_(set_data("ok".to_string()));
        let c = get_sdata::<String>();
        say(&log, format!("{:?}", (a.as_str(), b.as_str(), c.as_str())))
    };
    keep_all(t);
    assert_eq!(lines(&log), ["(\"base\", \"base\", \"ok\")"]);
}

#[test]
fn s4_indexed_data_pure_per_branch_and_mutable_shared() {
    let log = Log::default();
    let t = tdo! { [log]
        set_index_data(1i32, "uno".to_string());
        set_index_data(2i32, "dos".to_string());
        let a = get_index_data::<i32, String>(1);
        let b = get_index_data::<i32, String>(3);
        set_index_rdata(7i32, "siete".to_string());
        with_index_rdata(7i32, |s: String| s + "!", "x".to_string());
        with_index_rdata(8i32, |s: String| s + "!", "ocho".to_string());
        abduce();
        set_index_data(1i32, "cambiado en la rama".to_string());
        set_index_rdata(9i32, "nueve".to_string());
        let r7 = get_index_rdata::<i32, String>(7);
        let r8 = get_index_rdata::<i32, String>(8);
        let r9 = get_index_rdata::<i32, String>(9);
        say(&log, format!("{:?}", (a, b, r7, r8, r9)))
    };
    keep_all(t);
    assert_eq!(
        lines(&log),
        ["(Some(\"uno\"), None, Some(\"siete!\"), Some(\"ocho\"), Some(\"nueve\"))"]
    );
}

#[test]
fn s5_the_mutable_index_is_shared_with_the_branches_and_the_pure_one_is_not() {
    let log = Log::default();
    let t = tdo! { [log]
        set_index_rdata(1i32, "compartido".to_string());
        set_index_data(1i32, "puro".to_string());
        fork(tdo! { set_index_rdata(2i32, "desde la rama".to_string()); set_index_data(1i32, "cambiado".to_string()) });
        lift_io(|| sleep(ms(40)));
        let r = get_index_rdata::<i32, String>(2);
        let p = get_index_data::<i32, String>(1);
        say(&log, format!("{r:?} {p:?}"))
    };
    keep_all(t);
    assert_eq!(lines(&log), ["Some(\"desde la rama\") Some(\"puro\")"]);
}

// ---- operators

#[test]
fn o1_seq_right_runs_the_second_even_if_the_first_is_empty() {
    let log = Log::default();
    let t = tdo! { [log]
        let r = seq_right(tdo! { say(&log, "a"); empty::<i32>() }, tdo! { say(&log, "b"); pure(2) });
        say(&log, format!("r {r}"))
    };
    keep_all(t);
    assert_eq!(lines(&log), ["a", "b", "r 2"]);
}

#[test]
fn o2_at_end_runs_mb_after_each_thread_of_ma_before_what_follows() {
    let log = Log::default();
    let t = tdo! { [log]
        let x = at_end(choose(vec![1, 2, 3]), say(&log, "  mb"));
        say(&log, format!("x {x}"))
    };
    assert_eq!(keep_all(t).len(), 3);
    let l = lines(&log);
    // one direct execution in the calling thread and one per thread: 4 in total
    assert_eq!(l.iter().filter(|s| s.as_str() == "  mb").count(), 4);
    let mut xs: Vec<_> = l.iter().filter(|s| s.starts_with("x ")).cloned().collect();
    xs.sort();
    assert_eq!(xs, ["x 1", "x 2", "x 3"]);
    // before each "x i" there was already at least one "mb" from its thread: the first "x" never goes before the first "mb"
    assert_eq!(l.iter().position(|s| s == "  mb") < l.iter().position(|s| s.starts_with("x ")), true);
}

#[test]
fn o2b_at_end_with_a_synchronous_ma_runs_mb_once() {
    let log = Log::default();
    let t = tdo! { [log]
        let x = at_end(tdo! { say(&log, "ma"); pure(1) }, say(&log, "  mb"));
        say(&log, format!("x {x}"))
    };
    keep_all(t);
    assert_eq!(lines(&log), ["ma", "  mb", "x 1"]);
}

#[test]
fn o3_both_continues_with_each_result_in_this_thread() {
    let log = Log::default();
    let t = tdo! { [log]
        let r = both(tdo! { say(&log, "uno"); pure(1) }, tdo! { say(&log, "dos"); pure(2) });
        say(&log, format!("r {r}"))
    };
    assert_eq!(keep_all(t).len(), 2);
    assert_eq!(lines(&log), ["uno", "r 1", "dos", "r 2"]);
}

#[test]
fn o3b_or_only_runs_the_second_when_the_first_gives_nothing() {
    let log = Log::default();
    let t = tdo! { [log]
        let r = tdo! { say(&log, "uno"); pure(1) } | tdo! { say(&log, "dos"); pure(2) };
        say(&log, format!("r {r}"))
    };
    assert_eq!(keep_all(t).len(), 1);
    assert_eq!(lines(&log), ["uno", "r 1"]);
}

#[test]
fn o4_at_end_again_runs_ma_again_for_each_event_of_mb() {
    let log = Log::default();
    let n = Arc::new(AtomicI32::new(0));
    let ev = EVar::<()>::new();
    let ma = {
        let (log, n) = (log.clone(), n.clone());
        lift_io(move || {
            let k = n.fetch_add(1, Ordering::SeqCst) + 1;
            log.lock().unwrap().push(format!("ma ejecutado {k}"));
            k
        })
    };
    let first = tdo! { [log, ev]
        let r = at_end_again(ma, read_evar(&ev));
        say(&log, format!("r {r}"));
        empty::<()>()
    };
    let writer = tdo! { [ev]
        lift_io(|| sleep(ms(20)));
        lift_io({ let ev = ev.clone(); move || { ev.write(()); ev.write(()); ev.clean() } });
        empty::<()>()
    };
    assert_eq!(keep_all(first | writer).len(), 0);
    assert_eq!(lines(&log), ["ma ejecutado 1", "r 1", "ma ejecutado 2", "r 2", "ma ejecutado 3", "r 3"]);
}

// ---- threads

fn peak_of(limit_and_add: impl Fn() -> Trans<i32>) -> i32 {
    let (cur, peak) = (Arc::new(AtomicI32::new(0)), Arc::new(AtomicI32::new(0)));
    let (c2, p2) = (cur.clone(), peak.clone());
    let t = tdo! { [c2, p2]
        let _x = limit_and_add();
        lift_io(move || {
            let now = c2.fetch_add(1, Ordering::SeqCst) + 1;
            p2.fetch_max(now, Ordering::SeqCst);
            sleep(ms(60));
            c2.fetch_sub(1, Ordering::SeqCst);
        })
    };
    keep_all(t);
    peak.load(Ordering::SeqCst)
}

#[test]
fn t1c_add_threads_before_launching_raises_the_limit() {
    // threads 1 and addThreads 3: peak 3
    assert_eq!(peak_of(|| threads(1, tdo! { add_threads(3); choose(vec![1, 2, 3, 4]) })), 3);
}

#[test]
fn t1d_add_threads_never_lowers_the_limit() {
    // threads 3 and addThreads 2: stays at 3
    assert_eq!(peak_of(|| threads(3, tdo! { add_threads(2); choose(vec![1, 2, 3, 4, 5]) })), 3);
}

#[test]
fn t1b_without_add_threads_one_at_a_time() {
    assert_eq!(peak_of(|| threads(1, choose(vec![1, 2, 3, 4]))), 1);
}

#[test]
fn t2_hooked_threads_inside_free_threads_are_waited_for() {
    let log = Log::default();
    let t = tdo! { [log]
        free_threads(hooked_threads(async_(|| sleep(ms(150)))));
        say(&log, "tras hooked")
    };
    assert_eq!(keep_all(t).len(), 1);
    assert_eq!(lines(&log), ["tras hooked"]);
}

// Experiment T4. Haskell's `onWaitThreads` ran when the FIRST thread ended ("work 1", "WAITTHREADS",
// "work 2", "work 3") because its check (`noactiveth`) always returned True; it was fixed in Haskell and now
// matches this test: it runs when all the threads that hang from that point are stopped.
#[test]
fn t4_on_wait_threads_runs_when_all_the_threads_are_stopped() {
    let log = Log::default();
    let l = log.clone();
    let t = tdo! { [log]
        on_wait_threads(move |_| say(&l, "WAITTHREADS"));
        let x = choose(vec![1u64, 2, 3]);
        lift_io(move || sleep(ms(x * 40)));
        say(&log, format!("work {x}"))
    };
    keep_all(t);
    assert_eq!(lines(&log), ["work 1", "work 2", "work 3", "WAITTHREADS"]);
}

#[test]
fn t3_on_wait_threads_does_not_wait_for_the_listeners() {
    // a live listener keeps the tree (on_finish would not run), but the threads are stopped: it runs
    let log = Log::default();
    let (l, ev) = (log.clone(), EVar::<i32>::new());
    let t = tdo! { [log, ev]
        on_wait_threads(move |_| tdo! { say(&l, "WAITTHREADS"); exit(()) });
        let x = choose(vec![1u64, 2]);
        lift_io(move || sleep(ms(x * 30)));
        say(&log, format!("work {x}"));
        let _r = read_evar(&ev);
        pure(())
    };
    let r: Result<(), String> = keep_with(|| Cursor::new(""), t);
    assert_eq!(r, Ok(()));
    assert_eq!(lines(&log), ["work 1", "work 2", "WAITTHREADS"]);
}

// ---- utilities

#[test]
fn oc_open_close_closes_when_the_threads_are_done() {
    let log = Log::default();
    let l = log.clone();
    let t = tdo! { [log]
        let r = open_close(tdo! { say(&log, "  open"); pure(7) }, move |v| say(&l, format!("  close {v}")));
        let x = choose(vec![1u64, 2]);
        lift_io(move || sleep(ms(x * 30)));
        say(&log, format!("usa {:?}", (r, x)))
    };
    assert_eq!(keep_all(t).len(), 2);
    assert_eq!(lines(&log), ["  open", "usa (7, 1)", "usa (7, 2)", "  close 7"]);
}

// Exp11 L1: with an `abduce` inside and an alternative, the original restores in the calling thread before the alternative:
// the handler inside no longer exists and only the one outside fires.
#[test]
fn l1_local_back_restores_in_the_calling_thread_even_if_the_body_goes_on_in_another() {
    let log = Log::default();
    let (l1, l2) = (log.clone(), log.clone());
    let t = tdo! { [l2]
        on_undo(pure(()), say(&l1, "  outer"));
        local_back::<(), ()>(tdo! { on_undo(pure(()), say(&l2, "  inner")); abduce() }) | undo::<()>()
    };
    assert_eq!(keep_all(t).len(), 1);
    assert_eq!(lines(&log), ["  outer"]);
}

#[test]
fn gen_id_counts_in_the_state_and_is_copied_when_forking() {
    let log = Log::default();
    let t = tdo! { [log]
        let a = gen_id();
        let b = gen_id();
        abduce();
        let c = gen_id();
        say(&log, format!("{a} {b} {c}"))
    };
    keep_all(t);
    assert_eq!(lines(&log), ["0 1 2"]);
}

#[test]
fn label_state_shows_in_the_tree_of_threads() {
    let log = Log::default();
    let t = tdo! { [log]
        label_state("mi hilo");
        let h = thread_handle();
        say(&log, h.show_threads())
    };
    keep_all(t);
    assert!(lines(&log)[0].contains("mi hilo"), "{:?}", lines(&log));
}

// Exp11 T1: in Haskell `addThreads` inside the continuation (already outside the scope of `threads`) has no effect: the
// rest of the program runs with the previous limit, because `threads` is `t <*** restore`. Peak 1, as without addThreads.
#[test]
fn t1_add_threads_after_the_scope_of_threads_has_no_effect() {
    let peak = peak_of(|| tdo! { let x = threads(1, choose(vec![1, 2, 3, 4])); add_threads(3); pure(x) });
    assert_eq!(peak, 1);
}

// And for the same reason, what is launched after the scope does not inherit the limit: a second `choose` is not limited
// and does not block waiting for the slot its own thread occupies.
#[test]
fn t5_threads_scope_does_not_leak_to_the_rest_of_the_program() {
    let log = Log::default();
    let t = tdo! { [log]
        let x = threads(1, choose(vec![1, 2]));
        let y = choose(vec![10, 20]);
        say(&log, format!("{}", x + y))
    };
    assert_eq!(keep_all(t).len(), 4);
    let mut l = lines(&log);
    l.sort();
    assert_eq!(l, ["11", "12", "21", "22"]);
}
