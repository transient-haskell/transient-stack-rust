//! Continuation points (`Transient.Move.Logged`): `endpoint`, the closure table (`LocalClosure`) and `PrevClos`.
//!
//! An `endpoint` stores the continuation of the program at that point under a stable name (the hash of the path, the
//! same in all the nodes that run the same code). Whoever knows the name can send it a log and the program
//! goes on from there, reading the steps already done from the log (see `logged`).
//!
//! * The continuation is the reader of an `EVar` (`read_evar`): it does not need to be stored in the table. The table
//!   keeps the log fragment since the previous closure, to be able to rebuild the path, and whether the continuation is alive.
//! * A `collectc` makes the closure keep every path that reached it (`Fragment::Collect`).

use super::*;
use std::time::{Duration, Instant};

/// A stretch of the log between two continuation points (`FragmentType`): one path, or all the paths that a
/// `collectc` has gathered in a closure (`Collect`, oldest last)
#[derive(Clone, Debug, PartialEq)]
pub enum Fragment {
    Text(String),
    Collect(Vec<String>),
}

impl Fragment {
    pub fn text(s: impl Into<String>) -> Fragment {
        Fragment::Text(s.into())
    }

    /// The text of a single path
    pub fn as_text(&self) -> &str {
        match self {
            Fragment::Text(s) => s,
            Fragment::Collect(_) => panic!("a Collect fragment has several paths"),
        }
    }
}

/// The previous closure in the flow of the execution (`PrevClos`)
#[derive(Clone, Debug)]
pub struct PrevClos {
    pub prev: String,
    pub new_session: Option<String>,
    pub preserve_path: bool,
    pub is_applicativ: bool,
    pub is_collect: bool,
}

impl Default for PrevClos {
    fn default() -> Self {
        PrevClos { prev: CLOS0.into(), new_session: None, preserve_path: false, is_applicativ: false, is_collect: false }
    }
}

pub const CLOS0: &str = "0-0";

/// The key of a closure: `<closure>-<session>` (`kLocalClos`)
pub fn k_local_clos(session: &str, clos: &str) -> String {
    format!("{clos}-{session}")
}

/// The session and the closure of a key (`getSessClosure`)
pub fn get_sess_closure(key: &str) -> (String, String) {
    let (c, s) = key.split_once('-').unwrap();
    (s.to_string(), c.to_string())
}

/// What a continuation point receives: whether it is the last message, the fragments, the session and the closure of
/// the sender, and whether they come from another node
#[derive(Clone)]
pub struct Message {
    pub last: bool,
    pub logs: Vec<Fragment>,
    pub session: String,
    pub clos: String,
    pub from_remote: bool,
    /// the connection it arrived by, and the closure to respond to: they pass to the state of the continuation
    pub conn: Option<Connection>,
    pub to_respond: Option<ClosToRespond>,
    /// the headers of the HTTP request that sent it, if it was one
    pub headers: Option<HttpHeaders>,
}

#[derive(Clone)]
pub struct LocalClosure {
    pub session: String,
    pub clos: String,
    pub prev: String,
    pub local_log: LogB,
    /// the paths that a `collectc` gathered before `local_log`, the newest first (`Collect` of `localLog`)
    pub collected: Option<Vec<LogB>>,
    /// the closure this one is a copy of, when it was made by a jump to another session (`#<source>-<new>`)
    pub source: Option<String>,
    pub evar: Option<EVar<Message>>,
    /// the continuation is alive in this process (`localCont`)
    pub live: bool,
    /// the node that called this one when the closure was created by the arrival of a request (the first teleport of
    /// a `run_at`, in the called side). It is persisted: after a restart the connection to the caller does not exist
    /// and the second teleport opens it with this node (`localCaller`).
    pub caller: Option<Node>,
}

/// What of a closure is stored on disk (the TCache file of Haskell): the continuation, the event variable and
/// whether it is alive only exist in the process
#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    session: String,
    clos: String,
    prev: String,
    log: String,
    #[serde(default)]
    collected: Option<Vec<String>>,
    #[serde(default)]
    source: Option<String>,
    caller: Option<Node>,
}

