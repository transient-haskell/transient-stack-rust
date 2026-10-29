//! The console of `keep`: `option`, `option1` and `input` register a listener; each time the console receives what it
//! waits for, the rest of the computation runs in a new thread with the value received (like `react` in Haskell).
//!
//! The command line gives values to the options and inputs of the program, in two notations:
//!
//! * `--start localhost:8080 --cookie abc`: each `option` looks for `-name` or `--name` in the command line when it is
//!   executed, and gives to the console the arguments that follow it, up to the next one that starts with `-`, as if they had
//!   been typed. The inputs that the program executes after the option find their values there. The options can be in any
//!   order in the command line, and be executed at any time.
//! * `-p start/localhost/8080/add/localhost/8081/n/go` (the old notation) is the same as typing
//!   `start localhost 8080 add localhost 8081 n go` (the separators are `/`, `:`, `;` and spaces) when the program starts.
//!
//! The tokens are consumed by the options and inputs of the program as they are registered.
//!
//! Differences with `Console.hs`: input is by lines (no editing with ANSI keys) and options are identified by a string.

use super::*;
use std::io::{BufRead, Write};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Response of a listener to a fragment of the line
enum Take {
    No,
    /// consumed; if the listener is one-shot, it is removed
    Yes,
    /// consumed, but the listener goes on waiting (an `input` that could not read the fragment)
    Retry,
    /// not consumed, but the listener is satisfied with its default value: the fragment goes to the next listener
    /// (`input'` when the fragment is not what it expects)
    Default,
}

struct Listener {
    id: usize,
    ident: String,
    message: String,
    once: bool,
    /// the node of the thread tree that waits for this listener; while it exists, the node stays alive.
    /// Those of `keep` (`end`, `options`, `ps`) are in a node of their own: they do not count as pending activity
    node: Arc<TNode>,
    offer: Arc<dyn Fn(&str) -> Take + Send + Sync>,
}

pub(crate) struct Console {
    listeners: Mutex<Vec<Listener>>,
    next_id: AtomicUsize,
    version: AtomicUsize,
    closed: AtomicBool,
    /// the arguments of the command line, and the positions of those that have been given to their option
    args: Vec<String>,
    used: Mutex<Vec<usize>>,
    /// the arguments of the options that are executed together are given to the console one option after the other
    injecting: Mutex<()>,
    /// the program has returned control after registering its first listeners: from then on the console distributes tokens
    started: (Mutex<bool>, std::sync::Condvar),
    /// the options of the command line that are being given to the console
    injections: AtomicUsize,
}

impl Console {
    fn new(args: Vec<String>) -> Console {
        Console {
            listeners: Mutex::default(),
            next_id: AtomicUsize::new(0),
            version: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            args,
            used: Mutex::default(),
            injecting: Mutex::default(),
            started: (Mutex::new(false), std::sync::Condvar::new()),
            injections: AtomicUsize::new(0),
        }
    }

    /// The arguments of the option `name` in the command line: those that follow `-name` or `--name` up to the next one that
    /// starts with `-`. `None` if it is not there. Each occurrence is given once, in order, to the executions of the option.
    fn option_args(&self, name: &str) -> Option<Vec<String>> {
        let mut used = self.used.lock().unwrap();
        let i = self.args.iter().enumerate().position(|(i, a)| !used.contains(&i) && a.strip_prefix("--").or(a.strip_prefix('-')) == Some(name))?;
        used.push(i);
        // counted while the lock is held: `options_pending` never sees the option as neither pending nor being given
        self.injections.fetch_add(1, Ordering::SeqCst);
        Some(self.args[i + 1..].iter().take_while(|a| !a.starts_with('-')).cloned().collect())
    }

    /// Whether some option of the command line has not been given yet to its option, or is being given. The old notation
    /// (`-p`) is not one.
    fn options_pending(&self) -> bool {
        let used = self.used.lock().unwrap();
        self.injections.load(Ordering::SeqCst) > 0
            || self.args.iter().enumerate().any(|(i, a)| a.starts_with('-') && a != "-p" && a != "--path" && !used.contains(&i))
    }

