//! Backtracking (`Backtrack`): stacks of handlers by type of reason, from which undo/retry come out
//! (`on_undo`, `undo`, `retry`) and finalization (`on_finish`).
//!
//! `on_back` registers, in the data state, a handler together with the continuation of the point where it was registered.
//! `back(reason)` runs the handlers in the reverse order of registration. A handler that calls `forward` makes
//! the execution resume forwards **from the point where it was registered** (with the value that the handler gave);
//! if it ends without `forward`, the backtracking goes on with the previous handler; if it stops (`empty`), it ends.
//! The stacks travel in the state, so the threads that are born after registering them inherit them.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

type Handler<B> = Arc<dyn Fn(B) -> Trans<Dyn> + Send + Sync>;

struct BackStack<B> {
    /// the reason being backtracked; `None` if there is no backtracking in progress or `forward` was done
    backtracking: Option<B>,
    /// handlers, the last registered first, each with the continuation of its registration point
    stack: Cons<(Handler<B>, Cons<Cont>)>,
}

impl<B: Clone> Clone for BackStack<B> {
    fn clone(&self) -> Self {
        BackStack { backtracking: self.backtracking.clone(), stack: self.stack.clone() }
    }
}

fn stack_of<B: Clone + Send + Sync + 'static>(ctx: &Ctx) -> BackStack<B> {
    ctx.get::<BackStack<B>>().map_or(BackStack { backtracking: None, stack: Cons::nil() }, |b| (*b).clone())
}

/// Runs `ac` and registers `handler` for when `back` is done with a reason of type `B`. The handler gives a
/// value of the type of `ac`, with which the computation goes on if it does `forward`.
pub fn on_back<A, B>(ac: Trans<A>, handler: impl Fn(B) -> Trans<A> + Send + Sync + 'static) -> Trans<A>
where
    A: Send + 'static,
    B: Clone + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    let erased: Handler<B> = Arc::new(move |b| {
        let t = handler(b);
        Trans::new(move |ctx| t.run(ctx).map(|a| Box::new(a) as Dyn))
    });
    Trans::new(move |ctx| {
        let mut bs = stack_of::<B>(ctx);
        bs.stack = bs.stack.push((erased.clone(), ctx.fcomp.clone()));
        ctx.set(bs);
        register_merger::<B>(ctx);
        ac.run(ctx)
    })
}

/// How to merge two states of the same type (`combine` of `collectc`)
pub(crate) type Merger = fn(&Arc<dyn Any + Send + Sync>, &Arc<dyn Any + Send + Sync>) -> Arc<dyn Any + Send + Sync>;

/// The types of the data state that are merged instead of replaced when the states of the branches of a `collectc` are
/// joined: the stacks of handlers (`BackStack<B>`), which register themselves here
#[derive(Clone, Default)]
pub(crate) struct Mergers(pub HashMap<TypeId, Merger>);

fn register_merger<B: Clone + Send + Sync + 'static>(ctx: &mut Ctx) {
    let id = TypeId::of::<BackStack<B>>();
    let mut mergers = ctx.get::<Mergers>().map_or_else(Mergers::default, |m| (*m).clone());
    if !mergers.0.contains_key(&id) {
        mergers.0.insert(id, merge_stacks::<B>);
        ctx.set(mergers);
    }
}

/// The handlers of both stacks, those of `y` first, and no backtracking in progress
fn merge_stacks<B: Clone + Send + Sync + 'static>(x: &Arc<dyn Any + Send + Sync>, y: &Arc<dyn Any + Send + Sync>) -> Arc<dyn Any + Send + Sync> {
    let (x, y) = (x.downcast_ref::<BackStack<B>>().unwrap(), y.downcast_ref::<BackStack<B>>().unwrap());
    let mut handlers = Vec::new();
    let mut cur = y.stack.clone();
    while let Some((h, rest)) = cur.uncons().map(|(h, r)| (h.clone(), r.clone())) {
        handlers.push(h);
        cur = rest;
    }
    let stack = handlers.into_iter().rev().fold(x.stack.clone(), |st, h| st.push(h));
    Arc::new(BackStack { backtracking: None, stack })
}

/// `on_back` without a computation: it only registers the handler (`onBack1`)
pub fn on_back1<B>(handler: impl Fn(B) -> Trans<()> + Send + Sync + 'static) -> Trans<()>
where
    B: Clone + Send + Sync + 'static,
{
    on_back(pure(()), handler)
}

/// Starts the backtracking of the reason `reason`: runs the handlers registered for the type `B`, the last
/// first. The computation that calls it never goes on: if a handler does `forward`, the one of the point
/// where it was registered goes on.
pub fn back<A, B>(reason: B) -> Trans<A>
where
    A: Send + 'static,
    B: Clone + Send + Sync + 'static,
{
    Trans::new(move |ctx| {
        let mut stack = stack_of::<B>(ctx).stack;
        while let Some(((handler, k), rest)) = stack.uncons().map(|((h, k), r)| ((h.clone(), k.clone()), r.clone())) {
            ctx.set(BackStack { backtracking: Some(reason.clone()), stack: rest.clone() });
            // a handler that stops ends the backtracking
            let x = handler(reason.clone()).run(ctx)?;
            if stack_of::<B>(ctx).backtracking.is_none() {
                // `forward`: the complete stack is recovered and execution resumes from the registration point
                ctx.set(BackStack { backtracking: None, stack: stack.clone() });
                run_stack(&k, x, ctx);
                return None;
            }
            stack = rest;
        }
        None
    })
}

/// Inside a handler: stops the backtracking of the reasons of type `B`; the execution goes on forwards
/// when the handler ends. The type must be given: `forward::<Finish>()`.
pub fn forward<B: Clone + Send + Sync + 'static>() -> Trans<()> {
    Trans::new(|ctx| {
        let mut bs = stack_of::<B>(ctx);
        bs.backtracking = None;
        ctx.set(bs);
        Some(())
    })
}

