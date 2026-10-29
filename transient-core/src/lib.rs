//! Transient core in Rust: `bind`, `or` (<|>), `abduce` and the applicative (`lift2`).
//!
//! Like in Haskell (`LEngine.hs`): a computation is direct style, `&mut Ctx -> Option<A>`, and the
//! continuation lives as data in the `fcomp` stack of the state. `abduce` copies the state, takes that stack to another
//! thread and in the current thread returns `None` (`empty`).

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::ops::{Add, BitOr};
use std::sync::atomic::AtomicUsize;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;

mod back;
mod collect;
mod console;
mod data;
mod evar;
mod exception;
mod http;
mod indeterminism;
mod job;
mod log;
mod cloud;
mod closure;
mod net;
mod node;
mod utils;
mod web;
mod wormhole;
mod ops;
mod parse;
mod react;
mod save;
mod tree;
pub use back::*;
pub use collect::*;
pub use console::*;
pub use data::*;
pub use evar::*;
pub use exception::*;
pub use http::*;
pub use indeterminism::*;
pub use job::*;
pub use log::*;
pub use cloud::*;
pub use closure::*;
pub use net::*;
pub use node::*;
pub use utils::*;
pub use web::*;
pub use wormhole::*;
pub use ops::*;
pub use parse::*;
use parse::ParseContext;
pub use transient_loggable::{dup_slash, from_log, loggable_json, to_log, undup_slash, Json, Loggable};
pub use react::*;
pub use save::*;
pub use tree::*;
use tree::TNode;

static ID_GEN: AtomicUsize = AtomicUsize::new(0);

/// A new identifier, unique in the whole program (`genNewId`)
pub fn gen_new_id() -> usize {
    ID_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

// ---------------------------------------------------------------------------------------------------------------
// Persistent list: cloning is O(1), like Haskell's `fcomp` list

struct Cons<T>(Option<Arc<(T, Cons<T>)>>);

impl<T> Clone for Cons<T> {
    fn clone(&self) -> Self {
        Cons(self.0.clone())
    }
}

impl<T> Cons<T> {
    fn nil() -> Self {
        Cons(None)
    }
    fn push(&self, x: T) -> Self {
        Cons(Some(Arc::new((x, self.clone()))))
    }
    fn uncons(&self) -> Option<(&T, &Cons<T>)> {
        self.0.as_ref().map(|n| (&n.0, &n.1))
    }
    fn tail(&self) -> Self {
        self.uncons().map_or_else(Cons::nil, |(_, t)| t.clone())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// State

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Serial,
    Parallel,
    Remote,
}

/// Thread slots of a `threads n` scope: shared by all the branches born inside
struct Slots {
    free: Mutex<usize>,
    cv: Condvar,
}

impl Slots {
    /// At least `n` free slots (`addThreads`)
    fn raise_to(&self, n: usize) {
        let mut free = self.free.lock().unwrap();
        *free = (*free).max(n);
        self.cv.notify_all();
    }
    /// `n` more free slots (`addThreads'`)
    fn add(&self, n: usize) {
        *self.free.lock().unwrap() += n;
        self.cv.notify_all();
    }
    fn new(n: usize) -> Arc<Self> {
        Arc::new(Slots { free: Mutex::new(n), cv: Condvar::new() })
    }
    fn acquire(&self) {
        let mut free = self.free.lock().unwrap();
        while *free == 0 {
            free = self.cv.wait(free).unwrap();
        }
        *free -= 1;
    }
}

/// Thread limit of the scope: `threads 0` starts the tasks in the current thread; `threads n`, up to n at a time
#[derive(Clone)]
enum MaxThreads {
    Sync,
    Limited(Arc<Slots>),
}

/// Gives its slot back to the `threads n` scope when the thread ends
struct SlotGuard(Option<Arc<Slots>>);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        if let Some(s) = &self.0 {
            *s.free.lock().unwrap() += 1;
            s.cv.notify_one();
        }
    }
}

/// Result of `exit` / `exit_left`, which `keep` waits for. The first one to arrive wins.
#[derive(Default)]
pub(crate) struct ExitSlot {
    value: Mutex<Option<Result<Dyn, String>>>,
    cv: Condvar,
}

