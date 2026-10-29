//! Exceptions: they are backtracking (`back`) with the reason `SomeException`, as in the original.
//!
//! * `on_exception` registers a handler filtered by type. It stays registered: it is fired by **any thread** among those
//!   born later that throws an exception of that type, as many times as it happens.
//! * The handlers run from the last registered to the first. If one ends with nothing else, the backtracking goes on with the
//!   previous one; `continue_` stops it and the execution resumes forwards from the registration point; if it
//!   stops (`empty`), it ends. `keep` and `keep_all` put the first of all, which prints the exception.
//! * Each registration point wraps in a `catch` **everything that comes after it**, and each thread catches at its
//!   top what is left. An exception thrown inside the first argument of `on_exception_with`, before it is
//!   complete, is not caught by that point (as in the original). `catcht` does catch it: it is a fix.
//! * `throwt` throws through the backtracking track and **keeps the state**; `throw` is a `panic` with an exception
//!   (without the message of the panic hook) and the handler sees the state that existed at the registration point.
//! * A normal `panic!` is caught as `Panic(message)`.

use super::*;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

/// An exception of any type, so that `SomeException` in a handler catches everything
#[derive(Clone)]
pub struct SomeException {
    value: Arc<dyn Any + Send + Sync>,
    text: Arc<str>,
}

impl SomeException {
    pub fn new<E: Any + Send + Sync + std::fmt::Debug>(e: E) -> Self {
        SomeException { text: format!("{e:?}").into(), value: Arc::new(e) }
    }

    pub fn downcast_ref<E: Any>(&self) -> Option<&E> {
        self.value.downcast_ref::<E>()
    }

    /// A copy of the exception as `E`, if it is one; `SomeException` catches everything (`fromException`)
    pub fn cast<E: Any + Clone>(&self) -> Option<E> {
        (self as &dyn Any).downcast_ref::<E>().cloned().or_else(|| self.downcast_ref::<E>().cloned())
    }
}

impl std::fmt::Display for SomeException {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::fmt::Debug for SomeException {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// A caught `panic!`
#[derive(Clone, Debug)]
pub struct Panic(pub String);

pub(crate) fn exception_from_panic(p: Box<dyn Any + Send>) -> SomeException {
    match p.downcast::<SomeException>() {
        Ok(e) => *e,
        Err(p) => {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic".into());
            SomeException::new(Panic(msg))
        }
    }
}

/// Starts the exception backtracking on a copy of `st`. If a handler throws another exception, it restarts with
/// the new one, always from `st` (`backExceptIO`).
pub(crate) fn back_exception(st: &Ctx, mut e: SomeException) {
    loop {
        let mut c = st.clone();
        match catch_unwind(AssertUnwindSafe(|| back::<(), SomeException>(e.clone()).run(&mut c))) {
            Ok(_) => return,
            Err(p) => e = exception_from_panic(p),
        }
    }
}

/// Runs `mx` and, if it gives a value, **the rest of the computation inside a `catch`**. If the rest throws, the
/// exception backtracking starts with the state that existed at this point.
fn guard_rest<A: Send + 'static>(mx: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let r = mx.run(ctx)?;
        let st = ctx.clone();
        let fs = ctx.fcomp.clone();
        if let Err(p) = catch_unwind(AssertUnwindSafe(|| run_stack(&fs, Box::new(r), ctx))) {
            back_exception(&st, exception_from_panic(p));
        }
        None
    })
}

/// Runs `mx` inside a `catch`: what `mx` throws in this thread starts the backtracking with the state from here, which already
/// includes the handler registered just before. It is what `catcht` adds to the original's `onException'`, which only
/// guards what comes after `mx`.
fn guard_first<A: Send + 'static>(mx: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let st = ctx.clone();
        match catch_unwind(AssertUnwindSafe(|| mx.run(ctx))) {
            Ok(r) => r,
            Err(p) => {
                back_exception(&st, exception_from_panic(p));
                None
            }
        }
    })
}

/// Registers a handler for any exception for `mx` and everything that comes after (`onAnyException`)
pub fn on_any_exception<A: Send + 'static>(
    mx: Trans<A>,
    handler: impl Fn(SomeException) -> Trans<A> + Send + Sync + 'static,
) -> Trans<A> {
    on_back(guard_rest(mx), handler)
}