/// Deletes the handlers registered so far for the reasons of type `B`
pub fn back_cut<B: Clone + Send + Sync + 'static>() -> Trans<()> {
    Trans::new(|ctx| {
        ctx.del::<BackStack<B>>();
        Some(())
    })
}

/// The handlers for reasons of type `B` that are registered inside `t` disappear when leaving `t`. Inside
/// `t` those already registered outside are also seen: they fire after those inside (`localBack`, which
/// is `sandboxData`: it only restores the previous state on exit, it does not empty it on entry). The restoration is done
/// after each result of `t` and also as soon as `t` returns control, even if it goes on in another thread.
pub fn local_back<A, B>(t: Trans<A>) -> Trans<A>
where
    A: Send + 'static,
    B: Clone + Send + Sync + 'static,
{
    sandbox_data::<BackStack<B>, A>(t)
}

// ---- the default track: undo and retry

/// `on_back` for the reason `()`: `y` runs when `undo` is done
pub fn on_undo<A: Send + 'static>(x: Trans<A>, y: Trans<A>) -> Trans<A> {
    on_back(x, move |()| y.clone())
}

/// `back(())`
pub fn undo<A: Send + 'static>() -> Trans<A> {
    back(())
}

/// `forward` for the reason `()`: inside an `on_undo` handler, it resumes forwards
pub fn retry() -> Trans<()> {
    forward::<()>()
}

/// `back_cut` of the reason `()`
pub fn undo_cut() -> Trans<()> {
    back_cut::<()>()
}

// ---- finalization

/// The reason of the end backtracking: each thread throws it when it ends if it has no children
#[derive(Clone, Debug)]
pub struct Finish(pub String);

/// Registers `f` for when no thread is alive under this point (not even waiting for events): it runs once,
/// in the last thread that ends. Nested finalizers run from the last registered to the first.
/// The computation goes on in a new thread (like `abduce`), which is the one that hangs from this point.
pub fn on_finish(f: impl Fn(&str) -> Trans<()> + Send + Sync + 'static) -> Trans<()> {
    let f = Arc::new(f);
    tdo! {
        [f]
        any_threads(abduce());
        register_finish(f)
    }
}

/// Registers the handler of the end of the threads under this point, to run once
fn register_finish(f: Arc<impl Fn(&str) -> Trans<()> + Send + Sync + 'static>) -> Trans<()> {
    Trans::new(move |ctx| {
        let (master, done, f) = (ctx.node.clone(), Arc::new(AtomicBool::new(false)), f.clone());
        let register = on_back1(move |Finish(reason)| {
            let (master, done, f) = (master.clone(), done.clone(), f.clone());
            Trans::new(move |ctx| {
                // without `forward` the backtracking goes on with the previous handler
                if !master.subtree_quiet() || done.swap(true, Ordering::SeqCst) {
                    return Some(());
                }
                f(&reason).run(ctx);
                Some(())
            })
        });
        register.run(ctx)
    })
}

/// Starts the end backtracking (`finish`). It is not normally needed: each thread does it when it ends.
pub fn finish(reason: &str) -> Trans<()> {
    back(Finish(reason.into()))
}

/// Registers `f` for when the threads that hang from this point are **all stopped** (none is running,
/// even if there are listeners waiting for events). It runs once (`onWaitThreads`). Unlike `on_finish`, it does not
/// open a new thread or wait for the listeners to be removed.
///
/// (Haskell's did not do this: its `noactiveth` check always gave `True` and it ran when the first thread ended. It was
/// fixed in the same session; see `transient/tests/original-vs-rust/README.md`.)
pub fn on_wait_threads(f: impl Fn(&str) -> Trans<()> + Send + Sync + 'static) -> Trans<()> {
    let f = Arc::new(f);
    Trans::new(move |ctx| {
        ctx.free = false;
        let (master, done, f) = (ctx.node.clone(), Arc::new(AtomicBool::new(false)), f.clone());
        on_back1(move |Finish(reason)| {
            let (master, done, f) = (master.clone(), done.clone(), f.clone());
            Trans::new(move |ctx| {
                if !master.subtree_stopped() || done.swap(true, Ordering::SeqCst) {
                    return Some(());
                }
                f(&reason).run(ctx);
                Some(())
            })
        })
        .run(ctx)
    })
}