impl ExitSlot {
    pub(crate) fn set(&self, r: Result<Dyn, String>) {
        self.value.lock().unwrap().get_or_insert(r);
        self.cv.notify_all();
    }
    pub(crate) fn wait(&self) -> Result<Dyn, String> {
        let mut v = self.value.lock().unwrap();
        loop {
            if let Some(r) = v.take() {
                return r;
            }
            v = self.cv.wait(v).unwrap();
        }
    }
}

/// The state that goes with the execution. Values are copied when forking; what is mutable is shared through `Arc`.
#[derive(Clone)]
pub struct Ctx {
    fcomp: Cons<Cont>,
    mode: Mode,
    max_thread: Option<MaxThreads>,
    /// the node of the current thread in the thread tree
    node: Arc<TNode>,
    /// the threads that are born do not hang from the tree (`free_threads`)
    free: bool,
    /// the data state: a map by type, persistent (copied on write if shared). The mutable
    /// cells (`RState`) are values of the map that hold an `Arc<Mutex<..>>` inside
    data: Arc<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    /// the listeners registered with `react_id`, by identifier
    callbacks: Arc<Callbacks>,
    console: Option<Arc<Console>>,
    exit: Option<Arc<ExitSlot>>,
    /// ends the innermost `collect` (`end_collect`)
    collect_end: Option<Arc<dyn Fn() + Send + Sync>>,
    /// the counter of identifiers of the state (`mfSequence`, `genId`)
    seq: usize,
    /// the text being parsed and where this branch is in it (`parseContext`)
    parse: ParseContext,
    /// the log of the distributed execution (`mfLog`)
    log: Log,
    /// the computation is inside the term of an applicative (`isApplicative`)
    is_applicative: bool,
}

impl Ctx {
    pub fn new() -> Self {
        Ctx {
            fcomp: Cons::nil(),
            mode: Mode::Serial,
            max_thread: None,
            node: TNode::root(),
            free: false,
            data: Arc::default(),
            callbacks: Arc::default(),
            console: None,
            exit: None,
            collect_end: None,
            seq: 0,
            parse: ParseContext::default(),
            log: Log::default(),
            is_applicative: false,
        }
    }
}

impl Default for Ctx {
    fn default() -> Self {
        Ctx::new()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Computations

type Dyn = Box<dyn Any + Send>;
type Cont = Arc<dyn Fn(Dyn) -> Trans<Dyn> + Send + Sync>;

/// A computation: it can be run several times and cloned
pub struct Trans<A>(Arc<dyn Fn(&mut Ctx) -> Option<A> + Send + Sync>);

impl<A> Clone for Trans<A> {
    fn clone(&self) -> Self {
        Trans(self.0.clone())
    }
}

impl<A: Send + 'static> Trans<A> {
    pub fn new(f: impl Fn(&mut Ctx) -> Option<A> + Send + Sync + 'static) -> Self {
        Trans(Arc::new(f))
    }
    pub fn run(&self, ctx: &mut Ctx) -> Option<A> {
        (self.0)(ctx)
    }
    /// `fmap`. The function is pushed as a continuation, like in `bind`: if `self` does an `abduce`, the rest applies it
    pub fn map<B: Send + 'static>(self, f: impl Fn(A) -> B + Send + Sync + 'static) -> Trans<B> {
        let f = Arc::new(f);
        let k: Cont = {
            let f = f.clone();
            Arc::new(move |d: Dyn| once(Box::new(f(*d.downcast::<A>().unwrap())) as Dyn))
        };
        Trans::new(move |ctx| {
            ctx.fcomp = ctx.fcomp.push(k.clone());
            let r = self.run(ctx);
            ctx.fcomp = ctx.fcomp.tail();
            r.map(&*f)
        })
    }
}

pub fn pure<A: Clone + Send + Sync + 'static>(a: A) -> Trans<A> {
    Trans::new(move |_| Some(a.clone()))
}

pub fn empty<A: Send + 'static>() -> Trans<A> {
    Trans::new(|_| None)
}

/// Runs an action in the current thread
pub fn lift_io<A: Send + 'static>(f: impl Fn() -> A + Send + Sync + 'static) -> Trans<A> {
    Trans::new(move |_| Some(f()))
}

