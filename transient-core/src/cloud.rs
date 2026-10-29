//! `Cloud` (`Transient.Move.Internals`): a `Trans` that only accepts what goes through `local` (which logs) or
//! `on_all` (which does not). It is Haskell's `newtype Cloud`: it costs nothing at run time and the compiler rejects
//! an unlogged step in a distributed program. It is written with `cdo!`, like `tdo!`.

use super::*;

pub struct Cloud<A>(Trans<A>);

impl<A> Clone for Cloud<A> {
    fn clone(&self) -> Self {
        Cloud(self.0.clone())
    }
}

impl<A: Send + 'static> Cloud<A> {
    /// The `Trans` inside (`unCloud`)
    pub fn un_cloud(self) -> Trans<A> {
        self.0
    }
    pub fn run(&self, ctx: &mut Ctx) -> Option<A> {
        self.0.run(ctx)
    }
    /// Stops after this: the result is discarded and the alternative runs (`>> empty`)
    pub fn then_empty<B: Send + 'static>(self) -> Cloud<B> {
        Cloud(tdo! {
            self.0;
            empty::<B>()
        })
    }
    /// `fmap`
    pub fn map<B: Send + 'static>(self, f: impl Fn(A) -> B + Send + Sync + 'static) -> Cloud<B> {
        Cloud(self.0.map(f))
    }
}

/// A logged step: its result goes to the log and a replay reads it from there (`local = Cloud . logged`)
pub fn local<A: Loggable + Send + Sync + 'static>(t: Trans<A>) -> Cloud<A> {
    Cloud(logged(t))
}

/// Like `local`, for an IO action (`localIO`)
pub fn local_io<A: Loggable + Send + Sync + 'static>(f: impl Fn() -> A + Send + Sync + 'static) -> Cloud<A> {
    local(lift_io(f))
}

/// An unlogged step, which is also executed in a replay (`onAll`)
pub fn on_all<A: Send + 'static>(t: Trans<A>) -> Cloud<A> {
    Cloud(t)
}

/// `on_all`, but the computation is forced to pass through the statement that you want to re-execute: a normal function
/// is replaced by its result in a replay and its body is not run again, and with this one an effect (a `set_data` that
/// has to stay set once the function returned) is applied again (`onAllDeep`, with `preservePath`)
pub fn on_all_deep<A: Send + 'static>(t: Trans<A>) -> Cloud<A> {
    Cloud(tdo! {
        [t]
        preserve_path_on();
        t
    })
}

/// The path of the closures is kept in the log (`preservePath`)
fn preserve_path_on() -> Trans<()> {
    Trans::new(|ctx| {
        if let Some(p) = ctx.get::<PrevClos>() {
            ctx.set(PrevClos { preserve_path: true, ..(*p).clone() });
        }
        Some(())
    })
}

pub fn cpure<A: Clone + Send + Sync + 'static>(a: A) -> Cloud<A> {
    Cloud(pure(a))
}

pub fn cempty<A: Send + 'static>() -> Cloud<A> {
    Cloud(empty())
}

pub fn cbind<A: Send + 'static, B: Send + 'static>(x: Cloud<A>, f: impl Fn(A) -> Cloud<B> + Send + Sync + 'static) -> Cloud<B> {
    Cloud(bind(x.0, move |a| f(a).0))
}

/// `<|>`
pub fn cor<A: Send + 'static>(x: Cloud<A>, y: Cloud<A>) -> Cloud<A> {
    Cloud(or(x.0, y.0))
}

/// `x | y` is `cor(x, y)`: alternatives read in a row, as `<|>` does in Haskell
impl<A: Send + 'static> BitOr for Cloud<A> {
    type Output = Cloud<A>;
    fn bitor(self, y: Cloud<A>) -> Cloud<A> {
        cor(self, y)
    }
}

/// The result of the computation goes to the log and replaces everything that was logged inside (`loggedc`)
pub fn loggedc<A: Loggable + Send + Sync + 'static>(c: Cloud<A>) -> Cloud<A> {
    loggedc_with(false, c)
}

pub fn loggedc_with<A: Loggable + Send + Sync + 'static>(compact: bool, c: Cloud<A>) -> Cloud<A> {
    Cloud(logged_with(compact, c.0))
}

/// The applicative of `Cloud` (`f <$> a <*> b`): both terms start at once, and their logs are put together so that a node
/// that replays them can complete the applicative (see `lift2`)
pub fn clift2<A, B, C>(f: impl Fn(A, B) -> C + Send + Sync + 'static, a: Cloud<A>, b: Cloud<B>) -> Cloud<C>
where
    A: Clone + Send + Sync + 'static,
    B: Clone + Send + Sync + 'static,
    C: Send + 'static,
{
    Cloud(lift2(f, a.0, b.0))
}

/// `(,) <$> a <*> b`
pub fn cpair<A, B>(a: Cloud<A>, b: Cloud<B>) -> Cloud<(A, B)>
where
    A: Clone + Send + Sync + 'static,
    B: Clone + Send + Sync + 'static,
{
    clift2(|x, y| (x, y), a, b)
}