impl LocalClosure {
    /// The log of the closure as it travels (`localLog`)
    pub fn fragment(&self) -> Fragment {
        match &self.collected {
            None => Fragment::Text(self.local_log.render()),
            Some(rest) => Fragment::Collect(std::iter::once(&self.local_log).chain(rest).map(LogB::render).collect()),
        }
    }

    fn is_resolved(&self) -> bool {
        self.local_log.is_resolved() && self.collected.iter().flatten().all(LogB::is_resolved)
    }
}

/// A job: a durable checkpoint (`Job`). It is the closure to resume, the node that has to be up for it to run, and the
/// last error.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Job {
    pub closure: String,
    pub node: Node,
    pub error: Option<String>,
    /// created by `job_task`: it is not closed by the sending of a call
    pub scoped: bool,
}

/// What is waiting to be written to disk: closures and jobs to write, jobs to delete
#[derive(Default)]
struct Dirty {
    closures: std::collections::HashSet<String>,
    jobs: std::collections::HashSet<String>,
    removed_jobs: std::collections::HashSet<String>,
}

/// When the closures are written to disk and which ones stay in memory: the policy of the cache (`SyncMode` of TCache,
/// `savepol` of the console of Transient).
#[derive(Clone)]
pub struct SyncPolicy {
    /// Time between two writes of what has changed. `None`: only when `Closures::flush` is called (`SyncManual`)
    pub frequency: Option<std::time::Duration>,
    /// When the cache has more closures than this, the cold ones are dropped from memory after writing them; they are
    /// read from disk again if they are needed
    pub cache_size: usize,
    /// Which closures are cold: receives the current time, the last access to the closure and the last write
    pub check: fn(Instant, Instant, Instant) -> bool,
    /// A `job` writes what is pending before the computation goes on: it is the promise that it will finish even if the
    /// process dies. Without it, a job is as durable as the last write.
    pub flush_on_job: bool,
}

impl Default for SyncPolicy {
    /// The defaults of `savepol` in Transient: every 10 seconds, 1000 objects
    fn default() -> Self {
        SyncPolicy { frequency: Some(std::time::Duration::from_secs(10)), cache_size: 1000, check: default_check, flush_on_job: true }
    }
}

impl SyncPolicy {
    /// The default policy, with the period of the writes in the environment variable `TRANSIENT_SYNC_MS` (milliseconds)
    /// if it is set (the tests use a short one so that a process that is killed has written), and `TRANSIENT_FLUSH_ON_JOB=0`
    /// to make a `job` not write before going on
    pub fn from_env() -> SyncPolicy {
        let frequency = std::env::var("TRANSIENT_SYNC_MS").ok().and_then(|v| v.parse().ok()).map(std::time::Duration::from_millis);
        SyncPolicy {
            frequency: frequency.or(SyncPolicy::default().frequency),
            flush_on_job: std::env::var("TRANSIENT_FLUSH_ON_JOB").map_or(true, |v| v != "0"),
            ..SyncPolicy::default()
        }
    }
}

/// The closures not accessed since half the time between now and the last write are cold (`defaultCheck`)
pub fn default_check(now: Instant, last_access: Instant, last_sync: Instant) -> bool {
    let halftime = now - now.saturating_duration_since(last_sync) / 2;
    last_access <= halftime
}

struct Entry {
    lc: LocalClosure,
    last_access: Instant,
}

/// The closure table of a node. Each node has its own (in Haskell it is the TCache of the process): it is in the state,
/// and it is shared by all the threads born from it.
///
/// With a directory it is also persistent, with the policy of TCache. Every closure is a file named after its key. The
/// table is a cache: a closure is read from its file when it is needed and it is not in memory, so a node that starts
/// with the directory of its previous life finds the closures of that life (not alive). The writes are **deferred and
/// grouped**: a change only marks the entry as dirty, and the dirty entries are written every `frequency`, so the cost on
/// the path of the program is a set insertion, and an entry that changes many times (the closure of a stream) is
/// serialised once per period. The log is rendered when the file is written, so the holes of the applicative are resolved
/// if the terms are; an entry with a hole still waiting is kept dirty until it is resolved. After writing, if the cache
/// has more than `cache_size` closures, the cold ones that are not alive are dropped from memory.
///
/// What has not been written is lost if the process dies (as in Haskell, where the state is saved at the period, on
/// `save`, or on exit with `saveonexit`).
pub struct Closures {
    map: Mutex<HashMap<String, Entry>>,
    jobs: Mutex<HashMap<String, Job>>,
    dirty: Mutex<Dirty>,
    last_sync: Mutex<Instant>,
    /// how many closures have been serialised and written (to measure the policy)
    serialised: std::sync::atomic::AtomicUsize,
    dir: Option<std::path::PathBuf>,
    policy: Mutex<SyncPolicy>,
}