/// A computation that gives its value only once: the continuations run once for each value they receive
fn once<A: Send + 'static>(a: A) -> Trans<A> {
    let a = Mutex::new(Some(a));
    Trans::new(move |_| a.lock().unwrap().take())
}

/// A value that can be taken once, and cloned: `tdo!` clones what it binds, and a value that is not `Clone` can travel in a
/// `Taken` (`let x = a.map(Taken::new); ... x.take()`)
pub(crate) struct Taken<A>(Arc<Mutex<Option<A>>>);

impl<A> Clone for Taken<A> {
    fn clone(&self) -> Self {
        Taken(self.0.clone())
    }
}

impl<A> Taken<A> {
    pub(crate) fn new(a: A) -> Self {
        Taken(Arc::new(Mutex::new(Some(a))))
    }
    /// The value, the first time
    pub(crate) fn take(&self) -> Option<A> {
        self.0.lock().unwrap().take()
    }
}

fn erase<A: Send + 'static, B: Send + 'static>(f: Arc<impl Fn(A) -> Trans<B> + Send + Sync + 'static>) -> Cont {
    Arc::new(move |d: Dyn| {
        let a = *d.downcast::<A>().unwrap_or_else(|_| panic!("expected {}", std::any::type_name::<A>()));
        let t = f(a);
        // it only erases the type: it is not a step of the program, so it is not pushed
        Trans::new(move |ctx| t.run(ctx).map(|b| Box::new(b) as Dyn))
    })
}

/// `x >>= f`: pushes `f` as a continuation while `x` runs
pub fn bind<A: Send + 'static, B: Send + 'static>(
    x: Trans<A>,
    f: impl Fn(A) -> Trans<B> + Send + Sync + 'static,
) -> Trans<B> {
    let f = Arc::new(f);
    Trans::new(move |ctx| {
        ctx.fcomp = ctx.fcomp.push(erase(f.clone()));
        let r = x.run(ctx);
        ctx.fcomp = ctx.fcomp.tail();
        r.filter(|_| !ctx.node.cancelled()).and_then(|a| f(a).run(ctx))
    })
}

/// Runs the stack of continuations `f1 >=> f2 >=> ... >=> const empty`. While `f1` runs, its continuation is
/// the rest of the stack: so an `abduce` inside `f1` takes exactly what is left.
fn run_stack(fs: &Cons<Cont>, x: Dyn, ctx: &mut Ctx) -> Option<Dyn> {
    let (f, rest) = fs.uncons().filter(|_| !ctx.node.cancelled())?;
    ctx.fcomp = rest.clone();
    let y = f(x).run(ctx)?;
    run_stack(rest, y, ctx)
}

/// `x <|> y`: `y` runs if `x` gives no result in this thread
pub fn or<A: Send + 'static>(x: Trans<A>, y: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let mx = x.run(ctx);
        if ctx.mode == Mode::Remote {
            None
        } else {
            mx.or_else(|| y.run(ctx))
        }
    })
}

/// End of a thread (`cleanupThreads`): if it has no children it starts the `Finish` backtracking, and its node becomes quiet
fn end_thread(ctx: &mut Ctx) {
    ctx.node.stop_running();
    if ctx.node.has_no_children() {
        let _ = back::<(), _>(Finish("thread ended".into())).run(ctx);
    }
    settle(&ctx.node);
}

/// Does `end_thread` at the end, also if the thread panics
struct EndOfThread<'a>(&'a mut Ctx);

impl Drop for EndOfThread<'_> {
    fn drop(&mut self) {
        end_thread(self.0);
    }
}

/// Each thread catches at its top what nobody has caught and turns it into exception backtracking, with the
/// state it had at the start (`runContCleanup ... catch backExceptIO`)
fn run_thread(c: &mut Ctx, fs: &Cons<Cont>, x: Dyn) {
    let g = EndOfThread(c);
    let st = g.0.clone();
    if let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_stack(fs, x, &mut *g.0))) {
        back_exception(&st, exception_from_panic(p));
    }
}