    /// Gives to the console, as if they had been typed, the option `name` and its arguments from the command line, if they
    /// are there. It is done once the listener of the option is registered, in a thread of its own: the continuation of the option
    /// runs in it. An argument that is a URL or has spaces is a single value.
    fn inject_option(self: &Arc<Self>, name: &str) {
        let Some(params) = self.option_args(name) else { return };
        let toks: Vec<String> = std::iter::once(name.to_string())
            .chain(params.iter().flat_map(|p| {
                if p.contains("://") || p.contains(' ') {
                    vec![p.clone()]
                } else {
                    p.split(|c| "/: \t;".contains(c)).filter(|t| !t.is_empty()).map(String::from).collect()
                }
            }))
            .collect();
        let c = self.clone();
        thread::spawn(move || {
            let (flag, cv) = &c.started;
            drop(cv.wait_while(flag.lock().unwrap(), |started| !*started).unwrap());
            // the threads that the program forked keep registering their listeners and endpoints while the option runs: a
            // command line in the old notation gave them the same time (the wait of `take` for a token that nobody takes yet),
            // and the jobs that `exec` resumes must not run before the program has registered the endpoints they come back to
            thread::sleep(Duration::from_millis(100));
            let _one_at_a_time = c.injecting.lock().unwrap();
            toks.iter().for_each(|t| c.take(t));
            c.injections.fetch_sub(1, Ordering::SeqCst);
        });
    }

    /// Removes the listeners, notifying their node. It is called without the lock: the node can become quiet and
    /// run hooks that register other listeners
    fn release(removed: Vec<Listener>) {
        removed.iter().for_each(|l| l.node.remove_listener());
    }

