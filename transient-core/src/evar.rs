//! Event variables (`EVar`) and mailboxes: publish and subscribe without loops or blocking.
//!
//! The behaviour of the original (`Core.hs`) has been copied and checked by running it:
//!
//! * `write` is **synchronous**: it calls the readers one after another, in the thread of the writer, and does not return until
//!   all have finished, except those that go on in their own thread with `abduce`. The readers are called **from the last to
//!   the first** (LIFO).
//! * `read_evar` keeps alive the thread that called it until it is removed (`del_read_evar`, `clean`, `last_write`);
//!   meanwhile a `collect` or a `keep_all` do not end. `read_evar1` reads a single value and keeps nothing.
//! * There is no queue: a value written when there are no readers is lost.
//! * Mailboxes are application-wide and are identified by a key and by the type of the data. Each one is an
//!   `EVar` in a global registry; `put_mailbox` can be called from any thread, also from outside Transient.

use super::*;
use react::run_callback;

struct Sub<A> {
    id: usize,
    cb: Arc<dyn Fn(A) + Send + Sync>,
    /// the node that waits for this reader (only if it is permanent): while it exists, that node stays alive
    listener: Option<Arc<TNode>>,
}

/// An event variable: publishers and readers without coupling
pub struct EVar<A>(Arc<Mutex<Vec<Sub<A>>>>);

impl<A> Clone for EVar<A> {
    fn clone(&self) -> Self {
        EVar(self.0.clone())
    }
}

/// The identifier of the last subscription of this thread, for `del_read_evar`
#[derive(Clone)]
struct EVarId(usize);

fn release<A>(subs: Vec<Sub<A>>) {
    // outside the lock of the EVar: the node can become quiet and run hooks
    subs.iter().filter_map(|s| s.listener.as_ref()).for_each(|n| n.remove_listener());
}

impl<A> EVar<A> {
    pub fn new() -> EVar<A> {
        EVar(Arc::new(Mutex::new(Vec::new())))
    }

    /// Removes the reader with that identifier (`delReadEVarPoint`)
    pub fn del_read_point(&self, id: usize) {
        let removed = {
            let mut subs = self.0.lock().unwrap();
            let (removed, kept): (Vec<_>, Vec<_>) = subs.drain(..).partition(|s| s.id == id);
            *subs = kept;
            removed
        };
        release(removed);
    }

    /// Removes all the readers (`cleanEVar`)
    pub fn clean(&self) {
        let removed = std::mem::take(&mut *self.0.lock().unwrap());
        release(removed);
    }
}

impl<A> Default for EVar<A> {
    fn default() -> Self {
        EVar::new()
    }
}

impl<A: Clone> EVar<A> {
    /// Publishes `x`. The readers run now, in this thread, from the last subscribed to the first (`writeEVar`)
    pub fn write(&self, x: A) {
        let readers: Vec<_> = self.0.lock().unwrap().iter().map(|s| s.cb.clone()).collect();
        readers.into_iter().for_each(|cb| cb(x.clone()));
    }

    /// Publishes `x` and removes all the readers (`lastWriteEVar`)
    pub fn last_write(&self, x: A) {
        self.write(x);
        self.clean();
    }
}

/// Creates a new `EVar` each time it runs (`newEVar`)
pub fn new_evar<A: Send + 'static>() -> Trans<EVar<A>> {
    Trans::new(|_| Some(EVar::new()))
}

/// Subscribes to an `EVar`: the rest of the computation goes on, with each value that is written, in the thread of the one
/// that writes. The thread that called stays waiting while the subscription exists (`readEVar`).
pub fn read_evar<A: Send + 'static>(ev: &EVar<A>) -> Trans<A> {
    let ev = ev.clone();
    Trans::new(move |ctx| read_evar_with(true, gen_new_id(), &ev).run(ctx))
}

/// Reads a single value: the subscription is removed when the continuation starts to run (`readEVar1`)
pub fn read_evar1<A: Send + 'static>(ev: &EVar<A>) -> Trans<A> {
    let ev = ev.clone();
    Trans::new(move |ctx| read_evar_with(false, gen_new_id(), &ev).run(ctx))
}