/// Runs the rest of the computation with `x` **in the current thread and in the same node**, without creating a thread or a node, and
/// returns (`runContLoop`). It is what `for_` does. If the node has no children, it throws the `Finish` backtracking.
fn run_cont_loop(ctx: &Ctx, x: Dyn) {
    let mut c = ctx.clone();
    let fs = std::mem::replace(&mut c.fcomp, Cons::nil());
    let st = c.clone();
    if let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_stack(&fs, x, &mut c))) {
        back_exception(&st, exception_from_panic(p));
    }
    if c.node.has_no_children() {
        let _ = back::<(), Finish>(Finish("loop thread ended".into())).run(&mut c);
    }
}

/// Takes the rest of the computation to another thread, giving it `x` as the value. It is the only place that creates threads.
/// With `threads 0` the rest runs right here; with `threads n` it waits for a free slot. If the branch is
/// cancelled nothing is born.
fn detach(ctx: &mut Ctx, x: Dyn) {
    if ctx.node.cancelled() {
        return;
    }
    if ctx.mode != Mode::Remote {
        ctx.mode = Mode::Parallel;
    }
    let mut c = ctx.clone();
    c.node = if ctx.free { TNode::detached() } else { TNode::child(&ctx.node, true) };
    let fs = std::mem::replace(&mut c.fcomp, Cons::nil());
    match &ctx.max_thread {
        Some(MaxThreads::Sync) => run_thread(&mut c, &fs, x),
        limit => {
            let slots = match limit {
                Some(MaxThreads::Limited(s)) => Some(s.clone()),
                _ => None,
            };
            if let Some(s) = &slots {
                s.acquire();
            }
            let slot = SlotGuard(slots);
            thread::spawn(move || {
                let _slot = slot;
                run_thread(&mut c, &fs, x);
            });
        }
    }
}

/// Runs the rest of the computation in a new thread; in the current one it looks like `empty`
pub fn abduce() -> Trans<()> {
    Trans::new(|ctx| {
        detach(ctx, Box::new(()));
        None
    })
}

/// Sets the maximum number of threads that can be created inside `t`. With 0, the tasks start in the current thread.
/// The limit applies only to `t`: the rest of the program, including what runs in the threads that `t` launches, goes back
/// to the previous limit (`threads` is `t <*** restore`).
pub fn threads<A: Send + 'static>(n: usize, t: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let old = std::mem::replace(
            &mut ctx.max_thread,
            Some(if n == 0 { MaxThreads::Sync } else { MaxThreads::Limited(Slots::new(n)) }),
        );
        after_each(t.clone(), Arc::new(move |ctx: &mut Ctx| ctx.max_thread = old.clone())).run(ctx)
    })
}

/// Makes sure there are at least `n` free thread slots in the current scope. Without a limit it does nothing (`addThreads`).
pub fn add_threads(n: usize) -> Trans<()> {
    Trans::new(move |ctx| {
        if let Some(MaxThreads::Limited(s)) = &ctx.max_thread {
            s.raise_to(n);
        }
        Some(())
    })
}

/// Adds `n` thread slots to the current scope. Without a limit it does nothing (`addThreads'`).
pub fn add_threads_plus(n: usize) -> Trans<()> {
    Trans::new(move |ctx| {
        if let Some(MaxThreads::Limited(s)) = &ctx.max_thread {
            s.add(n);
        }
        Some(())
    })
}

/// The threads born inside `t` are tracked again, also inside `free_threads` (`hookedThreads`)
pub fn hooked_threads<A: Send + 'static>(t: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let old = std::mem::replace(&mut ctx.free, false);
        let r = t.run(ctx);
        ctx.free = old;
        r
    })
}

/// Removes the thread limit inside `t`; the rest goes back to the previous limit (`anyThreads`)
pub fn any_threads<A: Send + 'static>(t: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let old = ctx.max_thread.take();
        after_each(t.clone(), Arc::new(move |ctx: &mut Ctx| ctx.max_thread = old.clone())).run(ctx)
    })
}

/// An IO action in a new thread: `abduce >> liftIO io`
pub fn async_<A: Send + 'static>(io: impl Fn() -> A + Send + Sync + 'static) -> Trans<A> {
    let io = Arc::new(io);
    tdo! {
        [io]
        abduce();
        lift_io(move || io())
    }
}

/// A result of a stream
#[derive(Clone, Debug, PartialEq)]
pub enum StreamData<A> {
    More(A),
    Last(A),
    Done,
}

