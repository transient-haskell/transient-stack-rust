//! Saving the state on exit (`saveonexit`, `save` and the signal handlers of `keep` in `Transient.Console`).
//!
//! In Haskell a global flag `save` says that the state has to be committed (`syncCache`) when `keep` ends. The flag is
//! set by the console option `saveonexit`, by `savepol` and by every `job`. `keep` ends when the program does `exit`,
//! when the console gets `end`, or when the process gets SIGINT or SIGTERM (the handlers do `exitLeft`), so a node
//! killed by a signal saves its state before dying. A process killed by SIGKILL, or a crash, saves nothing.
//!
//! Here the flag and the tables to sync are also global (one TCache per process in Haskell). A node with persistent
//! closures registers its table and starts to watch the signals.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Weak;

static SAVE: AtomicBool = AtomicBool::new(false);

fn tables() -> &'static Mutex<Vec<Weak<Closures>>> {
    static T: std::sync::OnceLock<Mutex<Vec<Weak<Closures>>>> = std::sync::OnceLock::new();
    T.get_or_init(Default::default)
}

/// The `keep` that is running, which a signal ends
fn current_exit() -> &'static Mutex<Option<Arc<ExitSlot>>> {
    static E: std::sync::OnceLock<Mutex<Option<Arc<ExitSlot>>>> = std::sync::OnceLock::new();
    E.get_or_init(Default::default)
}

/// The state is saved when `keep` ends (`saveonexit`)
pub fn save_on_exit() {
    SAVE.store(true, Ordering::SeqCst);
}

/// Changes the policy of the cache of all the persistent tables while the node runs: the period of the writes and the
/// number of closures that stay in memory. The state is saved when `keep` ends, too (`savepol` of the console)
pub fn save_policy(period: std::time::Duration, cache_size: usize) {
    let live: Vec<Arc<Closures>> = tables().lock().unwrap().iter().filter_map(Weak::upgrade).collect();
    live.iter().for_each(|t| t.set_policy(Some(period), cache_size));
    save_on_exit();
    println!("\n- syncing each {} seconds.\n- Max objects: {cache_size}\n", period.as_secs_f64());
    println!("- saving cache at the end of the 'keep' execution");
}

/// Saves now the state of all the persistent tables (`syncCache`)
pub fn sync_cache() {
    let live: Vec<Arc<Closures>> = tables().lock().unwrap().iter().filter_map(Weak::upgrade).collect();
    live.iter().for_each(|t| t.flush());
}

/// What `keep` does when it ends: if the flag is set, saves the state
pub(crate) fn save_at_exit_if_asked() {
    if SAVE.load(Ordering::SeqCst) {
        print!("\nsaving cache state...");
        sync_cache();
        println!("\nsaved\n");
    }
}

/// A persistent table joins the saving of the process, and the signals that end `keep` start to be watched
pub(crate) fn register(table: &Arc<Closures>, exit: Option<Arc<ExitSlot>>) {
    tables().lock().unwrap().push(Arc::downgrade(table));
    *current_exit().lock().unwrap() = exit;
    static WATCH: std::sync::Once = std::sync::Once::new();
    WATCH.call_once(|| {
        let mut signals = signal_hook::iterator::Signals::new([signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM]).expect("signals");
        thread::spawn(move || {
            for sig in signals.forever() {
                let msg = format!("signal: {sig} received");
                eprintln!("{msg}\n");
                match &*current_exit().lock().unwrap() {
                    // `exitLeft`: `keep` returns and saves if it has to
                    Some(slot) => slot.set(Err(msg)),
                    // no `keep` to end: save if asked and die as the signal would have made us
                    None => {
                        save_at_exit_if_asked();
                        std::process::exit(128 + sig);
                    }
                }
            }
        });
    });
}