    /// Registers a listener; another one with the same identifier is removed (there is only one active `input`).
    /// If the input is already closed, nothing can arrive and it is not registered.
    fn add(&self, node: &Arc<TNode>, ident: &str, message: &str, once: bool, offer: impl Fn(&str) -> Take + Send + Sync + 'static) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let removed = {
            let mut ls = self.listeners.lock().unwrap();
            let (removed, mut kept): (Vec<_>, Vec<_>) = ls.drain(..).partition(|l| l.ident == ident);
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            node.add_listener();
            kept.push(Listener {
                id,
                ident: ident.into(),
                message: message.into(),
                once,
                node: node.clone(),
                offer: Arc::new(offer),
            });
            *ls = kept;
            self.version.fetch_add(1, Ordering::SeqCst);
            removed
        };
        Console::release(removed);
    }

    /// The active options and inputs, with their messages (`options`)
    fn show(&self) {
        self.listeners.lock().unwrap().iter().for_each(|l| println!("{}\t{}", l.ident, l.message));
    }

    fn remove(&self, id: usize) {
        let removed = {
            let mut ls = self.listeners.lock().unwrap();
            let (removed, kept): (Vec<_>, Vec<_>) = ls.drain(..).partition(|l| l.id == id);
            *ls = kept;
            removed
        };
        Console::release(removed);
    }

    fn offer(&self, tok: &str) -> bool {
        // a listener is not called with the lock held: when fired it may register others. Those of a
        // cancelled branch no longer wait for anything (when cancelled they were removed from the node): they are discarded
        let offers: Vec<_> = {
            let mut ls = self.listeners.lock().unwrap();
            ls.retain(|l| !l.node.cancelled());
            ls.iter().map(|l| (l.id, l.once, l.offer.clone())).collect()
        };
        for (id, once, offer) in offers {
            match offer(tok) {
                Take::No => {}
                Take::Yes => {
                    if once {
                        self.remove(id);
                    }
                    return true;
                }
                Take::Retry => return true,
                Take::Default => {
                    if once {
                        self.remove(id);
                    }
                    // the listeners that the continuation registered see the same fragment
                    return self.offer(tok);
                }
            }
        }
        false
    }

    /// A fragment that nobody consumes is retried if a listener has been registered in the meantime
    /// (the one that waits for it may be registering); otherwise it is discarded
    fn take(&self, tok: &str) {
        loop {
            let v = self.version.load(Ordering::SeqCst);
            if self.offer(tok) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
            if self.version.load(Ordering::SeqCst) == v {
                eprintln!("{tok}: can't read, skip");
                return;
            }
        }
    }

    /// A token of the command line waits for whoever takes it, up to `patience`: the program may be busy (starting a node,
    /// say) before it executes the option or input that is for it. A token that nobody takes is reported and skipped.
    fn take_waiting(&self, tok: &str, patience: Duration) {
        let started = Instant::now();
        while !self.offer(tok) {
            if started.elapsed() > patience {
                eprintln!("{tok}: can't read, skip");
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// When the input is closed nothing that is waited for can arrive: the listeners stop keeping the tree alive
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let removed = std::mem::take(&mut *self.listeners.lock().unwrap());
        Console::release(removed);
    }

    /// Feeds the console with lines. `first` is a line to take before: the command line.
    fn run_input(&self, first: Option<String>, input: impl BufRead) {
        let lines = first.into_iter().map(|l| (true, l)).chain(input.lines().map_while(Result::ok).map(|l| (false, l)));
        for (from_command_line, line) in lines {
            let toks: Vec<&str> = line.split(|c| "/: \t;".contains(c)).filter(|t| !t.is_empty()).collect();
            if toks.is_empty() {
                // an empty line: the inputs that have a default take it
                self.offer("");
            }
            for tok in toks {
                if from_command_line {
                    self.take_waiting(tok, Duration::from_secs(5));
                } else {
                    self.take(tok);
                }
            }
        }
        // the options of the command line arrive when the program executes them, even if the input is already closed (nobody
        // types any more): the listeners stay until all of them have been given
        while self.options_pending() {
            thread::sleep(Duration::from_millis(50));
        }
        self.close();
    }
}

/// The options that `keep` offers, written with the same primitives as any program (the options of `keep` in `Console.hs`):
/// each one waits for its word in the console, does its job and ends (`empty`), so it is ready for the next time. The command
/// line can give them too: `--saveonexit`, `--end`, `--ps`.
fn core_options(root: Arc<TNode>, console: Arc<Console>) -> Trans<()> {
    let end = tdo! {
        option("end", "exit");
        exit_left("end");
        empty::<()>()
    };
    let options = tdo! {
        [console]
        option("options", "show the active options");
        lift_io({ let console = console.clone(); move || console.show() });
        empty::<()>()
    };
    let ps = tdo! {
        [root]
        option("ps", "show threads");
        lift_io({ let root = root.clone(); move || print!("{}", root.show_tree()) });
        empty::<()>()
    };
    let save = tdo! {
        option("save", "commit the current execution state to permanent storage now");
        lift_io(|| {
            sync_cache();
            println!("\nSaved!!");
        });
        empty::<()>()
    };
    let save_on_exit = tdo! {
        option("saveonexit", "commit the current execution state to permanent storage on exit");
        lift_io(save_on_exit);
        empty::<()>()
    };
    // `savepol [<max objects> [<seconds>]]`: the policy of the cache (the defaults are those of the original: 1000 and 10)
    let savepol = tdo! {
        option("savepol", "configure saving execution state");
        let max = input_default(1000usize, |_| true, "max number of cached objects?");
        let secs = input_default(10u64, |_| true, "seconds between check for objects to be saved?");
        lift_io(move || save_policy(Duration::from_secs(secs), max));
        empty::<()>()
    };
    end | options | ps | save | save_on_exit | savepol
}

fn console_of(ctx: &Ctx) -> Arc<Console> {
    ctx.console.clone().expect("no console: use keep")
}

fn exit_slot(ctx: &Ctx) -> Arc<ExitSlot> {
    ctx.exit.clone().expect("no Exit state: use keep")
}

/// Runs the rest of the computation, registered in `ctx`, with `x`: **in the console thread and one after another**, like
/// Haskell's `react` (an event does not start until the previous one has returned control, e.g. on an `abduce`)
fn fire<A: Send + 'static>(ctx: &Ctx, x: A) {
    react::run_callback(ctx, x);
}

/// Waits for the line `key` in the console and then goes on, in the console thread, each time it is typed
pub fn option(key: &str, message: &str) -> Trans<String> {
    option_with(key, message, false)
}