/// Stream of results: `io` is called in a loop in a new thread; each `More` goes on in another thread, a `Last`
/// goes on in the thread of the loop and ends, and `Done` ends without a result.
pub fn parallel<A: Send + 'static>(io: impl Fn() -> StreamData<A> + Send + Sync + 'static) -> Trans<StreamData<A>> {
    let io = Arc::new(io);
    tdo! {
        [io]
        abduce();
        stream_loop(io)
    }
}

/// The loop of `parallel`: calls `io` until it gives the last result or `Done`
fn stream_loop<A: Send + 'static>(io: Arc<impl Fn() -> StreamData<A> + Send + Sync + 'static>) -> Trans<StreamData<A>> {
    Trans::new(move |ctx| loop {
        if ctx.node.cancelled() {
            return None;
        }
        match io() {
            StreamData::More(x) => detach(ctx, Box::new(StreamData::More(x))),
            last @ StreamData::Last(_) => return Some(last),
            StreamData::Done => return None,
        }
    })
}

/// An infinite stream of events: each value of `io` goes on in a different thread
pub fn wait_events<A: Send + 'static>(io: impl Fn() -> A + Send + Sync + 'static) -> Trans<A> {
    parallel(move || StreamData::More(io())).map(|s| match s {
        StreamData::More(x) | StreamData::Last(x) => x,
        StreamData::Done => unreachable!(),
    })
}

/// Runs `p` in a separate thread and goes on in the current one: `(abduce >> p >> empty) <|> return ()`
pub fn fork(p: Trans<()>) -> Trans<()> {
    let branch = tdo! {
        [p]
        abduce();
        p;
        empty()
    };
    or(branch, pure(()))
}

// ---------------------------------------------------------------------------------------------------------------
// Applicative

