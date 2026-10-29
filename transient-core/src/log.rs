//! The log of the execution (`Log` of `LEngine.hs`) and `logged` (`Transient.Move.Logged`), without the distributed part.
//!
//! The log lives in the state. `logged` runs a computation and writes its result in `part_log`; if the text being
//! parsed is not empty (a log being replayed) it reads the result from there instead of running the computation:
//!
//! * `e/` — the computation was executed: it is executed again (this is what is inside a `logged` that did not finish)
//! * `w/` — the branch was left waiting (`<|>` with an asynchronous computation): a replay skips it
//! * `value/` — the result, in the `Loggable` format
//!
//! The hash of the path (`hash`) names the continuation points: it is the same in all the nodes that run the same
//! code.
//!
//! The applicative (`Pending`, `Wait`) is not here yet. The continuation points (`endpoint`) are in `closure.rs`.

use super::*;

/// A segment of the log: written text, or a hole whose content is read when the log is serialised (`LogSeg`). The holes
/// are for the terms of an applicative that are not resolved yet: the log that is sent to another node carries their
/// value if they are resolved by then, and `w/` if they are not.
#[derive(Clone)]
pub enum LogSeg {
    Fixed(String),
    Pending(Arc<dyn Hole>),
}

/// The hole of a term that may not be resolved yet
pub trait Hole: Send + Sync {
    /// What the log has in the hole now: the value of the term if it is resolved, `w/` marks if it is not
    fn read(&self) -> LogB;
    /// The term is resolved
    fn resolved(&self) -> bool;
}

/// The log as a list of segments (`LogB`)
#[derive(Clone, Default)]
pub struct LogB(pub Vec<LogSeg>);

impl LogB {
    /// The text of the log, reading the holes now (`renderLog`)
    pub fn render(&self) -> String {
        self.0
            .iter()
            .map(|seg| match seg {
                LogSeg::Fixed(s) => s.clone(),
                LogSeg::Pending(h) => h.read().render(),
            })
            .collect()
    }

    /// No hole of the log is waiting for a term: what `render` gives is definitive
    pub fn is_resolved(&self) -> bool {
        self.0.iter().all(|seg| match seg {
            LogSeg::Fixed(_) => true,
            LogSeg::Pending(h) => h.resolved() && h.read().is_resolved(),
        })
    }

    /// Nothing written (`nullLog`)
    pub fn is_null(&self) -> bool {
        self.render().is_empty()
    }

    /// The same log with `s` at the end. The segments are not merged: the applicative relies on prefixes.
    pub fn append(&self, s: &str) -> LogB {
        let mut segs = self.0.clone();
        segs.push(LogSeg::Fixed(s.to_string()));
        LogB(segs)
    }
}

impl From<&str> for LogB {
    fn from(s: &str) -> LogB {
        LogB(vec![LogSeg::Fixed(s.to_string())])
    }
}

impl From<String> for LogB {
    fn from(s: String) -> LogB {
        LogB(vec![LogSeg::Fixed(s)])
    }
}

impl std::fmt::Debug for LogB {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.render())
    }
}

impl PartialEq for LogB {
    fn eq(&self, other: &LogB) -> bool {
        self.render() == other.render()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Log {
    /// a log is being replayed
    pub recover: bool,
    /// what has been written since the last continuation point
    pub part_log: LogB,
    /// the fragments that remain to be replayed, one per continuation point (`restLog`)
    pub rest_log: Vec<Fragment>,
    /// the hash of the path
    pub hash: i64,
}

impl Default for Log {
    fn default() -> Self {
        Log { recover: false, part_log: LogB::default(), rest_log: Vec::new(), hash: 0 }
    }
}

pub const HASH_EXEC: i64 = 1;
pub const HASH_DONE: i64 = 2;
pub const HASH_WAIT: i64 = 3;
pub const HASH_APP: i64 = 4;
pub const HASH_TERM: i64 = 5;
pub const HASH_NODE: i64 = 6;

/// `hashStep`: the arithmetic is 64-bit on every platform
pub fn hash_step(h: i64, c: i64) -> i64 {
    (h * 31 + c) % 2147483647
}

pub fn get_log() -> Trans<Log> {
    Trans::new(|ctx| Some(ctx.log.clone()))
}

pub fn put_log(l: Log) -> Trans<()> {
    Trans::new(move |ctx| {
        ctx.log = l.clone();
        Some(())
    })
}

/// Runs `mx`, writes its result in the log and returns it. With a log to replay it reads the result instead of running.
pub fn logged<A: Loggable + Send + Sync + 'static>(mx: Trans<A>) -> Trans<A> {
    logged_with(false, mx)
}

/// `logged` with the compaction as a parameter. With `compact` the log of the statement is always replaced by its
/// result, even if it went through continuation points (it is for the calls between nodes that do not have the same
/// code). Without `compact` the continuation points inside decide (`preserve_path`).
pub fn logged_with<A: Loggable + Send + Sync + 'static>(compact: bool, mx: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let prev = ctx.get::<PrevClos>();
        if let Some(p) = &prev {
            ctx.set(PrevClos { preserve_path: false, ..(**p).clone() });
        }
        let initial = ctx.log.clone();
        let rest = get_parse_buffer().run(ctx)?;
        ctx.log = Log { part_log: initial.part_log.append("e/"), hash: hash_step(initial.hash, HASH_EXEC), ..initial.clone() };

