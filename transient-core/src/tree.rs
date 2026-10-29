//! The thread tree (`TranShip.parent` / `children` / `labelth` in Haskell).
//!
//! Each thread created by `abduce` hangs from a node. A node is *quiet* when its thread has finished, it waits for no
//! event (listeners of `option`, `input`...) and it has no children. When it becomes quiet its end hooks run
//! (Haskell's `Finish`: what `collect` and `keep` use) and it is released from its parent, which may become quiet
//! in turn. This is how `collect` knows that nothing is alive, without counting threads by hand.
//!
//! A thread can not be killed from outside: `kill_*` marks the nodes as cancelled and each thread checks it between
//! steps (`bind`, `run_stack`, the loops of `parallel`). A thread blocked inside an `io()` ends
//! when that one returns.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

type Hook = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct State {
    /// the thread of this node is running
    running: bool,
    /// callbacks waiting for an event: while there is one, the node stays alive
    listeners: usize,
    children: Vec<Arc<TNode>>,
    hooks: Vec<Hook>,
}

impl State {
    fn quiet(&self) -> bool {
        !self.running && self.listeners == 0 && self.children.is_empty()
    }
}

pub(crate) struct TNode {
    id: usize,
    /// labels for debugging (`labelState`)
    label: Mutex<String>,
    /// strong: a node keeps its ancestors alive. The cycle with `children` is broken when the child becomes quiet
    parent: Option<Arc<TNode>>,
    state: Mutex<State>,
    cancelled: AtomicBool,
}

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

impl TNode {
    fn new(parent: Option<&Arc<TNode>>, running: bool) -> Arc<TNode> {
        let n = Arc::new(TNode {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            label: Mutex::default(),
            parent: parent.cloned(),
            state: Mutex::new(State { running, ..State::default() }),
            cancelled: AtomicBool::new(parent.is_some_and(|p| p.cancelled())),
        });
        if let Some(p) = parent {
            p.state.lock().unwrap().children.push(n.clone());
        }
        n
    }

    /// The root of an execution, with its thread running
    pub(crate) fn root() -> Arc<TNode> {
        TNode::new(None, true)
    }

    /// A child of `parent`; `running` if a thread is going to run in it (otherwise it only groups)
    pub(crate) fn child(parent: &Arc<TNode>, running: bool) -> Arc<TNode> {
        TNode::new(Some(parent), running)
    }

    /// A free thread (`free_threads`): it hangs from nobody, so nobody waits for it or can kill it
    pub(crate) fn detached() -> Arc<TNode> {
        TNode::new(None, true)
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// The thread of this node has stopped running (it has not been released from the tree yet)
    pub(crate) fn stop_running(&self) {
        self.state.lock().unwrap().running = false;
    }

    pub(crate) fn has_no_children(&self) -> bool {
        self.state.lock().unwrap().children.is_empty()
    }

    /// Neither this node nor anything that hangs from it runs a thread or waits for an event (`hasNoAliveThreads`)
    pub(crate) fn subtree_quiet(&self) -> bool {
        let children = {
            let s = self.state.lock().unwrap();
            if s.running || s.listeners > 0 {
                return false;
            }
            s.children.clone()
        };
        children.iter().all(|c| c.subtree_quiet())
    }

    /// The thread that was running in `self` now goes on in `to` (`one_thread`)
    pub(crate) fn transfer(&self, to: &TNode) {
        self.state.lock().unwrap().running = false;
        to.state.lock().unwrap().running = true;
    }

    pub(crate) fn add_listener(&self) {
        self.state.lock().unwrap().listeners += 1;
    }

    pub(crate) fn remove_listener(self: &Arc<Self>) {
        if self.cancelled() {
            return; // when cancelling they were already removed
        }
        self.state.lock().unwrap().listeners -= 1;
        settle(self);
    }

    /// A hook that runs once, when the node becomes quiet
    pub(crate) fn on_idle(&self, hook: impl FnOnce() + Send + 'static) {
        self.state.lock().unwrap().hooks.push(Box::new(hook));
    }

    /// No thread of this node or of what hangs from it is running; listeners do not count
    pub(crate) fn subtree_stopped(&self) -> bool {
        let children = {
            let s = self.state.lock().unwrap();
            if s.running {
                return false;
            }
            s.children.clone()
        };
        children.iter().all(|c| c.subtree_stopped())
    }

    pub(crate) fn add_label(&self, l: &str) {
        let mut label = self.label.lock().unwrap();
        label.push(',');
        label.push_str(l);
    }

    fn children(&self) -> Vec<Arc<TNode>> {
        self.state.lock().unwrap().children.clone()
    }

    /// Marks the node and everything that hangs from it as cancelled, children first
    fn mark(self: &Arc<Self>, out: &mut Vec<Arc<TNode>>) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.state.lock().unwrap().listeners = 0;
        for c in self.children() {
            c.mark(out);
        }
        out.push(self.clone());
    }

    fn kill_all(nodes: Vec<Arc<TNode>>) {
        let mut out = Vec::new();
        nodes.iter().for_each(|n| n.mark(&mut out));
        out.iter().for_each(settle);
    }

