//! `react`: inversion of control reversed. An external `callback` becomes the point where the computation goes on.

use super::*;
use std::time::Duration;

/// Listeners that keep alive the node where they were registered, by identifier (`rcallbacks` in Haskell)
#[derive(Default)]
pub(crate) struct Callbacks(Mutex<Vec<(String, Arc<TNode>)>>);

/// Runs the rest of the computation registered in `cont`, with `event`, **in the current thread** and hanging as a child of
/// the node of `cont`. It is what each call to the callback of `react` does (`runContCleanup` after `hangFrom`).
pub(crate) fn run_callback<E: Send + 'static>(cont: &Ctx, event: E) {
    if cont.node.cancelled() {
        return;
    }
    let mut c = cont.clone();
    c.node = if cont.free { TNode::detached() } else { TNode::child(&cont.node, true) };
    let fs = std::mem::replace(&mut c.fcomp, Cons::nil());
    run_thread(&mut c, &fs, Box::new(event));
}

/// Turns a callback registration into a computation: `set_handler` receives the function `cb` that has to be called
/// with each event; each time it is called, the rest of the computation goes on with the value of the event and `cb`
/// returns what `response` gives. The rest runs **in the thread that calls `cb`**, hanging as a child of the thread
/// that called `react`.
///
/// As in `reactId`: `keep` makes the thread that called stay alive while the listener exists, so `collect`
/// and `keep_all` wait until it is removed with `del_listener`. `id` identifies the listener: a new one with the same
/// `id` replaces the previous one (and frees its thread).
pub fn react_id<E, R>(
    keep: bool,
    id: &str,
    set_handler: impl Fn(Arc<dyn Fn(E) -> R + Send + Sync>) + Send + Sync + 'static,
    response: impl Fn() -> R + Send + Sync + 'static,
) -> Trans<E>
where
    E: Send + 'static,
    R: 'static,
{
    let (id, set_handler, response) = (id.to_string(), Arc::new(set_handler), Arc::new(response));
    Trans::new(move |ctx| {
        if ctx.mode != Mode::Remote {
            ctx.mode = Mode::Parallel;
        }
        if keep {
            let previous = {
                let mut cbs = ctx.callbacks.0.lock().unwrap();
                let (previous, mut kept): (Vec<_>, Vec<_>) = cbs.drain(..).partition(|(i, _)| *i == id);
                ctx.node.add_listener();
                kept.push((id.clone(), ctx.node.clone()));
                *cbs = kept;
                previous
            };
            // outside the lock: the node can become quiet and run hooks
            previous.iter().for_each(|(_, n)| n.remove_listener());
        }
        let (cont, response) = (ctx.clone(), response.clone());
        set_handler(Arc::new(move |event: E| {
            run_callback(&cont, event);
            response()
        }));
        None
    })
}

/// `react` with an anonymous identifier of its own (`react`). It keeps the thread tree alive while the listener exists;
/// an anonymous listener can not be removed with `del_listener`, so a `collect` or a `keep_all` that contain it
/// do not end by themselves: use `end_collect`, `exit`, or `react_id` with an identifier.
pub fn react<E, R>(
    set_handler: impl Fn(Arc<dyn Fn(E) -> R + Send + Sync>) + Send + Sync + 'static,
    response: impl Fn() -> R + Send + Sync + 'static,
) -> Trans<E>
where
    E: Send + 'static,
    R: 'static,
{
    let (set_handler, response) = (Arc::new(set_handler), Arc::new(response));
    Trans::new(move |ctx| {
        let (h, r) = (set_handler.clone(), response.clone());
        react_id(true, &format!("react {}", gen_new_id()), move |cb| h(cb), move || r()).run(ctx)
    })
}

/// `react` whose response does not matter (`react1`)
pub fn react1<E: Send + 'static>(set_handler: impl Fn(Arc<dyn Fn(E) + Send + Sync>) + Send + Sync + 'static) -> Trans<E> {
    react(set_handler, || ())
}

/// `react` that does not keep the tree alive: a `collect` or a `keep_all` can end before any
/// event arrives (Haskell's `react`, which is `reactId False`)
pub fn react_nokeep<E, R>(
    set_handler: impl Fn(Arc<dyn Fn(E) -> R + Send + Sync>) + Send + Sync + 'static,
    response: impl Fn() -> R + Send + Sync + 'static,
) -> Trans<E>
where
    E: Send + 'static,
    R: 'static,
{
    react_id(false, "", set_handler, response)
}

/// Removes the listener `id` (`delListener`): its node stops waiting and the tree can become quiet
pub fn del_listener(id: &str) -> Trans<()> {
    let id = id.to_string();
    Trans::new(move |ctx| {
        let removed: Vec<_> = {
            let mut cbs = ctx.callbacks.0.lock().unwrap();
            let (removed, kept): (Vec<_>, Vec<_>) = cbs.drain(..).partition(|(i, _)| *i == id);
            *cbs = kept;
            removed
        };
        removed.iter().for_each(|(_, n)| n.remove_listener());
        Some(())
    })
}

/// The threads of `wait_events` do not hang from the tree (`spawn`): they are not waited for or cancelled
pub fn spawn<A: Send + 'static>(io: impl Fn() -> A + Send + Sync + 'static) -> Trans<A> {
    free_threads(wait_events(io))
}

/// A stream with the value of `action` each time it changes, checked every `interval`. The first one arrives at once.
pub fn sample<A>(action: impl Fn() -> A + Send + Sync + 'static, interval: Duration) -> Trans<A>
where
    A: Clone + PartialEq + Send + Sync + 'static,
{
    let action = Arc::new(action);
    Trans::new(move |ctx| {
        let first = action();
        let prev = Arc::new(Mutex::new(first.clone()));
        let action = action.clone();
        let changes = wait_events(move || loop {
            thread::sleep(interval);
            let v = action();
            let mut p = prev.lock().unwrap();
            if *p != v {
                *p = v.clone();
                return v;
            }
        });
        or(changes, pure(first)).run(ctx)
    })
}