        // the path is within an applicative term: `receive1` must not record its closures as the last ones of the connection
        let app = ctx.is_applicative;
        if app {
            if let Some(p) = ctx.get::<PrevClos>() {
                ctx.set(PrevClos { is_applicativ: true, ..(*p).clone() });
            }
        }

        let run = if rest.is_empty() { mx.clone() } else { recover_it(mx.clone()) };
        // nested logged statements within `mx` are not applicative terms; the flag is restored afterwards, also when
        // `mx` fails or goes on asynchronously in other threads
        let run = after_each(
            Trans::new(move |ctx: &mut Ctx| {
                ctx.is_applicative = false;
                run.run(ctx)
            }),
            Arc::new(move |ctx: &mut Ctx| ctx.is_applicative = app),
        );
        let (i1, i2) = (initial.clone(), initial);
        let body = or(run, add_wait(compact, app, i1));
        let t = tdo! {
            [i2]
            let r = body;
            finish(compact, app, i2, r)
        };
        match prev {
            None => t.run(ctx),
            Some(prev) => after_each(t, Arc::new(move |ctx: &mut Ctx| handle(compact, &prev, ctx))).run(ctx),
        }
    })
}

/// When the statement ends, the previous closure goes back to what it was, unless it went through continuation
/// points and it is not compacted (`handle` of `logged'`)
fn handle(compact: bool, prev: &PrevClos, ctx: &mut Ctx) {
    let Some(now) = ctx.get::<PrevClos>() else { return };
    // a term of an applicative (the statement that set `is_applicativ`) is compacted to its value relative to the
    // closure previous to the term: its internal closures are not part of the path
    let new = if now.is_applicativ && !prev.is_applicativ || compact {
        prev.clone()
    } else {
        PrevClos {
            prev: now.prev.clone(),
            new_session: now.new_session.clone(),
            preserve_path: now.preserve_path || prev.preserve_path,
            is_applicativ: prev.is_applicativ,
            is_collect: now.is_collect || prev.is_collect,
        }
    };
    ctx.set(new);
}

fn preserve_path(ctx: &Ctx) -> bool {
    ctx.get::<PrevClos>().is_some_and(|p| p.preserve_path)
}

/// The alternative branch has been chosen: it leaves `w/` so that a replay skips the computation
fn add_wait<A: Send + 'static>(compact: bool, app: bool, initial: Log) -> Trans<A> {
    Trans::new(move |ctx| {
        let rest = give_parse_string().run(ctx)?;
        let recover = !(rest.is_empty() && initial.rest_log.is_empty());
        let part_log = if preserve_path(ctx) && !compact { ctx.log.part_log.append("w/") } else { initial.part_log.append("w/") };
        // in an applicative the first term counts as one step whatever it is resolved or not (`lift2`): same hash as the
        // path with its value, so that endpoint names do not depend on whether it was resolved
        let hash = if app { hash_step(initial.hash, HASH_TERM) } else { hash_step(ctx.log.hash, HASH_WAIT) };
        ctx.log = Log { recover, part_log, hash, ..ctx.log.clone() };
        None
    })
}

/// After the result, in the thread that receives it: writes it in the log
fn finish<A: Loggable + Send + 'static>(compact: bool, app: bool, initial: Log, r: A) -> Trans<A> {
    let r = Mutex::new(Some(r));
    Trans::new(move |ctx| {
        let r = r.lock().unwrap().take()?;
        let rest = give_parse_string().run(ctx)?;
        let recover = !(rest.is_empty() && initial.rest_log.is_empty());
        // an applicative term is always compacted to its value, even if it went through endpoints (e.g. `run_at`): a node
        // that replayed its internal path would follow its teleports and could not complete the applicative
        ctx.log = if preserve_path(ctx) && !app && !compact {
            // the path inside stays in the log: a replay goes through `mx` again and adds the same
            // hashes. It goes on from the final hash so that later points do not coincide with the inner ones.
            Log { recover, hash: hash_step(ctx.log.hash, HASH_DONE), ..ctx.log.clone() }
        } else {
            Log {
                recover,
                part_log: initial.part_log.append(&format!("{}/", to_log(&r))),
                hash: hash_step(initial.hash, HASH_DONE),
                ..ctx.log.clone()
            }
        };
        Some(r)
    })
}

fn recover_it<A: Loggable + Send + Sync + 'static>(mx: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let s = give_parse_string().run(ctx)?;
        match s.get(..2) {
            Some("e/") => {
                t_drop(2).run(ctx)?;
                mx.run(ctx)
            }
            Some("w/") => {
                t_drop(2).run(ctx)?;
                if ctx.mode != Mode::Remote {
                    ctx.mode = Mode::Parallel;
                }
                None
            }
            _ => {
                let (x, left) = from_log::<A>(&s).unwrap_or_else(|| {
                    panic!("error parsing <{s}> to {}", std::any::type_name::<A>())
                });
                let used = s[..s.len() - left.len()].chars().count();
                t_drop(used).run(ctx)?;
                if left.starts_with('/') {
                    t_drop(1).run(ctx)?;
                }
                Some(x)
            }
        }
    })
}
