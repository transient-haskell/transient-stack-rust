//! `Transient.Indeterminism` and `sync`: injecting lists of values, grouping results and waiting for threads.
//! Each primitive has been checked by running the original (see the tests).

use super::*;
use std::time::Duration;

/// One thread per element (`choose`): each one goes on with the rest of the computation in a new thread.
/// `threads` limits the parallelism. Several `choose` can be composed algebraically.
pub fn choose<A: Clone + Send + Sync + 'static>(xs: Vec<A>) -> Trans<A> {
    xs.into_iter().rev().fold(empty(), |rest, x| or(async_(move || x.clone()), rest))
}

/// Walks the list **in this thread and in order**: the rest of the computation runs once per element and
/// then the computation stops (`empty`), so an alternative (`|`) runs after the loop. If inside
/// there is an asynchronous task (`async_`, `abduce`), the loop goes on without waiting for it (`for`).
pub fn for_<A: Clone + Send + Sync + 'static>(xs: Vec<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        for x in &xs {
            run_cont_loop(ctx, Box::new(x.clone()));
        }
        None
    })
}

/// Delivers the results of `proc` in groups of `n`. Each group comes out with the last result first
/// (`[3,2,1]`), as in the original (`group`).
pub fn group<A: Send + 'static>(num: usize, proc: Trans<A>) -> Trans<Vec<A>> {
    Trans::new(move |ctx| {
        let acc: Arc<Mutex<Vec<A>>> = Arc::default();
        tdo! {
            [acc]
            let x = proc.clone().map(Taken::new);
            Trans::new(move |_| {
                let mut g = acc.lock().unwrap();
                g.insert(0, x.take()?);
                (g.len() == num).then(|| std::mem::take(&mut *g))
            })
        }
        .run(ctx)
    })
}

/// A monoid: what is needed to join the results of an interval (`Monoid`)
pub trait Monoid {
    fn mempty() -> Self;
    fn mappend(self, other: Self) -> Self;
}

impl<T> Monoid for Vec<T> {
    fn mempty() -> Self {
        Vec::new()
    }
    fn mappend(mut self, mut other: Self) -> Self {
        self.append(&mut other);
        self
    }
}

impl Monoid for String {
    fn mempty() -> Self {
        String::new()
    }
    fn mappend(mut self, other: Self) -> Self {
        self.push_str(&other);
        self
    }
}

impl Monoid for () {
    fn mempty() -> Self {}
    fn mappend(self, _: Self) -> Self {}
}

/// Joins everything that `comp` gives in each interval and delivers it when the interval ends (`groupByTime`)
pub fn group_by_time<A: Monoid + Send + 'static>(interval: Duration, comp: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let v = Arc::new(Mutex::new(A::mempty()));
        let run = {
            let v = v.clone();
            tdo! {
                [v]
                let x = comp.clone().map(Taken::new);
                Trans::new(move |_| {
                    let mut g = v.lock().unwrap();
                    let old = std::mem::replace(&mut *g, A::mempty());
                    *g = old.mappend(x.take()?);
                    None::<A>
                })
            }
        };
        let gather = {
            let v = v.clone();
            wait_events(move || {
                thread::sleep(interval);
                std::mem::replace(&mut *v.lock().unwrap(), A::mempty())
            })
        };
        or(gather, run).run(ctx)
    })
}

/// Waits, **in this thread**, for `x` and all the threads it launches to end, and gives the list of results. If there is
/// none, the computation stops and the alternative runs. With `n > 0` it waits only for `n` results and with
/// `time` it waits at most that time (`syncCollect`).
pub fn sync_collect<A: Send + 'static>(n: usize, time: Option<Duration>, x: Trans<A>) -> Trans<Vec<A>> {
    Trans::new(move |ctx| {
        let (tx, rx) = mpsc::channel();
        let tx = Arc::new(Mutex::new(tx));
        let first = tdo! {
            [tx]
            let r = collect_for(n, time, x.clone()).map(Taken::new);
            Trans::new(move |_| {
                let _ = tx.lock().unwrap().send(r.take()?);
                None::<()>
            })
        };
        first.run(ctx);
        // `collect` goes on in another thread when it ends; this one waits here for its list
        rx.recv().ok().filter(|r| !r.is_empty())
    })
}

/// `sync_collect` with no limit of results or time (`sync`)
pub fn sync<A: Send + 'static>(x: Trans<A>) -> Trans<Vec<A>> {
    sync_collect(0, None, x)
}

/// Another name for `sync` (`await`)
pub fn await_<A: Send + 'static>(x: Trans<A>) -> Trans<Vec<A>> {
    sync(x)
}

/// The first result; it cancels the other threads of `x` (`sync1`)
pub fn sync1<A: Send + 'static>(x: Trans<A>) -> Trans<A> {
    sync_collect(1, None, x).map(|mut rs| rs.remove(0))
}

/// Fails (stops) if `proc` gives no result in `time` (`timeout`)
pub fn timeout<A: Send + 'static>(time: Duration, proc: Trans<A>) -> Trans<A> {
    sync_collect(1, Some(time), proc).map(|mut rs| rs.remove(0))
}

/// A stream with a `More(r)` for each result of `comp` and a `Done` each time `timeout` passes without any
/// arriving (`burst`). Each new result cancels the timer of the previous one (`one_thread`), so `Done` only
/// comes out after a silence.
///
/// (Haskell's `burst` never emitted `Done`: it used `<|>`, which does not run the second branch if the first gives a
/// result. It was fixed in the same session with `<->`.)
pub fn burst<A: Clone + Send + Sync + 'static>(timeout: Duration, comp: Trans<A>) -> Trans<StreamData<A>> {
    tdo! {
        let r = one_thread(comp);
        both(pure(StreamData::More(r)), tdo! { async_(move || thread::sleep(timeout)); pure(StreamData::Done) })
    }
}