/// Like `on_any_exception`, but only for exceptions of type `E`; the others go on with the previous
/// handler (`onException'`). The handler gives the value with which the computation goes on if it does `continue_`.
pub fn on_exception_with<A, E>(mx: Trans<A>, handler: impl Fn(E) -> Trans<A> + Send + Sync + 'static) -> Trans<A>
where
    A: Send + 'static,
    E: Clone + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    on_any_exception(mx, move |e| match e.cast::<E>() {
        None => back(e),
        Some(e) => handler(e),
    })
}

/// Registers a handler for the exceptions of type `E` that happen in the rest of the computation, in this thread
/// or in those born from it (`onException`). `E = SomeException` catches everything.
pub fn on_exception<E>(handler: impl Fn(E) -> Trans<()> + Send + Sync + 'static) -> Trans<()>
where
    E: Clone + Send + Sync + 'static,
{
    on_exception_with(pure(()), handler)
}

/// Inside a handler: stops the backtracking and resumes forwards from the registration point (`continue`)
pub fn continue_() -> Trans<()> {
    forward::<SomeException>()
}

/// Deletes the exception handlers registered so far (`cutExceptions`)
pub fn cut_exceptions() -> Trans<()> {
    back_cut::<SomeException>()
}

/// The handlers registered inside `w` are not valid outside it (`localExceptions`)
pub fn local_exceptions<A: Send + 'static>(w: Trans<A>) -> Trans<A> {
    tdo! {
        let r = local_back::<A, SomeException>(w).map(Taken::new);
        on_exception(|_: SomeException| pure(()));
        Trans::new(move |_| r.take())
    }
}

/// Transient's `try`/`catch`: the handler resumes the computation with its value (`catcht`). It catches what `mx` throws
/// with `throw`, with `throwt` or with a `panic!`. (Haskell's `catcht` only catches `throwt` inside `mx`: it is a
/// bug of that version and here it is fixed.)
pub fn catcht<A, E>(mx: Trans<A>, handler: impl Fn(E) -> Trans<A> + Send + Sync + 'static) -> Trans<A>
where
    A: Send + 'static,
    E: Clone + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    local_exceptions(on_exception_with(guard_first(mx), move |e: E| {
        let handler = handler.clone();
        tdo! {
            [handler, e]
            continue_();
            handler(e)
        }
    }))
}

/// Repeats `mx` each time it throws an exception of type `E`, calling `fix` first (`whileException`)
pub fn while_exception<A, E>(mx: Trans<A>, fix: impl Fn(E) -> Trans<()> + Send + Sync + 'static) -> Trans<A>
where
    A: Send + 'static,
    E: Clone + Send + Sync + 'static,
{
    while_exception_dyn(mx, Arc::new(fix))
}

/// The recursion goes through `dyn` so that the type of the closure does not change on each turn
fn while_exception_dyn<A, E>(mx: Trans<A>, fix: Arc<dyn Fn(E) -> Trans<()> + Send + Sync>) -> Trans<A>
where
    A: Send + 'static,
    E: Clone + Send + Sync + 'static,
{
    let again = mx.clone();
    catcht(mx, move |e: E| {
        let (fix, again) = (fix.clone(), again.clone());
        tdo! {
            [again, fix]
            fix(e);
            while_exception_dyn(again, fix)
        }
    })
}

/// Throws an exception through the backtracking track keeping the state (`throwt`)
pub fn throwt<A: Send + 'static, E: Any + Send + Sync + std::fmt::Debug + Clone>(e: E) -> Trans<A> {
    back(SomeException::new(e))
}

/// Throws an exception as a `panic`, without going through the panic hook. It does not keep the state: the handler sees the
/// one that existed at the point where it was registered (`throw`).
pub fn throw<E: Any + Send + Sync + std::fmt::Debug>(e: E) -> ! {
    resume_unwind(Box::new(SomeException::new(e)))
}

/// The first handler of `keep` and `keep_all`: prints what nobody has stopped
pub(crate) fn print_unhandled() -> Trans<()> {
    on_exception(|e: SomeException| {
        lift_io(move || {
            println!("{:?}: {e}", thread::current().id());
        })
    })
}