    /// Cancels everything that hangs from this node, but not the node
    pub(crate) fn kill_children(&self) {
        TNode::kill_all(self.children());
    }

    /// Cancels this node and everything that hangs from it
    pub(crate) fn kill_branch(self: &Arc<Self>) {
        TNode::kill_all(vec![self.clone()]);
    }

    /// `one_thread`: cancels what hangs from this node except the path to `current`. They are the branches that
    /// were born from the previous results; the path is the producer and the result being processed.
    pub(crate) fn kill_all_but_path_to(self: &Arc<Self>, current: &Arc<TNode>) {
        let mut path = vec![current.clone()];
        while !Arc::ptr_eq(path.last().unwrap(), self) {
            match path.last().unwrap().parent.clone() {
                Some(p) => path.push(p),
                None => return, // `current` does not hang from this node
            }
        }
        let on_path = |n: &Arc<TNode>| path.iter().any(|p| Arc::ptr_eq(p, n));
        let off_path: Vec<_> = path.iter().skip(1).flat_map(|p| p.children()).filter(|c| !on_path(c)).collect();
        TNode::kill_all(off_path);
    }

    /// The tree that hangs from the root, for debugging (`ps` in Haskell's console)
    pub(crate) fn show_tree(self: &Arc<Self>) -> String {
        let mut root = self.clone();
        while let Some(p) = root.parent.clone() {
            root = p;
        }
        let mut out = String::from("------Threads-------\n");
        root.show(0, self, &mut out);
        out
    }

    fn show(&self, depth: usize, me: &Arc<TNode>, out: &mut String) {
        let s = self.state.lock().unwrap();
        let status = if s.running { "running" } else if s.listeners > 0 { "listener" } else { "dead" };
        let cancelled = if self.cancelled() { " cancelled" } else { "" };
        let here = if std::ptr::eq(self, Arc::as_ptr(me)) { " <--" } else { "" };
        let label = self.label.lock().unwrap().clone();
        out.push_str(&format!("{}#{} {status}{cancelled}{here}{label}\n", "  ".repeat(depth), self.id));
        let children = s.children.clone();
        drop(s);
        children.iter().for_each(|c| c.show(depth + 1, me, out));
    }
}

/// The node may have become quiet: it runs its hooks and is released from the parent, and so on upwards
pub(crate) fn settle(node: &Arc<TNode>) {
    let mut cur = node.clone();
    loop {
        let hooks = {
            let mut s = cur.state.lock().unwrap();
            if !s.quiet() {
                return;
            }
            std::mem::take(&mut s.hooks)
        };
        hooks.into_iter().for_each(|h| h());
        // a hook can give new life to the node (the continuation of `collect` hangs from it)
        if !cur.state.lock().unwrap().quiet() {
            return;
        }
        let Some(parent) = cur.parent.clone() else { return };
        parent.state.lock().unwrap().children.retain(|c| !Arc::ptr_eq(c, &cur));
        cur = parent;
    }
}

/// A node of the thread tree of the computation in progress
#[derive(Clone)]
pub struct ThreadHandle(pub(crate) Arc<TNode>);

impl ThreadHandle {
    /// Cancels everything that was born from this thread, without cancelling it
    pub fn kill_children(&self) {
        self.0.kill_children();
    }
    /// Cancels this thread and everything that was born from it
    pub fn kill_branch(&self) {
        self.0.kill_branch();
    }
    /// The thread tree as text
    pub fn show_threads(&self) -> String {
        self.0.show_tree()
    }
}

/// The node of the current thread (`st <- get` in Haskell), to cancel branches with `kill_children` / `kill_branch`
pub fn thread_handle() -> Trans<ThreadHandle> {
    Trans::new(|ctx| Some(ThreadHandle(ctx.node.clone())))
}

/// For a stream `t`: each new result cancels the branches that were born from the previous results
/// (the thread of the producer and that of the current result go on). It is Haskell's `oneThread`, fixed in the same
/// session so that they match (before it killed the producer of a `waitEvents` stream and cancelled nothing with `option`).
///
/// Warning: after `one_thread` the thread stays hanging from the new node, and so does whatever it launches later (an alternative `|`,
/// for example). Everything that is not on the path of the current result is cancelled by the next result. What
/// must survive is launched with `free_threads`.
pub fn one_thread<A: Send + 'static>(t: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let n = TNode::child(&ctx.node, false);
        ctx.node.transfer(&n);
        ctx.node = n.clone();
        let n = n.clone();
        tdo! {
            [n]
            let x = t.clone().map(Taken::new);
            Trans::new(move |ctx: &mut Ctx| {
                n.kill_all_but_path_to(&ctx.node);
                x.take()
            })
        }
        .run(ctx)
    })
}

/// The threads born inside `t` hang from nobody: they are not waited for and can not be cancelled. The rest, including what
/// runs in those threads, is tracked again (`freeThreads` is `t <*** restore`).
pub fn free_threads<A: Send + 'static>(t: Trans<A>) -> Trans<A> {
    Trans::new(move |ctx| {
        let old = std::mem::replace(&mut ctx.free, true);
        after_each(t.clone(), Arc::new(move |ctx: &mut Ctx| ctx.free = old)).run(ctx)
    })
}