/// `f <$> a <*> b` (`apTransIO`). Both terms start at once; the result comes out when both are there. Each arrival is
/// paired with the last value of the other term, so a term that gives a stream produces a stream.
///
/// # The log of the applicative
///
/// Each `<*>` is a node with two sides, which can be other nodes. If the first term is asynchronous, the two terms run
/// in parallel in different threads. The thread that completes the node is the one that gets the last result, and its
/// log would have only its own side: a remote node (or a restart) that replayed it could not complete the applicative.
/// The two cells of the node, which hold the result of each term, hold also what the term added to the log:
///
/// * At the end of the first term, in whatever thread, `r1` gets its value and the segments that it added to the log.
/// * The second term starts with the log of the start of the node followed by a `Pending` hole that reads `r1`: the
///   log of the first term if it is known when the log is serialised, `w/` if it is not. The partial steps of the first
///   term that its thread left in the log are not part of the log of the second. At its end, `r2` gets its value and
///   its segments.
/// * The thread that sees both results completes the node with the log of the start followed by the segments of both
///   terms, in whichever order they arrived.
///
/// Hash: the first term counts as one step (`HASH_TERM`), the same in every thread and in the replay of a log with
/// `w/`, whatever the number of statements it has. At the end of the node the hash advances with `HASH_APP`, also in the
/// thread that gets no result from the node.
///
/// When the first term finishes in the same thread as the second (an applicative of synchronous terms) nothing is added
/// to the log.
pub fn lift2<A, B, C>(
    f: impl Fn(A, B) -> C + Send + Sync + 'static,
    a: Trans<A>,
    b: Trans<B>,
) -> Trans<C>
where
    A: Clone + Send + Sync + 'static,
    B: Clone + Send + Sync + 'static,
    C: Send + 'static,
{
    let f = Arc::new(f);
    Trans::new(move |ctx| {
        let isapp = ctx.is_applicative;
        ctx.is_applicative = true;

        // the log and the hash at the start of the node. Entering the node advances the hash, so the nodes nested in the
        // first term do not repeat the path of this one
        let seg0 = ctx.log.part_log.0.clone();
        let h0 = hash_step(ctx.log.hash, HASH_NODE);
        ctx.log.hash = h0;
        let n0 = seg0.len();
        let (h_term, h_end) = (hash_step(h0, HASH_TERM), hash_step(h0, HASH_APP));

        // value of each term, the segments that it added to the log, and the thread that got the first one
        type R1<A> = Arc<Mutex<Option<(A, Vec<LogSeg>, thread::ThreadId)>>>;
        type R2<B> = Arc<Mutex<Option<(B, Vec<LogSeg>)>>>;
        let r1: R1<A> = Arc::default();
        let r2: R2<B> = Arc::default();

        let added_since = move |ctx: &Ctx, n: usize| ctx.log.part_log.0[n..].to_vec();
        let set_log = move |ctx: &mut Ctx, segs: Vec<LogSeg>, h: i64| {
            ctx.log.part_log = LogB(segs);
            ctx.log.hash = h;
        };
        // the thread that sees both results completes the node
        let join = {
            let seg0 = seg0.clone();
            move |ctx: &mut Ctx, ld: Vec<LogSeg>, rd: Vec<LogSeg>| set_log(ctx, [seg0.clone(), ld, rd].concat(), h_end)
        };

        let fparallel = {
            let (f, r1, r2, join) = (f.clone(), r1.clone(), r2.clone(), join.clone());
            tdo! {
                [f, r1, r2, join]
                let x = a.clone();
                Trans::new(move |ctx| {
                    let ld = added_since(ctx, n0);
                    *r1.lock().unwrap() = Some((x.clone(), ld.clone(), thread::current().id()));
                    ctx.log.hash = h_term;
                    let second = r2.lock().unwrap().clone();
                    second.map(|(y, rd)| {
                        join(ctx, ld, rd);
                        f(x.clone(), y)
                    })
                })
            }
        };

        // the second term, when the first one is not resolved in this thread
        let run_r = {
            let (f, r1, r2, b, join, seg0) = (f.clone(), r1.clone(), r2.clone(), b.clone(), join.clone(), seg0.clone());
            Trans::new(move |ctx: &mut Ctx| {
                // the log of the second term: the start of the node followed by a hole for the first term. If the first
                // term is not resolved when the log is serialised, the hole is what the first term has left in this thread
                // (the tokens `w/` of the terms that are pending, so a replay skips them), without the values that the
                // statements of the term get later: the term may go on after them. When it is resolved, the hole is all
                // that the term added.
                let pending = added_since(ctx, n0);
                struct FirstTerm<A>(R1<A>, Vec<LogSeg>);
                impl<A: Send> Hole for FirstTerm<A> {
                    fn read(&self) -> LogB {
                        match &*self.0.lock().unwrap() {
                            Some((_, ld, _)) => LogB(ld.clone()),
                            None => LogB(self.1.clone()),
                        }
                    }
                    fn resolved(&self) -> bool {
                        self.0.lock().unwrap().is_some()
                    }
                }
                let hole = LogSeg::Pending(Arc::new(FirstTerm(r1.clone(), pending)));
                set_log(ctx, [seg0.clone(), vec![hole]].concat(), h_term);
                tdo! {
                    [f, r1, r2, join]
                    let y = b.clone();
                    Trans::new(move |ctx| {
                        let rd = added_since(ctx, n0 + 1);
                        *r2.lock().unwrap() = Some((y.clone(), rd.clone()));
                        let first = r1.lock().unwrap().clone();
                        first.map(|(x, ld, _)| {
                            join(ctx, ld, rd);
                            f(x, y.clone())
                        })
                    })
                }
                .run(ctx)
            })
        };

        // the second term, when the first one finished in this very thread: the segments of the first are already in the
        // log, the second goes on after them and only the hash is normalised
        let run_sync = {
            let (f, r1, r2, b) = (f.clone(), r1.clone(), r2.clone(), b.clone());
            move |ctx: &mut Ctx, ld_len: usize| {
                tdo! {
                    [f, r1, r2]
                    let y = b.clone();
                    Trans::new(move |ctx| {
                        let rd = added_since(ctx, n0 + ld_len);
                        *r2.lock().unwrap() = Some((y.clone(), rd));
                        let first = r1.lock().unwrap().clone();
                        first.map(|(x, _, _)| {
                            ctx.log.hash = h_end;
                            f(x, y.clone())
                        })
                    })
                }
                .run(ctx)
            }
        };

        let xparallel = {
            let (r1, run_r) = (r1.clone(), run_r.clone());
            Trans::new(move |ctx: &mut Ctx| {
                let first = r1.lock().unwrap().as_ref().map(|(_, ld, t)| (ld.len(), *t));
                match first {
                    // the first term may be being executed in parallel and will give its result later; in serial mode
                    // (parsers) the second term does not start if the first has given no result
                    None => if ctx.mode == Mode::Serial { None } else { run_r.run(ctx) },
                    Some((ld_len, t)) if t == thread::current().id() => run_sync(ctx, ld_len),
                    Some(_) => run_r.run(ctx),
                }
            })
        };

        let cleanup = Trans::new(move |ctx: &mut Ctx| {
            ctx.is_applicative = isapp;
            ctx.log.hash = h_end;
            None
        });
        let result = fparallel | xparallel | cleanup;
        // the flag goes back to what it was, in the thread that has the result
        let result = tdo! {
            let c = result.map(Taken::new);
            Trans::new(move |ctx| {
                ctx.is_applicative = isapp;
                c.take()
            })
        };
        let r = result.run(ctx);
        ctx.is_applicative = isapp;
        r
    })
}

