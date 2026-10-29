//! Jobs (`Transient.Move.Job`): durable checkpoints. A `job` stores the closure where it is; if the process dies, a
//! restarted node that runs `run_jobs` (and is told `jobs exec` in the console) resumes it by replaying the log of the
//! closures up to there.
//!
//! A job is closed (its record removed) by the next `job`, or when a call to another node is sent without error
//! (`finish_job`, called by `teleport`): from then on the rest of the computation is in the other node, with its own
//! jobs. A `job_task` protects a whole computation: it is closed only when the computation completes.
//!
//! A recovered node says hello to the others (`Ping`), and a node that gets it launches the jobs that were waiting
//! for the sender: the ones whose call failed because that node was down.
//!
//! Not here yet: `config` and `clear`.

use super::*;
use node::{MyNode, Node};

/// The error of a failed connection (`ConnectionError`)
#[derive(Clone, Debug)]
pub struct ConnectionError {
    pub msg: String,
    pub node: Node,
}

/// A durable checkpoint (`job`)
pub fn job() -> Cloud<()> {
    job_with(false, None)
}

/// A durable checkpoint with the name of its closure (`job'`), so that it can be found from outside
pub fn job_named(name: &str) -> Cloud<()> {
    job_with(false, Some(name.to_string()))
}

/// A job that protects a whole computation: it is closed only when `x` completes, not when a call that `x` sends to
/// another node is sent. If the program is interrupted before, `x` is executed again from the start when it is
/// restarted (the calls that `x` did are done again: "at least once").
pub fn job_task<A: Loggable + Clone + Send + Sync + 'static>(x: Cloud<A>) -> Cloud<A> {
    cdo! {
        [x]
        job_with(true, None);
        let r = x;
        on_all(Trans::new(|ctx| {
            if let Some(j) = ctx.get::<Job>() {
                job_remove(ctx, &j);
            }
            Some(())
        }));
        cpure(r)
    }
}

fn job_with(scoped: bool, name: Option<String>) -> Cloud<()> {
    local(sandbox_data::<PrevClos, _>(Trans::new(move |ctx| {
        // the next job checkpoint eliminates the previous one, to reflect the advance of the computation
        if let Some(prev) = ctx.get::<Job>() {
            job_remove(ctx, &prev);
        }
        or(endpoint(name.clone()), pure(())).run(ctx)?;
        let this = ctx.get::<PrevClos>().expect("no job state").prev.clone();
        let node = ctx.get::<MyNode>().expect("job: use init_node").0.clone();
        let job = Job { closure: this, node, error: None, scoped };
        let table = closures(ctx);
        // a job asks for the state to be saved when the program ends (`writeIORef save True`)
        save_on_exit();
        table.write_job(job.clone());
        // a job is a promise that the computation will finish even if the process dies: it is on disk before going on
        if table.flush_on_job() {
            table.flush();
        }
        ctx.set(job.clone());
        // a failure is recorded in the job; a connection error also says which node it is waiting for
        on_exception(move |e: SomeException| {
            let (job, table) = (job.clone(), table.clone());
            lift_io(move || {
                let updated = match e.cast::<ConnectionError>() {
                    Some(c) => Job { node: c.node, error: Some(format!("{e}")), ..job.clone() },
                    None => Job { error: Some(format!("{e}")), ..job.clone() },
                };
                table.write_job(updated);
            })
        })
        .run(ctx)
    })))
}

fn job_remove(ctx: &mut Ctx, job: &Job) {
    closures(ctx).remove_job(&job.closure);
    ctx.del::<Job>();
}

/// A job is finished when a later job is executed. When the later job is in another node, the job is finished when the
/// call to that node has been sent without error: if this node is restarted, the call is not repeated. The jobs of a
/// `job_task` are not finished by a sending: they are finished when the task is completed.
pub(crate) fn finish_job(ctx: &mut Ctx) {
    if let Some(j) = ctx.get::<Job>().filter(|j| !j.scoped) {
        job_remove(ctx, &j);
    }
}

/// Resumes the pending jobs of this node on startup, when the console gets `jobs exec` (`runJobs`): each job replays
/// the log of its closure, so the computation continues from where it was interrupted.
pub fn run_jobs() -> Cloud<()> {
    fork_cloud(cdo! {
        let _jobs = local(option("jobs", "manage jobs"));
        let _exec = local(option1("exec", "execute jobs for this node"));
        local(start_jobs()) | local(comm_jobs()) | ping_nodes()
    })
}