/// A permanent subscription with an identifier, which can later be removed with `del_read_point` (`readEVarId`)
pub fn read_evar_id<A: Send + 'static>(id: usize, ev: &EVar<A>) -> Trans<A> {
    read_evar_with(true, id, ev)
}

/// A subscription with an identifier; `keep` says whether it is permanent or of a single value (`readEVar'`)
pub fn read_evar_with<A: Send + 'static>(keep: bool, id: usize, ev: &EVar<A>) -> Trans<A> {
    let ev = ev.clone();
    let subscribe = {
        let ev = ev.clone();
        Trans::new(move |ctx| {
            ctx.set(EVarId(id));
            if ctx.mode != Mode::Remote {
                ctx.mode = Mode::Parallel;
            }
            let cont = ctx.clone();
            let listener = keep.then(|| {
                ctx.node.add_listener();
                ctx.node.clone()
            });
            let cb: Arc<dyn Fn(A) + Send + Sync> = Arc::new(move |x| run_callback(&cont, x));
            // the last subscribed, the first of the list
            ev.0.lock().unwrap().insert(0, Sub { id, cb, listener });
            None
        })
    };
    if keep {
        subscribe
    } else {
        // `when (not keep) $ delReadEVarPoint ev id`: it is removed when the continuation starts, before the rest
        tdo! {
            [ev]
            let x = subscribe.map(Taken::new);
            Trans::new(move |_| {
                ev.del_read_point(id);
                x.take()
            })
        }
    }
}

/// Removes the last reader that this thread subscribed (`delReadEVar`). It goes after a `read_evar`.
pub fn del_read_evar<A: Send + 'static>(ev: &EVar<A>) -> Trans<()> {
    let ev = ev.clone();
    Trans::new(move |ctx| {
        let EVarId(id) = (*ctx.get::<EVarId>().expect("del_read_evar should be after a read_evar in the flow")).clone();
        ev.del_read_point(id);
        Some(())
    })
}

// ---- mailboxes

struct Mailbox {
    key_type: TypeId,
    key: Box<dyn Any + Send + Sync>,
    val_type: TypeId,
    evar: Arc<dyn Any + Send + Sync>,
}

impl Mailbox {
    fn is<K: Eq + Send + Sync + 'static, V: 'static>(&self, key: &K) -> bool {
        self.key_type == TypeId::of::<K>()
            && self.val_type == TypeId::of::<V>()
            && self.key.downcast_ref::<K>().is_some_and(|k| k == key)
    }
}

/// The application-wide registry (`mailboxes`)
static MAILBOXES: Mutex<Vec<Mailbox>> = Mutex::new(Vec::new());

/// The key of the default mailbox, like Haskell's `0 :: Int`
const DEFAULT_KEY: i32 = 0;

fn mailbox_entry<K: Eq + Send + Sync + 'static, V: Send + 'static>(key: K, evar: EVar<V>) -> Mailbox {
    Mailbox { key_type: TypeId::of::<K>(), key: Box::new(key), val_type: TypeId::of::<V>(), evar: Arc::new(evar) }
}

fn find_mailbox<K: Eq + Send + Sync + 'static, V: Send + 'static>(key: &K) -> Option<EVar<V>> {
    let boxes = MAILBOXES.lock().unwrap();
    let m = boxes.iter().find(|m| m.is::<K, V>(key))?;
    Some((*m.evar.clone().downcast::<EVar<V>>().unwrap()).clone())
}

fn remove_mailbox<K: Eq + Send + Sync + 'static, V: 'static>(key: &K) {
    MAILBOXES.lock().unwrap().retain(|m| !m.is::<K, V>(key));
}

/// Creates the mailbox of that key and that type; if it already existed it replaces it (`newMailbox`)
pub fn new_mailbox_key<K: Eq + Send + Sync + 'static, V: Send + 'static>(key: K) {
    let mut boxes = MAILBOXES.lock().unwrap();
    boxes.retain(|m| !m.is::<K, V>(&key));
    boxes.push(mailbox_entry(key, EVar::<V>::new()));
}