impl<A: Add<Output = A> + Clone + Send + Sync + 'static> Add for Trans<A> {
    type Output = Trans<A>;
    fn add(self, b: Trans<A>) -> Trans<A> {
        lift2(|x, y| x + y, self, b)
    }
}

impl<A: Send + 'static> BitOr for Trans<A> {
    type Output = Trans<A>;
    fn bitor(self, y: Trans<A>) -> Trans<A> {
        or(self, y)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Execution

/// Runs `t` without a console and waits until no thread is alive; returns the results in order of arrival.
/// It is Haskell's `keep'` (Rust does not allow the apostrophe in names).
pub fn keep_all<A: Send + 'static>(t: Trans<A>) -> Vec<A> {
    let out = Arc::new(Mutex::new(Vec::new()));
    let o = out.clone();
    let mut ctx = Ctx::new();
    let (idle, wait_idle) = mpsc::channel();
    ctx.node.on_idle(move || idle.send(()).unwrap());
    let guarded = tdo! {
        [t]
        print_unhandled();
        t
    };
    tdo! {
        [o]
        let a = guarded.map(Taken::new);
        Trans::new(move |_| {
            o.lock().unwrap().push(a.take()?);
            None::<()>
        })
    }
    .run(&mut ctx);
    end_thread(&mut ctx);
    wait_idle.recv().unwrap();
    let r = std::mem::take(&mut *out.lock().unwrap());
    r
}

#[doc(hidden)]
#[macro_export]
macro_rules! __do {
    // The state: `[names cloned in every step]` and `[x]`, the value of the previous step when it was bound to a simple
    // name. `x` is owned by the call that is running, so the last step can use it without cloning: the value of the
    // previous step does not have to be `Clone` if nothing but the last step uses it.
    //
    // `let x = a;` with a simple name: `x` is cloned in the following steps (but the last one)
    (@go $b:path, [$($c:ident),* $(,)?] [$($f:ident)?] let $x:ident = $e:expr; $($rest:tt)+) => {
        $b(
            { $(let $c = $c.clone(); let _ = &$c;)* $(let $f = $f.clone(); let _ = &$f;)? $e },
            { $(let $c = $c.clone(); let _ = &$c;)* $(let $f = $f.clone(); let _ = &$f;)? move |$x| $crate::__do!(@go $b, [$($c,)* $($f,)?] [$x] $($rest)+) },
        )
    };
    (@go $b:path, [$($c:ident),* $(,)?] [$($f:ident)?] let $p:pat = $e:expr; $($rest:tt)+) => {
        $b(
            { $(let $c = $c.clone(); let _ = &$c;)* $(let $f = $f.clone(); let _ = &$f;)? $e },
            { $(let $c = $c.clone(); let _ = &$c;)* $(let $f = $f.clone(); let _ = &$f;)? move |$p| $crate::__do!(@go $b, [$($c,)* $($f,)?] [] $($rest)+) },
        )
    };
    (@go $b:path, [$($c:ident),* $(,)?] [$($f:ident)?] $e:expr; $($rest:tt)+) => {
        $b(
            { $(let $c = $c.clone(); let _ = &$c;)* $(let $f = $f.clone(); let _ = &$f;)? $e },
            { $(let $c = $c.clone(); let _ = &$c;)* $(let $f = $f.clone(); let _ = &$f;)? move |_| $crate::__do!(@go $b, [$($c,)* $($f,)?] [] $($rest)+) },
        )
    };
    // the last step: the names are cloned, the previous value (`f`) is moved
    (@go $b:path, [$($c:ident),* $(,)?] [$($f:ident)?] $e:expr) => { { $(let $c = $c.clone(); let _ = &$c;)* $e } };
    (@go $b:path, [$($c:ident),* $(,)?] [$($f:ident)?] $($rest:tt)*) => {
        compile_error!("tdo!/cdo!: every step ends in `;` (also an `if`/`match` in the middle) except the last one, which is the result of the block and has no `;`")
    };
}