/// The announcement that a node is up: the jobs that were waiting for it can run (`Ping`)
#[derive(Clone, Debug)]
pub struct Ping(pub Node);

/// Launches the jobs that wait for the node that sends a `Ping` (`commJobs`)
fn comm_jobs() -> Trans<()> {
    tdo! {
        let Ping(node) = get_mailbox::<Ping>();
        launch_jobs_for_node(&node)
    }
}

/// A recovered node says hello to the other known nodes: it executes in each one the announcement of its ping. The
/// `comm_jobs` of that node launches the jobs that were waiting for this node (the ones whose sending failed, see `job`).
/// This is why `run_jobs` is a `Cloud` computation. These steps are logged: the node that receives the call replays
/// the program with the log, where `mynode` is the node that pings and `node` is the node that receives the ping.
fn ping_nodes() -> Cloud<()> {
    cdo! {
        let mynode = local(get_my_node());
        let nodes = local(get_nodes());
        let node = local(for_(nodes.iter().filter(|n| **n != mynode).cloned().collect::<Vec<_>>()));
        ping_one(mynode, node);
        cempty()
    }
}

fn ping_one(mynode: Node, node: Node) -> Cloud<()> {
    run_at(node, local(lift_io(move || put_mailbox(Ping(mynode.clone())))))
}

fn start_jobs() -> Trans<()> {
    Trans::new(|ctx| {
        let node = ctx.get::<MyNode>().expect("run_jobs: use init_node").0.clone();
        launch_jobs_for_node(&node).run(ctx)
    })
}

/// Sends the log of each job of `node` to the closure 0, which replays the program up to the job
fn launch_jobs_for_node(node: &Node) -> Trans<()> {
    let node = node.clone();
    Trans::new(move |ctx| {
        let table = closures(ctx);
        let mut jobs = table.jobs_for(&node);
        jobs.reverse();
        for job in jobs {
            let (session, clos) = get_sess_closure(&job.closure);
            let (logs, target) = get_closure_log(&table, &session, &clos);
            ctx.set(job.clone());
            let conn = ctx.get::<Connection>().map(|c| (*c).clone());
            let ev = target.evar.expect("launch jobs: closure 0 is not alive");
            ev.write(Message { last: false, logs, session, clos: "0".into(), from_remote: false, conn, to_respond: None, headers: None });
        }
        None
    })
}

/// `(abduce >> x >> empty) <|> return ()` logged: runs `x` aside and goes on (`forkCloud`)
pub fn fork_cloud(x: Cloud<()>) -> Cloud<()> {
    let aside = cdo! { [x] local(abduce()); x; cempty() };
    aside | cpure(())
}

// ---------------------------------------------------------------------------------------------------------------
// collectc

/// The current time in microseconds (`getMicroSeconds`)
pub fn get_micro_seconds() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64)
}

/// The state of a thread: what `collectc` joins when its branches end (the `PolyMap` of the original, with the log in it)
#[derive(Clone)]
struct Merged {
    data: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
    log: Log,
}

impl Merged {
    fn of(ctx: &Ctx) -> Merged {
        Merged { data: (*ctx.data).clone(), log: ctx.log.clone() }
    }

    /// `merge x y`: what both have; for the same type the stacks of handlers are joined, the fragments that remain to be
    /// replayed are joined, and for any other type the value of `y` wins
    fn merge(x: &Merged, y: &Merged) -> Merged {
        let mergers = y.data.get(&TypeId::of::<back::Mergers>()).and_then(|m| m.downcast_ref::<back::Mergers>()).cloned().unwrap_or_default();
        let mut data = x.data.clone();
        for (t, v) in &y.data {
            let merged = match (data.get(t), mergers.0.get(t)) {
                (Some(old), Some(merge)) => merge(old, v),
                _ => v.clone(),
            };
            data.insert(*t, merged);
        }
        let rest_log = x.log.rest_log.iter().chain(&y.log.rest_log).cloned().collect();
        Merged { data, log: Log { rest_log, ..y.log.clone() } }
    }
}

/// Restores the previous closure when `x` ends, but with the `is_collect` it had before: the flag of an enclosing
/// `collectc` is not lost (`preserveCollectFlag`)
fn sandbox_collect_flag<A: Send + 'static>(x: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let saved = ctx.get::<PrevClos>().expect("PrevClos information");
        let restore = Arc::new(move |ctx: &mut Ctx| {
            let now = ctx.get::<PrevClos>().expect("PrevClos information");
            ctx.set(PrevClos { is_collect: saved.is_collect, ..(*now).clone() });
        });
        ops::after_each(x.clone(), restore).run(ctx)
    })
}

