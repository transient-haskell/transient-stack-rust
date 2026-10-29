//! The applicative of `Cloud` with remote terms (AppWait.hs, AppThree.hs, AppRestart.hs of the Haskell battery): the
//! terms run in different nodes at once, and the computation goes on after them in the caller and in the nodes.
//! Processes: one caller and one node per term (`examples/node_demo.rs`).

mod common;
use common::*;
use std::path::PathBuf;
use std::time::Duration;

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("transient-app-{name}-{}", std::process::id()))
}

/// Runs the caller of an applicative of calls to `peers` (port, ms), after starting those nodes, and returns the lines of
/// the caller and of each peer up to the end of the program
fn run_app(name: &str, base: u16, delays: &[u64]) -> (Vec<String>, Vec<Vec<String>>) {
    let dir = tmp(name);
    let mode = format!("app:{}", delays.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(","));
    let mut peers = Vec::new();
    for i in 0..delays.len() {
        peers.push(spawn(&dir.join(format!("n{}", i + 1)), &mode, &path(base + 1 + i as u16, "")));
    }
    for (_, rx) in &peers {
        wait_line(rx, "READY");
    }
    // the caller knows the nodes of the terms, in the order of the terms, and gets `go`
    let nodes: String = (1..=delays.len() as u16).map(|i| add(base + i)).collect();
    let (_caller, rx) = spawn(&dir.join("n0"), &mode, &path(base, &format!("{nodes}/go")));
    let mut lines = Vec::new();
    loop {
        let l = wait_line(&rx, "");
        let end = l == "TEST END";
        lines.push(l);
        if end {
            break;
        }
    }
    std::thread::sleep(Duration::from_millis(300));
    let peer_lines = peers.iter().map(|(_, rx)| rx.try_iter().collect()).collect();
    let _ = std::fs::remove_dir_all(dir);
    (lines, peer_lines)
}

fn check(lines: &[String], peer_lines: &[Vec<String>], base: u16) {
    let n = peer_lines.len();
    let all: Vec<i64> = (1..=n as i64).collect();
    assert!(lines.contains(&format!("APPLICATIVE RESOLVED {all:?}")), "{lines:?}");
    for (i, pl) in peer_lines.iter().enumerate() {
        let k = i + 1;
        // each term runs once in its node, and the node receives the continuation with the value of every term
        assert_eq!(pl.iter().filter(|l| l.starts_with("TERM")).count(), 1, "node {k}: {pl:?}");
        assert!(pl.iter().any(|l| l == &format!("AFTER APPLICATIVE AT {} {all:?}", base as usize + k)), "node {k}: {pl:?}");
        assert!(lines.iter().any(|l| l.starts_with("BACK FROM") && l.ends_with(&format!("{all:?}"))), "{lines:?}");
    }
    assert_eq!(lines.iter().filter(|l| l.starts_with("BACK FROM")).count(), n, "{lines:?}");
}

#[test]
fn an_applicative_of_two_remote_terms() {
    let (lines, peers) = run_app("two", 9800, &[100, 500]);
    check(&lines, &peers, 9800);
}

#[test]
fn the_first_term_finishes_last() {
    let (lines, peers) = run_app("swap", 9810, &[500, 100]);
    check(&lines, &peers, 9810);
}

/// Three terms, in every order of termination (AppThree.hs)
#[test]
fn an_applicative_of_three_remote_terms_in_any_order() {
    for (i, delays) in [[100u64, 300, 500], [500, 300, 100], [300, 500, 100]].iter().enumerate() {
        let base = 9820 + 10 * i as u16;
        let (lines, peers) = run_app(&format!("three{i}"), base, delays);
        check(&lines, &peers, base);
    }
}

/// AppRestart.hs: the job after the applicative saves its closure; when the caller is restarted with `jobs exec`, it
/// continues from the job reading both values from the log, without executing the terms again.
fn restart_after_applicative(name: &str, base: u16, delays: [u64; 2]) {
    let dir = tmp(name);
    let mode = format!("app:{},{}:job", delays[0], delays[1]);
    let nodes = format!("{}{}", add(base + 1), add(base + 2));
    let peers = [spawn(&dir.join("n1"), &mode, &path(base + 1, "")), spawn(&dir.join("n2"), &mode, &path(base + 2, ""))];
    for (_, rx) in &peers {
        wait_line(rx, "READY");
    }
    let (mut caller, rx) = spawn(&dir.join("n0"), &mode, &path(base, &format!("{nodes}/go")));
    assert_eq!(wait_line(&rx, "AFTER JOB"), "AFTER JOB [1, 2]");
    caller.0.kill().unwrap();
    caller.0.wait().unwrap();

    // RESTART_FORGET=1 restarts the caller with an empty directory: the test must then fail (negative control)
    let dir0 = if std::env::var("RESTART_FORGET").is_ok() { dir.join("n0-empty") } else { dir.join("n0") };
    // (with `jobs/exec` in the command line the job runs at once, before the node says it is ready)
    let (_caller2, rx2) = spawn(&dir0, &mode, &path(base, &format!("{nodes}/jobs/exec")));
    assert_eq!(wait_line(&rx2, "AFTER JOB"), "AFTER JOB [1, 2]");
    std::thread::sleep(Duration::from_millis(300));
    // neither term was executed again
    for (i, (_, prx)) in peers.iter().enumerate() {
        let terms = prx.try_iter().filter(|l| l.starts_with("TERM")).count();
        assert_eq!(terms, 1, "node {}", i + 1);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_job_after_an_applicative_survives_a_restart() {
    restart_after_applicative("restart", 9860, [100, 500]);
}

#[test]
fn a_job_after_an_applicative_survives_a_restart_when_the_first_term_finishes_last() {
    restart_after_applicative("restart-swap", 9870, [500, 100]);
}

/// Every closure file has the definitive values: when the first term of an applicative finishes last, the closures created
/// meanwhile hold a hole for it, and their files must not keep the `w/` of the moment they were written.
#[test]
fn the_closure_files_hold_the_values_of_the_terms_that_finished_last() {
    let dir = tmp("files");
    let base = 9880;
    let mode = "app:500,100";
    let nodes = format!("{}{}", add(base + 1), add(base + 2));
    let peers = [spawn(&dir.join("n1"), mode, &path(base + 1, "")), spawn(&dir.join("n2"), mode, &path(base + 2, ""))];
    for (_, rx) in &peers {
        wait_line(rx, "READY");
    }
    let (_caller, rx) = spawn(&dir.join("n0"), mode, &path(base, &format!("{nodes}/go")));
    wait_line(&rx, "TEST END");
    std::thread::sleep(Duration::from_millis(300));

    let logs: Vec<(String, String)> = std::fs::read_dir(dir.join("n0").join(".tcachedata"))
        .unwrap()
        .filter_map(|e| {
            let e = e.ok()?;
            let text = std::fs::read_to_string(e.path()).ok()?;
            let stored: serde_json::Value = serde_json::from_str(&text).ok()?;
            Some((e.file_name().into_string().ok()?, stored["log"].as_str()?.to_string()))
        })
        .collect();
    assert!(logs.len() > 2, "{logs:?}");
    // the start of the program (the options of the console, `go`, the list of nodes) has `w/` of its own; after the
    // list of nodes, any `w/` is a term of the applicative that was pending when the closure was created
    let pending: Vec<_> = logs
        .iter()
        .filter(|(_, log)| log.split_once("]/").map_or(false, |(_, after)| after.contains("w/")))
        .collect();
    assert!(pending.is_empty(), "closure files with a pending term: {pending:?}");
    let _ = std::fs::remove_dir_all(dir);
}