/// Like `option`, but it waits only once
pub fn option1(key: &str, message: &str) -> Trans<String> {
    option_with(key, message, true)
}

fn option_with(key: &str, message: &str, once: bool) -> Trans<String> {
    let (key, message) = (key.to_string(), message.to_string());
    Trans::new(move |ctx| {
        println!("Enter\t\x1b[1;31m{key}\x1b[0m\t\tto: {message}");
        let (c, k) = (ctx.clone(), key.clone());
        console_of(ctx).add(&ctx.node, &key, &message, once, move |tok| {
            if tok == k {
                fire(&c, k.clone());
                Take::Yes
            } else {
                Take::No
            }
        });
        console_of(ctx).inject_option(&key);
        None
    })
}

/// Waits for a value of type `A` in the console and goes on with it. There is only one active `input`: the next one
/// that is registered replaces the previous one. A fragment that does not read as `A` is rejected and it keeps waiting.
pub fn input<A: FromStr + Send + 'static>(prompt: &str) -> Trans<A> {
    let prompt = prompt.to_string();
    Trans::new(move |ctx| {
        print!("{prompt}");
        std::io::stdout().flush().unwrap();
        let (c, p) = (ctx.clone(), prompt.clone());
        console_of(ctx).add(&ctx.node, "input", &prompt, true, move |tok| match tok.parse::<A>() {
            _ if tok.is_empty() => Take::No,
            Ok(v) => {
                fire(&c, v);
                Take::Yes
            }
            Err(_) => {
                print!("{tok}: not valid. {p}");
                std::io::stdout().flush().unwrap();
                Take::Retry
            }
        });
        None
    })
}

/// Like `input`, with a default value (`input'`). An empty line gives the default. A fragment that does not read as `A`, or
/// that `accept` rejects, also gives the default, and it is not consumed: the next input sees it. In
/// `start/localhost/9001` the fragment `9001` is not "retry", so the question about retrying takes its default and the
/// port takes `9001`.
pub fn input_default<A: FromStr + Clone + Send + Sync + 'static>(default: A, accept: impl Fn(&A) -> bool + Send + Sync + 'static, prompt: &str) -> Trans<A> {
    let (prompt, accept) = (prompt.to_string(), Arc::new(accept));
    Trans::new(move |ctx| {
        print!("{prompt}");
        std::io::stdout().flush().unwrap();
        let (c, default, accept) = (ctx.clone(), default.clone(), accept.clone());
        console_of(ctx).add(&ctx.node, "input", &prompt, true, move |tok| {
            if tok.is_empty() {
                fire(&c, default.clone());
                return Take::Yes;
            }
            match tok.parse::<A>() {
                Ok(v) if accept(&v) => {
                    fire(&c, v);
                    Take::Yes
                }
                _ => {
                    fire(&c, default.clone());
                    Take::Default
                }
            }
        });
        None
    })
}

/// Ends `keep` with a result. The type of `x` has to be the one `keep` expects.
pub fn exit<A: Clone + Send + Sync + 'static>(x: A) -> Trans<()> {
    Trans::new(move |ctx| {
        exit_slot(ctx).set(Ok(Box::new(x.clone())));
        None
    })
}

/// Ends `keep` without a result: `keep` returns `Err(cause)`
pub fn exit_left(cause: &str) -> Trans<()> {
    let cause = cause.to_string();
    Trans::new(move |ctx| {
        exit_slot(ctx).set(Err(cause.clone()));
        None
    })
}

/// Runs `t` (its result is ignored) and serves the console until someone does `exit`, `end` is typed, or nothing else can happen
/// (no thread alive and no listener waiting): in that case it returns `Err("No threads left")`.
/// The type of the result `A` is chosen by the caller and has to be that of `exit`: `let r: Result<i32, _> = keep(..)`.
/// The fragments of the lines that are typed are distributed by `option` and `input`; `options` lists the active ones.
///
/// The command line is the first thing that the console reads: see `command_line`.
pub fn keep<A: 'static, B: Send + 'static>(t: Trans<B>) -> Result<A, String> {
    keep_args(&std::env::args().skip(1).collect::<Vec<_>>(), t)
}