/// Persistent `collect` that preserves the backtracking handlers: gathers `n` results (all the ones that come, if `n`
/// is 0) of `action` and its threads, or those that arrive in `delta` microseconds (no limit if 0). Unlike `collect`, the
/// results are kept across restarts: every thread that reaches the end of `action` leaves a `job` and its path is added to
/// the one of the closure (see `endpoint`), so a restarted node replays them all and the collection goes on where it was.
/// The time is counted from the first execution, not from the restart.
pub fn collectc<A: Loggable + serde::Serialize + serde::de::DeserializeOwned + Clone + Send + Sync + 'static>(n: usize, delta: u64, action: Cloud<A>) -> Cloud<Vec<A>> {
    on_all(sandbox_collect_flag(collectc_inner(n, delta, action).un_cloud()))
}

fn collectc_inner<A: Loggable + serde::Serialize + serde::de::DeserializeOwned + Clone + Send + Sync + 'static>(n: usize, delta: u64, action: Cloud<A>) -> Cloud<Vec<A>> {
    cdo! {
        [action]
        on_all(Trans::new(|ctx| {
            let p = ctx.get::<PrevClos>().expect("collectc: no Previous closure");
            ctx.set(PrevClos { is_collect: true, ..(*p).clone() });
            Some(())
        }));
        let tinit = local_io(|| get_micro_seconds());
        let initial = on_all(Trans::new(|ctx| Some(Arc::new(Merged::of(ctx)))));
        let states = on_all(Trans::new(|_| Some(Arc::new(Mutex::new(None::<Merged>)))));
        let jobs = on_all(Trans::new(|_| Some(Arc::new(Mutex::new(Vec::<Job>::new())))));
        let r = local(collect_for(n, (delta > 0).then(|| std::time::Duration::from_micros((tinit + delta).saturating_sub(get_micro_seconds()).max(1))), collected_branch(action.clone(), states.clone(), jobs.clone()).un_cloud()));
        on_all(Trans::new(move |ctx| {
            // the state that goes on is the join of the one at the start and the ones at the end of the branches
            let merged = match &*states.lock().unwrap() {
                Some(s) => Merged::merge(&initial, s),
                None => (*initial).clone(),
            };
            // the closure that follows is reached from the one before the collection, with the log that has the result of
            // the collection (written when `local` returned): not from the last branch, whose path is inside the collection
            let mut data = merged.data;
            let before = initial.data.get(&TypeId::of::<PrevClos>()).and_then(|p| p.downcast_ref::<PrevClos>());
            let last = data.get(&TypeId::of::<PrevClos>()).and_then(|p| p.downcast_ref::<PrevClos>());
            if let (Some(before), Some(last)) = (before, last) {
                // the session of the last call stays: the next endpoint belongs to it
                let new = PrevClos { prev: before.prev.clone(), ..last.clone() };
                data.insert(TypeId::of::<PrevClos>(), Arc::new(new));
            }
            ctx.data = Arc::new(data);
            ctx.log = Log { rest_log: merged.log.rest_log, ..ctx.log.clone() };
            // the jobs of the branches are not needed any more
            for j in jobs.lock().unwrap().drain(..) {
                job_remove(ctx, &j);
            }
            Some(())
        }));
        cpure(r)
    }
}

/// What every thread of a `collectc` does: runs `action`, leaves a durable checkpoint with the path preserved, and joins
/// its state with the one of the other branches
fn collected_branch<A: Clone + Send + Sync + 'static>(action: Cloud<A>, states: Arc<Mutex<Option<Merged>>>, jobs: Arc<Mutex<Vec<Job>>>) -> Cloud<A> {
    cdo! {
        [action, states, jobs]
        let r = action.clone();
        on_all(Trans::new(|ctx| {
            let p = ctx.get::<PrevClos>().expect("no PrevClos state");
            ctx.set(PrevClos { preserve_path: true, ..(*p).clone() });
            Some(())
        }));
        job();
        on_all(Trans::new(move |ctx| {
            let j = ctx.get::<Job>().expect("collectc: no Job state");
            jobs.lock().unwrap().push((*j).clone());
            let mut acc = states.lock().unwrap();
            let now = Merged::of(ctx);
            *acc = Some(match &*acc {
                Some(before) => Merged::merge(before, &now),
                None => now,
            });
            Some(())
        }));
        cpure(r)
    }
}
