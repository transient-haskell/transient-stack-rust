//! Saving the state when `keep` ends (`saveonexit`, `save`, the signal handlers of `keep`). The period of the writes
//! is set to an hour so that only the explicit saves write: a node that dies by SIGTERM saves if it was asked to, and
//! one that dies by SIGKILL saves nothing.

mod common;
use common::*;
use std::path::Path;
use std::time::Duration;

const NEVER: &[(&str, &str)] = &[("TRANSIENT_SYNC_MS", "3600000")];

fn tmp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("transient-save-{name}-{}", std::process::id()))
}

/// The closures on disk, without the jobs directory and the temporary files
fn closures_on_disk(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir.join(".tcachedata"))
        .map(|rd| rd.filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n != "jobs" && !n.starts_with('.')).collect())
        .unwrap_or_default()
}

fn settle() {
    std::thread::sleep(Duration::from_millis(300));
}

/// Runs the program with an endpoint until it finishes
fn run_until_result(dir: &Path, port: u16) -> (Node, std::sync::mpsc::Receiver<String>) {
    let (node, rx) = spawn_with(dir, "endpoint", &path(port, "/go"), NEVER);
    wait_line(&rx, "RESULT");
    settle();
    (node, rx)
}

#[test]
fn a_signal_saves_the_state_if_saveonexit_was_asked() {
    let dir = tmp("signal");
    let (mut node, rx) = run_until_result(&dir, 9751);
    assert!(closures_on_disk(&dir).is_empty(), "nothing is written before");
    node.type_line("saveonexit");
    settle();
    node.signal("TERM");
    let status = node.wait_exit(10).expect("the node ends with the signal");
    assert!(status.success(), "keep returns and the process ends normally: {status:?}");
    assert!(closures_on_disk(&dir).len() >= 2, "{:?}", closures_on_disk(&dir));
    assert!(rx.try_iter().any(|l| l == "saved"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_signal_saves_nothing_if_it_was_not_asked() {
    let dir = tmp("nosave");
    let (mut node, _rx) = run_until_result(&dir, 9752);
    node.signal("TERM");
    node.wait_exit(10).expect("the node ends with the signal");
    assert!(closures_on_disk(&dir).is_empty(), "{:?}", closures_on_disk(&dir));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_kill_signal_saves_nothing_even_if_it_was_asked() {
    let dir = tmp("sigkill");
    let (mut node, _rx) = run_until_result(&dir, 9753);
    node.type_line("saveonexit");
    settle();
    node.signal("KILL");
    node.wait_exit(10).expect("the node dies");
    assert!(closures_on_disk(&dir).is_empty(), "{:?}", closures_on_disk(&dir));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_save_command_writes_now() {
    let dir = tmp("save");
    let (mut node, rx) = run_until_result(&dir, 9754);
    assert!(closures_on_disk(&dir).is_empty());
    node.type_line("save");
    wait_line(&rx, "Saved!!");
    assert!(closures_on_disk(&dir).len() >= 2, "{:?}", closures_on_disk(&dir));
    node.signal("KILL");
    node.wait_exit(10);
    let _ = std::fs::remove_dir_all(dir);
}

/// A job asks for the state to be saved on exit, as in Haskell, without the need to type `saveonexit`: the called node,
/// whose body has a job, is told to end by a signal during its wait, and its job and closures are on disk.
#[test]
fn a_job_saves_the_state_on_exit() {
    let dir = tmp("job");
    let (dir_a, dir_b) = (dir.join("a"), dir.join("b"));
    let (pa, pb) = (9755, 9756);
    let env = [("TRANSIENT_SYNC_MS", "3600000"), ("TRANSIENT_FLUSH_ON_JOB", "0")];
    let (mut b, rx_b) = spawn_with(&dir_b, "job", &path(pb, &add(pa)), &env);
    wait_line(&rx_b, "READY");
    let (_a, _rx_a) = spawn(&dir_a, "job", &path(pa, &format!("{}/go", add(pb))));
    wait_line(&rx_b, "N1 WORKING");
    assert!(closures_on_disk(&dir_b).is_empty(), "nothing written yet");
    b.signal("TERM");
    b.wait_exit(10).expect("the node ends with the signal");
    assert!(dir_b.join(".tcachedata").join("jobs").read_dir().unwrap().next().is_some(), "the job is on disk");
    assert!(closures_on_disk(&dir_b).len() >= 2, "{:?}", closures_on_disk(&dir_b));
    let _ = std::fs::remove_dir_all(dir);
}

/// `savepol` changes the period of the writes while the node runs, and asks to save when `keep` ends (as `saveonexit`)
#[test]
fn savepol_sets_the_period_and_the_saving_on_exit() {
    let dir = tmp("savepol");
    // the period of the environment is an hour: only `savepol` (1000 objects, every second) can make the node write
    let (mut node, rx) = spawn_with(&dir, "endpoint", &path(9757, "/go/savepol/1000/1"), NEVER);
    wait_line(&rx, "RESULT");
    assert!(closures_on_disk(&dir).is_empty(), "nothing written yet");
    wait_line(&rx, "- Max objects: 1000");
    std::thread::sleep(Duration::from_millis(1700));
    assert!(closures_on_disk(&dir).len() >= 2, "written after the period of savepol: {:?}", closures_on_disk(&dir));
    node.signal("TERM");
    node.wait_exit(10).expect("the node ends with the signal");
    assert!(rx.try_iter().any(|l| l == "saved"), "the state is saved when keep ends");
    let _ = std::fs::remove_dir_all(dir);
}