impl Default for Closures {
    fn default() -> Closures {
        Closures {
            map: Mutex::default(),
            jobs: Mutex::default(),
            dirty: Mutex::default(),
            last_sync: Mutex::new(Instant::now()),
            serialised: Default::default(),
            dir: None,
            policy: Mutex::new(SyncPolicy::default()),
        }
    }
}

impl Closures {
    /// A table persisted in `dir` with the default policy
    pub fn open(dir: &std::path::Path) -> Closures {
        Closures::open_with(dir, SyncPolicy::default())
    }

    /// A table persisted in `dir`. The jobs are loaded now (there are few and the node has to launch them); the closures
    /// are read when they are needed.
    pub fn open_with(dir: &std::path::Path, policy: SyncPolicy) -> Closures {
        std::fs::create_dir_all(dir).expect("closures directory");
        let jobs_dir = dir.join("jobs");
        std::fs::create_dir_all(&jobs_dir).expect("jobs directory");
        let jobs = std::fs::read_dir(&jobs_dir)
            .expect("jobs directory")
            .filter_map(|e| std::fs::read_to_string(e.ok()?.path()).ok())
            .filter_map(|text| serde_json::from_str::<Job>(&text).ok())
            .map(|j| (j.closure.clone(), j))
            .collect();
        Closures { map: Mutex::default(), jobs: Mutex::new(jobs), dirty: Mutex::default(), last_sync: Mutex::new(Instant::now()), serialised: Default::default(), dir: Some(dir.to_path_buf()), policy: Mutex::new(policy) }
    }

    /// Starts the thread that writes the dirty entries at the period of the policy; it ends when the table is dropped.
    /// Nothing to do without a directory or in manual mode.
    pub fn start_flusher(self: &Arc<Self>) {
        self.start_flusher_for(None)
    }

    /// `start_flusher`, saying which `keep` the signals end
    pub(crate) fn start_flusher_for(self: &Arc<Self>, exit: Option<Arc<ExitSlot>>) {
        if self.dir.is_none() {
            return;
        }
        register(self, exit);
        let weak = Arc::downgrade(self);
        thread::spawn(move || loop {
            // the period can be changed while the node runs (`savepol`): it is looked at every tick, which is shorter
            // than the period or 100 ms; nothing is serialised unless the period has passed
            let frequency = match weak.upgrade() {
                Some(table) => table.policy.lock().unwrap().frequency,
                None => break,
            };
            thread::sleep(frequency.map_or(Duration::from_millis(100), |f| f.min(Duration::from_millis(100))));
            match weak.upgrade() {
                Some(table) => {
                    if frequency.is_some_and(|f| table.last_sync.lock().unwrap().elapsed() >= f) {
                        table.flush();
                    }
                }
                None => break,
            }
        });
    }

    /// Changes the period of the writes and the size of the cache while the node runs (`savepol`)
    pub fn set_policy(&self, frequency: Option<Duration>, cache_size: usize) {
        let mut p = self.policy.lock().unwrap();
        p.frequency = frequency;
        p.cache_size = cache_size;
    }

