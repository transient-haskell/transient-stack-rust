//! `logged` (`Transient.Move.Logged`): the log of an execution, its replay and the hash of the path.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use transient_core::*;

fn prog(count: Arc<AtomicUsize>) -> Trans<i64> {
    let step = move |n: i64| {
        let c = count.clone();
        logged(lift_io(move || {
            c.fetch_add(1, Ordering::SeqCst);
            n
        }))
    };
    tdo! {
        [step]
        let a = step(1);
        let b = step(20);
        let c = step(300);
        pure(a + b + c)
    }
}

fn run(t: Trans<i64>, ctx: &mut Ctx) -> Option<i64> {
    t.run(ctx)
}

#[test]
fn log_text_and_replay() {
    let count = Arc::new(AtomicUsize::new(0));
    let mut ctx = Ctx::new();
    assert_eq!(run(prog(count.clone()), &mut ctx), Some(321));
    assert_eq!(count.load(Ordering::SeqCst), 3);
    let log = get_log().run(&mut ctx).unwrap();
    assert_eq!(log.part_log.render(), "1/20/300/");

    // replay: nothing is executed, the values come from the log
    let mut ctx2 = Ctx::new();
    set_parse_string(&log.part_log.render()).run(&mut ctx2);
    assert_eq!(run(prog(count.clone()), &mut ctx2), Some(321));
    assert_eq!(count.load(Ordering::SeqCst), 3);
    let log2 = get_log().run(&mut ctx2).unwrap();
    assert_eq!(log2.part_log.render(), "1/20/300/");
    assert_eq!(log2.hash, log.hash);
}

#[test]
fn partial_log_resumes() {
    let count = Arc::new(AtomicUsize::new(0));
    let mut ctx = Ctx::new();
    set_parse_string("1/20/").run(&mut ctx);
    assert_eq!(run(prog(count.clone()), &mut ctx), Some(321));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn path_hash_golden() {
    let mut t = logged(pure(0i64));
    for _ in 0..11 {
        t = tdo! { t; logged(pure(0i64)) };
    }
    let mut ctx = Ctx::new();
    t.run(&mut ctx);
    assert_eq!(get_log().run(&mut ctx).unwrap().hash, 148057647);
}

#[test]
fn cloud_replays_like_trans() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let step = move |n: i64| {
        let c = c.clone();
        local_io(move || {
            c.fetch_add(1, Ordering::SeqCst);
            n
        })
    };
    let prog = cdo! {
        [step]
        let a = step(1);
        let b = step(20);
        let _ = on_all(lift_io(|| ()));
        cpure(a + b)
    };
    let mut ctx = Ctx::new();
    assert_eq!(prog.run(&mut ctx), Some(21));
    let log = get_log().run(&mut ctx).unwrap();
    assert_eq!(log.part_log.render(), "1/20/");
    let mut ctx2 = Ctx::new();
    set_parse_string(&log.part_log.render()).run(&mut ctx2);
    assert_eq!(prog.run(&mut ctx2), Some(21));
    assert_eq!(count.load(Ordering::SeqCst), 2);
}
