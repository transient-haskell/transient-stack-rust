//! The operators of `AdditionalOperators` (`**>`, `<**`, `<***`, `<->`), the isolation of the state (`try`, `sandbox`)
//! and some utilities of `Transient.Base`. Each one has been checked by running the original.
//!
//! Rust does not allow creating new operators, so each one is a function:
//!
//! | Haskell | Rust |
//! |---|---|
//! | `x **> y` | `seq_right(x, y)` |
//! | `x <*** y` (`atEnd`) | `at_end(x, y)` |
//! | `x <** y` (`atEnd'`) | `at_end_again(x, y)` |
//! | `x <-> y` | `both(x, y)` |

use super::*;

/// Runs `x` discarding its result and then `y`, **also if `x` gives no result**: it is not the monad's `>>` (`**>`)
pub fn seq_right<A: Send + 'static, B: Send + 'static>(x: Trans<A>, y: Trans<B>) -> Trans<B> {
    Trans::new(move |ctx| {
        x.run(ctx);
        y.run(ctx)
    })
}

/// Runs `ma` and `mb` (discarding the result of `mb`); `mb` also runs **after each thread of `ma`
/// and before what follows**. The result is that of `ma` (`<***`, `atEnd`).
pub fn at_end<A, B>(ma: Trans<A>, mb: Trans<B>) -> Trans<A>
where
    A: Clone + Send + Sync + 'static,
    B: Send + 'static,
{
    Trans::new(move |ctx| {
        let fs = ctx.fcomp.clone();
        let mb2 = mb.clone();
        // `\x -> mb >> return x`
        let k = erase(Arc::new(move |x: A| {
            tdo! {
                [x]
                mb2.clone();
                pure(x)
            }
        }));
        ctx.fcomp = fs.push(k);
        let a = ma.run(ctx);
        mb.run(ctx);
        ctx.fcomp = fs;
        a
    })
}

/// Runs `ma` and then `mb` once. Each event of `mb` (a `react`, a `get_mailbox`...) runs `ma` **again**,
/// with the same sequence of identifiers (`gen_id`), and the computation that follows goes on with the new result of
/// `ma`. The value of the event and the result of `mb` are not passed. Without events in `mb`, `ma` runs only once.
/// It is for forms: `ma` reads the fields and `mb` is the button that sends them (`<**`, `atEnd'`).
pub fn at_end_again<A, B>(ma: Trans<A>, mb: Trans<B>) -> Trans<A>
where
    A: Send + 'static,
    B: Send + 'static,
{
    Trans::new(move |ctx| {
        let seq0 = ctx.seq;
        let a = ma.run(ctx);
        let fs = ctx.fcomp.clone();
        let ma2 = ma.clone();
        let again = Trans::new(move |ctx: &mut Ctx| {
            let seq1 = std::mem::replace(&mut ctx.seq, seq0);
            let r = ma2.run(ctx);
            ctx.seq = seq1;
            r
        });
        ctx.fcomp = fs.push(erase(Arc::new(move |_: B| again.clone())));
        mb.run(ctx);
        ctx.fcomp = fs;
        a
    })
}

/// Runs both and **goes on with the result of each one in this thread**, unlike `|`, which only runs the
/// second if the first gives no result (`<->`).
pub fn both<A: Send + 'static>(x: Trans<A>, y: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        if let Some(v) = x.run(ctx) {
            run_cont_loop(ctx, Box::new(v));
        }
        if let Some(v) = y.run(ctx) {
            run_cont_loop(ctx, Box::new(v));
        }
        None
    })
}

/// Runs `f` after each result of `t`, before what follows, and once more in the calling thread as soon as
/// `t` returns control (even if it gives no result because it goes on in another thread): it is the scheme of `<***` without the
/// value having to be cloned
pub(crate) fn after_each<A: Send + 'static>(t: Trans<A>, f: Arc<dyn Fn(&mut Ctx) + Send + Sync>) -> Trans<A> {
    Trans::new(move |ctx| {
        let fs = ctx.fcomp.clone();
        let f2 = f.clone();
        let k: Cont = Arc::new(move |d: Dyn| {
            let (f, d) = (f2.clone(), Mutex::new(Some(d)));
            Trans::new(move |ctx| {
                f(ctx);
                d.lock().unwrap().take()
            })
        });
        ctx.fcomp = fs.push(k);
        let a = t.run(ctx);
        f(ctx);
        ctx.fcomp = fs;
        a
    })
}

/// Runs `mx` and restores the whole data state and the parse position at the end, whether it succeeds or not (`sandbox'`). The changes made by
/// `mx` are not seen by what follows or by the alternatives.
pub fn sandbox<A: Send + 'static>(mx: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let (saved, parse) = (ctx.data.clone(), ctx.parse.clone());
        after_each(
            mx.clone(),
            Arc::new(move |ctx: &mut Ctx| {
                ctx.data = saved.clone();
                ctx.parse = parse.clone();
            }),
        )
        .run(ctx)
    })
}

/// Runs `mx` and restores only the data of type `T` (`sandboxData`)
pub fn sandbox_data<T: Any + Send + Sync, A: Send + 'static>(mx: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let saved = ctx.get::<T>();
        let restore = Arc::new(move |ctx: &mut Ctx| match &saved {
            Some(v) => Arc::make_mut(&mut ctx.data).insert(TypeId::of::<T>(), v.clone() as Arc<dyn Any + Send + Sync>).map(|_| ()).unwrap_or(()),
            None => ctx.del::<T>(),
        });
        after_each(mx.clone(), restore).run(ctx)
    })
}

/// If `mx` gives no result, it restores the data state and the parse position it had before, so that the alternatives see it intact
/// (`try`). If it gives a result, it keeps the changes.
pub fn try_<A: Send + 'static>(mx: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let (saved, parse) = (ctx.data.clone(), ctx.parse.clone());
        let r = mx.run(ctx);
        if r.is_none() {
            ctx.data = saved;
            ctx.parse = parse;
        }
        r
    })
}

/// Stops without a result: runs the alternative if there is one (`stop`)
pub fn stop<A: Send + 'static>() -> Trans<A> {
    empty()
}

/// A new identifier of the sequence of the state, which is copied when forking (`genId`)
pub fn gen_id() -> Trans<usize> {
    Trans::new(|ctx| {
        let n = ctx.seq;
        ctx.seq += 1;
        Some(n)
    })
}

/// Adds a label to the current thread, which appears in the thread tree (`ps`) (`labelState`)
pub fn label_state(label: &str) -> Trans<()> {
    let label = label.to_string();
    Trans::new(move |ctx| {
        ctx.node.add_label(&label);
        Some(())
    })
}

/// Opens a resource and closes it when no thread is left that could use it (`openClose`)
pub fn open_close<A: Clone + Send + Sync + 'static>(
    open: Trans<A>,
    close: impl Fn(A) -> Trans<()> + Send + Sync + 'static,
) -> Trans<A> {
    let close = Arc::new(close);
    tdo! {
        [close]
        let res = open;
        on_finish(move |_| close(res.clone()));
        pure(res)
    }
}