    /// Writes now what changed since the last write, and then drops the cold closures if the cache is too big
    /// (`syncCache`, `clearSyncCache`)
    pub fn flush(&self) {
        let Some(dir) = &self.dir else { return };
        let (closures, jobs, removed) = {
            let mut d = self.dirty.lock().unwrap();
            (std::mem::take(&mut d.closures), std::mem::take(&mut d.jobs), std::mem::take(&mut d.removed_jobs))
        };
        for key in closures {
            let Some(lc) = self.peek(&key) else { continue };
            // a hole of the log that waits for a term: the file is written later, with its value
            if !lc.is_resolved() {
                self.dirty.lock().unwrap().closures.insert(key.clone());
            }
            let stored = Stored { session: lc.session, clos: lc.clos, prev: lc.prev, log: lc.local_log.render(), collected: lc.collected.as_ref().map(|c| c.iter().map(LogB::render).collect()), source: lc.source, caller: lc.caller };
            write_atomically(&dir.join(&key), &serde_json::to_string(&stored).unwrap());
            self.serialised.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        for closure in jobs {
            if let Some(job) = self.jobs.lock().unwrap().get(&closure).cloned() {
                write_atomically(&dir.join("jobs").join(&closure), &serde_json::to_string(&job).unwrap());
            }
        }
        for closure in removed {
            let _ = std::fs::remove_file(dir.join("jobs").join(closure));
        }
        let (now, last) = (Instant::now(), std::mem::replace(&mut *self.last_sync.lock().unwrap(), Instant::now()));
        self.clear_cold(now, last);
    }

    /// Drops from memory the cold closures, if there are more than `cache_size`. Only the ones that are written and not
    /// alive: a live closure is the way to its continuation.
    fn clear_cold(&self, now: Instant, last_sync: Instant) {
        let mut map = self.map.lock().unwrap();
        let policy = self.policy.lock().unwrap().clone();
        if map.len() <= policy.cache_size {
            return;
        }
        let dirty = self.dirty.lock().unwrap();
        map.retain(|key, e| e.lc.live || dirty.closures.contains(key) || !(policy.check)(now, e.last_access, last_sync));
    }

    /// Stores a job (`newDBRef`, `writeDBRef`)
    pub fn write_job(&self, job: Job) {
        let mut d = self.dirty.lock().unwrap();
        d.removed_jobs.remove(&job.closure);
        d.jobs.insert(job.closure.clone());
        drop(d);
        self.jobs.lock().unwrap().insert(job.closure.clone(), job);
    }

    /// Removes a job (`delDBRef`)
    pub fn remove_job(&self, closure: &str) {
        let mut d = self.dirty.lock().unwrap();
        d.jobs.remove(closure);
        d.removed_jobs.insert(closure.to_string());
        drop(d);
        self.jobs.lock().unwrap().remove(closure);
    }

    /// The jobs that wait for `node` to be up, the oldest first (`jobNode .==. node`)
    pub fn jobs_for(&self, node: &Node) -> Vec<Job> {
        let mut js: Vec<Job> = self.jobs.lock().unwrap().values().filter(|j| &j.node == node).cloned().collect();
        js.sort_by(|a, b| a.closure.cmp(&b.closure));
        js
    }

    /// Whether some closure of the session exists, in memory or on disk
    pub fn knows_session(&self, session: &str) -> bool {
        let suffix = format!("-{session}");
        if self.map.lock().unwrap().keys().any(|k| k.ends_with(&suffix)) {
            return true;
        }
        self.dir.as_ref().and_then(|d| std::fs::read_dir(d).ok()).is_some_and(|mut files| files.any(|f| f.is_ok_and(|f| f.file_name().to_string_lossy().ends_with(&suffix))))
    }

    /// The closure in memory, without touching its access time
    fn peek(&self, key: &str) -> Option<LocalClosure> {
        self.map.lock().unwrap().get(key).map(|e| e.lc.clone())
    }

    /// The closure, from memory or else from its file (`readDBRef`)
    pub fn read(&self, key: &str) -> Option<LocalClosure> {
        let mut map = self.map.lock().unwrap();
        if let Some(e) = map.get_mut(key) {
            e.last_access = Instant::now();
            return Some(e.lc.clone());
        }
        let text = std::fs::read_to_string(self.dir.as_ref()?.join(key)).ok()?;
        let s: Stored = serde_json::from_str(&text).ok()?;
        let lc = LocalClosure { session: s.session, clos: s.clos, prev: s.prev, local_log: LogB::from(s.log), collected: s.collected.map(|c| c.into_iter().map(LogB::from).collect()), source: s.source, evar: None, live: false, caller: s.caller };
        map.insert(key.to_string(), Entry { lc: lc.clone(), last_access: Instant::now() });
        Some(lc)
    }

    pub fn write(&self, key: String, lc: LocalClosure) {
        if self.dir.is_some() {
            self.dirty.lock().unwrap().closures.insert(key.clone());
        }
        self.map.lock().unwrap().insert(key, Entry { lc, last_access: Instant::now() });
    }

    fn update(&self, key: &str, f: impl FnOnce(&mut LocalClosure)) {
        f(&mut self.map.lock().unwrap().get_mut(key).unwrap().lc)
    }

    /// The keys of the closures in memory, sorted (to debug and to compare with Haskell)
    pub fn keys(&self) -> Vec<String> {
        let mut ks: Vec<String> = self.map.lock().unwrap().keys().cloned().collect();
        ks.sort();
        ks
    }

    /// The policy asks a `job` to write before going on
    pub fn flush_on_job(&self) -> bool {
        self.policy.lock().unwrap().flush_on_job
    }

    /// How many closures have been serialised and written to disk so far
    pub fn serialised(&self) -> usize {
        self.serialised.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How many closures are in memory
    pub fn cached(&self) -> usize {
        self.map.lock().unwrap().len()
    }
}

impl Drop for Closures {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Written to a temporary file and renamed: a node killed while writing leaves the previous version
fn write_atomically(file: &std::path::Path, text: &str) {
    let tmp = file.with_file_name(format!(".{}.tmp", file.file_name().unwrap().to_string_lossy()));
    std::fs::write(&tmp, text).expect("write closure");
    std::fs::rename(tmp, file).expect("write closure");
}

/// The closure table of the node; created the first time
pub(crate) fn closures(ctx: &mut Ctx) -> Arc<Closures> {
    if let Some(c) = ctx.get::<Arc<Closures>>() {
        return (*c).clone();
    }
    let c = Arc::new(Closures::default());
    ctx.set(c.clone());
    c
}

pub fn get_closures() -> Trans<Arc<Closures>> {
    Trans::new(|ctx| Some(closures(ctx)))
}

/// Leaves the log as what has to be replayed: the first fragment is parsed and the rest waits for the next
/// continuation points (`setLogFromFile`). The paths of a `Collect` are replayed one after the other, the oldest
/// first, each one in this thread (`firstFragment`).
fn set_log_from_file(logs: Vec<Fragment>) -> Trans<()> {
    let Some((first, rest)) = logs.split_first() else { return pure(()) };
    let rest = rest.to_vec();
    let paths = match first {
        Fragment::Text(s) => vec![s.clone()],
        Fragment::Collect(ps) => ps.iter().rev().cloned().collect(),
    };
    tdo! {
        [rest]
        let path = for_(paths);
        Trans::new(move |ctx| {
            set_parse_string(&path).run(ctx);
            ctx.log.recover = true;
            ctx.log.rest_log = rest.clone();
            Some(())
        })
    }
}

/// What the reader of a continuation point does when it receives a message (`receive1`)
fn receive1(ev: EVar<Message>, key: String) -> Trans<()> {
    tdo! {
        [ev, key]
        let m = read_evar(&ev);
        take_message(ev, key, m)
    }
}

/// What the continuation point does with the message that it got: the connection, the closure to respond to and the headers
/// go to the state, and the log of the message is what is replayed
fn take_message(ev: EVar<Message>, key: String, m: Message) -> Trans<()> {
    let logs = m.logs.clone();
    let prelude = Trans::new(move |ctx| {
        if m.last {
            ev.clean();
            closures(ctx).update(&key, |lc| lc.evar = None);
        }
        if let Some(c) = &m.conn {
            let own = ctx.get::<Connection>().expect("receive1: no connection");
            ctx.set(Connection { calling: own.calling, ..c.clone() });
        }
        if let Some(r) = &m.to_respond {
            ctx.set(r.clone());
        }
        if let Some(h) = &m.headers {
            ctx.set(h.clone());
        }
        // a closure within an applicative term can not be the last one: the other term would not be resolved
        if m.from_remote {
            let app = ctx.get::<PrevClos>().expect("receive1").is_applicativ;
            if !app {
                let conn_id = ctx.get::<Connection>().expect("receive1: no connection").id;
                set_last_closure_for_connection(conn_id, k_local_clos(&m.session, &m.clos)).run(ctx);
            }
        }
        Some(())
    });
    tdo! {
        [logs]
        prelude.clone();
        set_log_from_file(logs.clone())
    }
}

/// Stores the continuation under `name` (or the hash of the path) and waits for messages. In normal execution the
/// computation ends here (`empty`); in a replay it injects the next fragment of the log. To go on in normal
/// execution, use `endpoint_here`.
pub fn endpoint(name: Option<String>) -> Trans<()> {
    Trans::new(move |ctx| {
        let log = ctx.log.clone();
        let clos_name = name.clone().unwrap_or_else(|| log.hash.to_string());
        let prev = ctx.get::<PrevClos>().expect("endpoint: use first_cont to start distributed computing");
        let session = prev.new_session.clone().unwrap_or_else(|| get_sess_closure(&prev.prev).0);
        let key = k_local_clos(&session, &clos_name);
        ctx.log = Log { part_log: LogB::default(), ..log.clone() };

        let table = closures(ctx);
        let existing = table.read(&key);
        let closure = match existing {
            Some(lc) => {
                let closure = if log.part_log.is_null() {
                    lc.clone()
                } else {
                    // in a `collectc` every visit adds its path to those of the closure
                    let (local_log, collected) = if log.recover {
                        (lc.local_log.clone(), lc.collected.clone())
                    } else if prev.is_collect {
                        (log.part_log.clone(), Some(std::iter::once(lc.local_log.clone()).chain(lc.collected.clone().into_iter().flatten()).collect()))
                    } else {
                        (log.part_log.clone(), None)
                    };
                    LocalClosure { evar: Some(EVar::new()), live: true, local_log, collected, ..lc.clone() }
                };
                ctx.set(PrevClos { prev: key.clone(), new_session: None, preserve_path: true, ..(*prev).clone() });
                // a replay that returns to a point whose continuation is still alive does not replace it: the messages
                // for the point must reach the continuation of the execution that created it
                let stored = if log.recover && lc.live { LocalClosure { evar: lc.evar.clone(), live: lc.live, ..closure.clone() } } else { closure.clone() };
                table.write(key.clone(), stored);
                closure
            }
            None => {
                // the first teleport of a `run_at` in the called side: remember who called, to respond after a restart
                let caller = ctx.get::<Connection>().and_then(|c| {
                    let is_self = matches!(*c.kind.lock().unwrap(), Some(ConnKind::Self_) | None);
                    if log.recover && !c.calling && !is_self { c.remote_node.lock().unwrap().clone() } else { None }
                });
                let closure = LocalClosure {
                    session: session.clone(),
                    clos: clos_name.clone(),
                    prev: prev.prev.clone(),
                    local_log: log.part_log.clone(),
                    collected: None,
                    source: None,
                    evar: Some(EVar::new()),
                    live: true,
                    caller,
                };
                ctx.set(PrevClos { prev: key.clone(), new_session: None, preserve_path: true, ..(*prev).clone() });
                table.write(key.clone(), closure.clone());
                closure
            }
        };

        let inject_next = inject_next_segment(log.clone(), closure.clone());
        or(receive1(closure.evar.clone().unwrap(), key.clone()), inject_next).run(ctx)
    })
}

/// `endpoint` followed by "go on only in normal execution": in a replay the computation stops here, because it
/// goes on through the message that it injects
pub fn endpoint_here() -> Trans<()> {
    Trans::new(|ctx| {
        let recover = ctx.log.recover;
        or(endpoint(None), Trans::new(move |_| (!recover).then_some(()))).run(ctx)
    })
}

fn inject_next_segment(log: Log, closure: LocalClosure) -> Trans<()> {
    Trans::new(move |ctx| {
        let first = log.rest_log.first()?;
        let isw = matches!(first, Fragment::Text(s) if s.starts_with("w/"));
        if log.recover && !isw {
            let msg = Message {
                last: false,
                logs: log.rest_log.clone(),
                session: closure.session.clone(),
                clos: closure.clos.clone(),
                from_remote: false,
                conn: None,
                to_respond: None,
                headers: None,
            };
            closure.evar.clone().unwrap().write(msg);
            ctx.log.part_log = LogB::default();
            ctx.log.rest_log = log.rest_log[1..].to_vec();
        }
        // leave_to_alternative
        if log.recover {
            if !isw {
                return None;
            }
            let mode = ctx.mode;
            set_parse_string(&first.as_text()[2..]).run(ctx);
            ctx.mode = mode;
            // in an applicative, the hash advances as the execution that wrote the "w/" did (see `add_wait` in `logged`): the
            // statement that was skipped is not replayed, so its step must be applied here
            let hash = if ctx.is_applicative { hash_step(log.hash, HASH_TERM) } else { ctx.log.hash };
            ctx.log = Log { recover: true, part_log: LogB::default(), rest_log: log.rest_log[1..].to_vec(), hash, ..ctx.log.clone() };
        } else {
            ctx.log = Log {
                part_log: LogB::from("w/"),
                rest_log: log.rest_log[1..].to_vec(),
                hash: hash_step(log.hash, HASH_WAIT),
                ..log.clone()
            };
        }
        None
    })
}

/// The first continuation point of a distributed program: closure 0 (`firstCont`, `firstEndpoint`)
pub fn first_cont() -> Trans<()> {
    Trans::new(|ctx| {
        ctx.set(PrevClos::default());
        let ev = EVar::new();
        let log = ctx.log.clone();
        closures(ctx).write(
            CLOS0.into(),
            LocalClosure {
                session: "0".into(),
                clos: "0".into(),
                prev: CLOS0.into(),
                local_log: log.part_log,
                collected: None,
                source: None,
                evar: Some(ev.clone()),
                live: true,
                caller: None,
            },
        );
        receive1(ev, CLOS0.into()).run(ctx)
    })
}

/// Sends a log to a live continuation point (`sendToClosure`)
pub fn send_to_closure(table: &Closures, session: &str, clos: &str, logs: Vec<Fragment>, last: bool) {
    let key = k_local_clos(session, clos);
    let lc = table.read(&key).unwrap_or_else(|| panic!("request received for non existent closure {key}"));
    let ev = lc.evar.unwrap_or_else(|| panic!("no EVar in closure {key}"));
    ev.write(Message { last, logs, session: session.into(), clos: clos.into(), from_remote: true, conn: None, to_respond: None, headers: None });
}

/// The last closure reached in a connection, to route the next messages (`setLastClosureForConnection`)
pub fn set_last_closure_for_connection(conn_id: usize, key: String) -> Trans<()> {
    set_index_rdata(conn_id, key)
}

pub fn get_last_closure_for_connection(conn_id: usize) -> Trans<String> {
    Trans::new(move |ctx| get_index_rdata::<usize, String>(conn_id).run(ctx)?)
}

/// The fragments of the path from `remclos` (not included) to `prevclos`, which are sent to the other node so that
/// it rebuilds the state (`getClosureLogFrom`)
pub fn get_closure_log_from(table: &Closures, prevclos: &str, remclos: &str) -> Vec<Fragment> {
    if prevclos == CLOS0 || prevclos == remclos {
        return Vec::new();
    }
    let reg = table.read(prevclos).unwrap_or_else(|| panic!("closure not found in DB: {prevclos}"));
    let mut log = get_closure_log_from(table, &reg.prev, remclos);
    log.push(reg.fragment());
    log
}

/// The log needed to reach a closure from the nearest ancestor whose continuation is alive, and that ancestor
/// (`getClosureLog`). A closure that was alive is its own ancestor.
pub fn get_closure_log(table: &Closures, session: &str, clos: &str) -> (Vec<Fragment>, LocalClosure) {
    let key = k_local_clos(session, clos);
    let reg = table.read(&key).unwrap_or_else(|| panic!("closure not found in DB: {key}"));
    if reg.live {
        return (Vec::new(), reg);
    }
    let (psession, pclos) = get_sess_closure(&reg.prev);
    let (mut log, ancestor) = get_closure_log(table, &psession, &pclos);
    log.push(reg.fragment());
    (log, ancestor)
}
