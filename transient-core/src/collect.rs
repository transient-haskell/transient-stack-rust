//! `collect`: gathers the results of all the threads born from a computation and goes on, with the list, when
//! none is alive any more (or `n` have been gathered, or the time ran out, or someone calls `end_collect`).

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Gathers the results of `t` and of all the threads it launches, until none is alive. With `n > 0`
/// it ends when `n` results are gathered and cancels what is left. The rest of the computation goes on with the list in
/// another thread; the one that calls `collect` ends (it looks like `empty`), as in Haskell.
///
/// The listeners (`option`, `input`) keep the `collect` alive: use `option1` or `end_collect`.
pub fn collect<A: Send + 'static>(n: usize, t: Trans<A>) -> Trans<Vec<A>> {
    collect_for(n, None, t)
}

/// Like `collect`, and it also ends when `time` passes
pub fn collect_for<A: Send + 'static>(n: usize, time: Option<Duration>, t: Trans<A>) -> Trans<Vec<A>> {
    Trans::new(move |ctx| {
        let results: Arc<Mutex<Vec<A>>> = Arc::default();
        let done = Arc::new(AtomicBool::new(false));
        // groups the threads of the gathered computation; it runs no thread of its own
        let scope = TNode::child(&ctx.node, false);

        // ends the `collect` only once: cancels what is left and goes on, in a thread that hangs from the node
        // of the caller, with the list of results
        let finish: Arc<dyn Fn() + Send + Sync> = {
            let (results, done, scope, rest) = (results.clone(), done.clone(), scope.clone(), ctx.clone());
            Arc::new(move || {
                if done.swap(true, Ordering::SeqCst) {
                    return;
                }
                scope.kill_children();
                let r = std::mem::take(&mut *results.lock().unwrap());
                detach(&mut rest.clone(), Box::new(r));
            })
        };
        let f = finish.clone();
        scope.on_idle(move || f());

        // the gathered computation and the timer, in threads of the scope with no thread limit (`anyThreads`)
        let collected = {
            let (results, finish) = (results.clone(), finish.clone());
            tdo! {
                [results, finish]
                let a = t.clone().map(Taken::new);
                Trans::new(move |_| {
                    let reached = {
                        let mut r = results.lock().unwrap();
                        r.push(a.take()?);
                        n > 0 && r.len() >= n
                    };
                    if reached {
                        finish();
                    }
                    None::<()>
                })
            }
        };
        let mut c = ctx.clone();
        c.node = scope.clone();
        c.max_thread = None;
        c.collect_end = Some(finish.clone());
        c.fcomp = Cons::nil().push(erase(Arc::new(move |_: ()| collected.clone())));
        if let Some(time) = time {
            let (mut timer, finish) = (c.clone(), finish.clone());
            timer.fcomp = Cons::nil().push(erase(Arc::new(move |_: ()| {
                let finish = finish.clone();
                lift_io(move || {
                    thread::sleep(time);
                    finish();
                })
            })));
            detach(&mut timer, Box::new(()));
        }
        detach(&mut c, Box::new(()));
        None
    })
}

/// Ends the innermost `collect` that contains this computation, with the results gathered so far.
/// It is the way to stop a `collect` whose listeners would never end.
pub fn end_collect() -> Trans<()> {
    Trans::new(|ctx| {
        (ctx.collect_end.clone().expect("end_collect: not inside a collect"))();
        None
    })
}