/// `keep` with the arguments of the command line given (the program may have taken some of them: a program with several
/// modes would pass the rest)
pub fn keep_args<A: 'static, B: Send + 'static>(args: &[String], t: Trans<B>) -> Result<A, String> {
    keep_core(args.to_vec(), command_line(args), || std::io::stdin().lock(), t)
}

/// `keep_args` reading the console from whatever `input` opens instead of the standard input (for tests)
pub fn keep_args_with<A: 'static, B: Send + 'static, R: BufRead>(args: &[String], input: impl FnOnce() -> R + Send + 'static, t: Trans<B>) -> Result<A, String> {
    keep_core(args.to_vec(), command_line(args), input, t)
}

/// The command line in the old notation, as the console reads it when the program starts (`thereIsArgPath`): what follows
/// `-p` or `--path`, up to the next argument that starts with `-`, each one followed by `/`. All of these start a node at
/// `localhost` in the port 8080:
///
/// ```text
/// program -p start/localhost/8080
/// program --start localhost:8080
/// program --start localhost/8080
/// ```
///
/// but only the first one is read here. The others are not transformed: each option looks for itself in the command line when
/// it is executed (see `Console::inject_option`), so they work wherever the option is in the program and in the command line.
///
/// The original quotes an argument that has a `/` and does not start with `start` (to pass a URL as one value). That breaks
/// `--start localhost/8080` (the host becomes `"localhost`), so here only an argument that looks like a URL (`://`) or
/// has spaces is quoted.
pub fn command_line(args: &[String]) -> Option<String> {
    let mut path = String::new();
    for i in args.iter().enumerate().filter(|(_, a)| *a == "-p" || *a == "--path").map(|(i, _)| i) {
        for a in args[i + 1..].iter().take_while(|a| !a.starts_with('-')) {
            path.push_str(&if a.contains("://") || a.contains(' ') { format!("\"{a}\"/") } else { format!("{a}/") });
        }
    }
    (!path.is_empty()).then_some(path)
}

/// `keep` reading the console from whatever `input` opens (for tests and for programs that do not use standard input).
/// It is opened inside the console thread because `StdinLock` can not be sent to another thread.
pub fn keep_with<A: 'static, B: Send + 'static, R: BufRead>(
    input: impl FnOnce() -> R + Send + 'static,
    t: Trans<B>,
) -> Result<A, String> {
    keep_core(Vec::new(), None, input, t)
}

fn keep_core<A: 'static, B: Send + 'static, R: BufRead>(
    args: Vec<String>,
    command_line: Option<String>,
    input: impl FnOnce() -> R + Send + 'static,
    t: Trans<B>,
) -> Result<A, String> {
    let mut ctx = Ctx::new();
    let slot = Arc::new(ExitSlot::default());
    let console = Arc::new(Console::new(args));
    ctx.console = Some(console.clone());
    ctx.exit = Some(slot.clone());
    // when nothing is alive or waiting, `keep` ends
    let s = slot.clone();
    ctx.node.on_idle(move || s.set(Err("No threads left".into())));

    // the options of `keep` are options like any other, in a node of their own: their listeners do not keep the program alive
    let mut core = ctx.clone();
    core.node = TNode::detached();
    core_options(ctx.node.clone(), console.clone()).run(&mut core);

    // the console starts reading when `t` has registered its synchronous listeners; those registered
    // later are waited for by `take`
    let (started, wait_start) = std::sync::mpsc::channel();
    let started_console = console.clone();
    thread::spawn(move || {
        let mut ctx = ctx;
        let g = EndOfThread(&mut ctx);
        tdo! {
            [t]
            print_unhandled();
            t
        }
        .run(&mut *g.0);
        let _ = started.send(());
        let (flag, cv) = &started_console.started;
        *flag.lock().unwrap() = true;
        cv.notify_all();
    });
    let c = console.clone();
    thread::spawn(move || {
        let _ = wait_start.recv();
        if let Some(path) = &command_line {
            println!("Executing: {path:?}");
        }
        c.run_input(command_line, input());
    });

    let r = slot.wait();
    save_at_exit_if_asked();
    r.map(|d| *d.downcast::<A>().expect("exit of type not expected"))
}