/// `do` notation: `let x = a; b; c` is translated to `bind`.
///
/// ```
/// use transient_core::*;
///
/// let t = tdo! {
///     let a = pure(2);
///     let b = pure(3);
///     pure(a + b)
/// };
/// assert_eq!(t.run(&mut Ctx::new()), Some(5));
/// ```
///
/// * Every step ends in `;` (also an `if`/`match` in the middle); the last one is the result and has no `;`.
/// * `if`/`match` work as expressions, with a computation of the same type in each branch.
/// * Each step is an `Fn` closure that can run several times. Numbers and booleans (`Copy` types) are
///   used as they are. Anything else that comes from outside the block (`String`, `Vec`, an `Arc`...) is declared at the start,
///   in brackets, and the macro clones it in each step: `tdo! { [name, data] ... }`.
///
///   ```
///   use transient_core::*;
///
///   let greeting = String::from("hello");
///   let t = tdo! {
///       [greeting]
///       let name = pure("Ana".to_string());
///       pure(format!("{greeting} {name}"))
///   };
///   assert_eq!(t.run(&mut Ctx::new()), Some("hello Ana".to_string()));
///   ```
///
/// * What is bound inside the block with `let x = ...;` (a simple name) is cloned automatically in the following steps,
///   except in the last one, which receives it as it is: a value that is not `Clone` can be used there.
///
///   ```
///   use transient_core::*;
///
///   struct Token(u32); // not `Clone`
///   fn spend(t: Token) -> Trans<u32> {
///       pure(t.0 * 2)
///   }
///
///   let t = tdo! {
///       let token = lift_io(|| Token(21));
///       spend(token)           // the last step: `token` is moved, not cloned
///   };
///   assert_eq!(t.run(&mut Ctx::new()), Some(42));
///   ```
///
/// * With a pattern (`let (a, b) = ...;`) the names are not cloned, and if they are not `Copy` they have to be cloned by hand.
///   If it is forgotten, rustc says "cannot move out of `x`, a captured variable in an `Fn` closure": the solution is
///   to add `x` to the list at the start, not to clone by hand as the compiler suggests.
/// * A branch is an expression too: `if` and `match` can be the last step.
///
///   ```
///   use transient_core::*;
///
///   let t = tdo! {
///       let n = pure(4);
///       if n > 3 { pure("big") } else { pure("small") }
///   };
///   assert_eq!(t.run(&mut Ctx::new()), Some("big"));
///   ```
#[macro_export]
macro_rules! tdo {
    ([$($c:ident),*] $($rest:tt)+) => { $crate::__do!(@go $crate::bind, [$($c),*] [] $($rest)+) };
    ($($rest:tt)+) => { $crate::__do!(@go $crate::bind, [] [] $($rest)+) };
}

/// `tdo!` for `Cloud`: the same notation, but each step is a `Cloud` (only what goes through `local`/`on_all` is accepted),
/// so the compiler checks that every step is logged.
///
/// ```
/// use transient_core::*;
///
/// let program = cdo! {
///     let a = local_io(|| 1i64);
///     let b = local_io(|| 20i64);
///     cpure(a + b)
/// };
/// // a step that is not logged does not compile here: `lift_io(..)` is a `Trans`, not a `Cloud`
/// assert_eq!(program.run(&mut Ctx::new()), Some(21));
/// ```
#[macro_export]
macro_rules! cdo {
    ([$($c:ident),*] $($rest:tt)+) => { $crate::__do!(@go $crate::cbind, [$($c),*] [] $($rest)+) };
    ($($rest:tt)+) => { $crate::__do!(@go $crate::cbind, [] [] $($rest)+) };
}