/// The `EVar` of a mailbox, which is created if it does not exist. It allows writing directly without looking up the mailbox on every
/// event (`getEVarFromMailbox'`).
pub fn get_evar_from_mailbox_key<K: Eq + Send + Sync + 'static, V: Send + 'static>(key: K) -> EVar<V> {
    let mut boxes = MAILBOXES.lock().unwrap();
    if let Some(m) = boxes.iter().find(|m| m.is::<K, V>(&key)) {
        return (*m.evar.clone().downcast::<EVar<V>>().unwrap()).clone();
    }
    let ev = EVar::<V>::new();
    boxes.push(mailbox_entry(key, ev.clone()));
    ev
}

/// The `EVar` of the default mailbox of that type (`getEVarFromMailbox`)
pub fn get_evar_from_mailbox<V: Send + 'static>() -> EVar<V> {
    get_evar_from_mailbox_key(DEFAULT_KEY)
}

/// Writes in the mailbox of that key and that type. Those that read it go on now, in this thread (`putMailbox'`).
/// A value written when nobody reads is lost.
pub fn put_mailbox_key<K: Eq + Send + Sync + 'static, V: Clone + Send + 'static>(key: K, x: V) {
    get_evar_from_mailbox_key::<K, V>(key).write(x);
}

/// Writes in the default mailbox of that type (`putMailbox`). It can be called from any thread, also
/// from an external callback.
pub fn put_mailbox<V: Clone + Send + 'static>(x: V) {
    put_mailbox_key(DEFAULT_KEY, x);
}

/// Reads from the mailbox of that key and that type: each message continues the computation (`getMailbox'`). It stays active until
/// `delete_mailbox`, `last_put_mailbox` or `delete_this_mailbox_reader`, so `collect` and `keep_all` do not end
/// before.
pub fn get_mailbox_key<K, V>(key: K) -> Trans<V>
where
    K: Clone + Eq + Send + Sync + 'static,
    V: Send + 'static,
{
    Trans::new(move |ctx| read_evar(&get_evar_from_mailbox_key::<K, V>(key.clone())).run(ctx))
}

/// Reads from the default mailbox of that type (`getMailbox`)
pub fn get_mailbox<V: Send + 'static>() -> Trans<V> {
    get_mailbox_key(DEFAULT_KEY)
}

/// Writes and removes all the readers; the mailbox is deleted after the data is read (`lastPutMailbox'`).
/// If the mailbox does not exist it does nothing.
pub fn last_put_mailbox_key<K: Eq + Send + Sync + 'static, V: Clone + Send + 'static>(key: K, x: V) {
    if let Some(ev) = find_mailbox::<K, V>(&key) {
        ev.last_write(x);
        remove_mailbox::<K, V>(&key);
    }
}

/// `last_put_mailbox_key` in the default mailbox (`lastPutMailbox`)
pub fn last_put_mailbox<V: Clone + Send + 'static>(x: V) {
    last_put_mailbox_key(DEFAULT_KEY, x);
}

/// Removes all the readers of the mailbox of that key and that type and deletes it (`deleteMailbox'`)
pub fn delete_mailbox_key<K: Eq + Send + Sync + 'static, V: Send + 'static>(key: K) {
    if let Some(ev) = find_mailbox::<K, V>(&key) {
        ev.clean();
        remove_mailbox::<K, V>(&key);
    }
}

/// `delete_mailbox_key` in the default mailbox (`deleteMailbox`)
pub fn delete_mailbox<V: Send + 'static>() {
    delete_mailbox_key::<i32, V>(DEFAULT_KEY);
}

/// Removes only the last reader that this thread subscribed, without touching the others (`deleteThisMailboxReader'`)
pub fn delete_this_mailbox_reader_key<K, V>(key: K) -> Trans<()>
where
    K: Clone + Eq + Send + Sync + 'static,
    V: Send + 'static,
{
    Trans::new(move |ctx| match find_mailbox::<K, V>(&key) {
        Some(ev) => del_read_evar(&ev).run(ctx),
        None => Some(()),
    })
}

/// `delete_this_mailbox_reader_key` in the default mailbox (`deleteThisMailboxReader`)
pub fn delete_this_mailbox_reader<V: Send + 'static>() -> Trans<()> {
    delete_this_mailbox_reader_key::<i32, V>(DEFAULT_KEY)
}
