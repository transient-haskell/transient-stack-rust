//! Nodes killed and restarted, as separate processes (`examples/node_demo.rs`).

mod common;
use common::*;
use std::time::Duration;
use transient_core::{encode, Fragment, Wire};

#[test]
fn a_restarted_caller_reconnects_and_the_first_call_is_replayed_from_the_log() {
    let tmp = std::env::temp_dir().join(format!("transient-restart-{}", std::process::id()));
    let (dir_a, dir_b) = (tmp.join("a"), tmp.join("b"));
    let (pa, pb) = (9711, 9712);

    let (mut b1, rx_b1) = spawn(&dir_b, "calls", &path(pb, ""));
    wait_line(&rx_b1, "READY");
    let (_a, rx_a) = spawn(&dir_a, "calls", &path(pa, &format!("{}/go", add(pb))));
    wait_line(&rx_b1, "PEER SERVES 1");
    wait_line(&rx_a, "CALLER WAITS");

    // the peer dies while the caller waits, and comes back with the same directory
    b1.0.kill().unwrap();
    b1.0.wait().unwrap();
    // RESTART_FORGET=1 starts the new process with an empty directory: the test must then fail (negative control)
    let dir_b2 = if std::env::var("RESTART_FORGET").is_ok() { tmp.join("b-empty") } else { dir_b.clone() };
    let (_b2, rx_b2) = spawn(&dir_b2, "calls", &path(pb, ""));
    wait_line(&rx_b2, "READY");

    assert_eq!(wait_line(&rx_a, "RESULT"), "RESULT 1 2");
    // the first call is replayed from the log in the restarted process, not executed again
    let served: Vec<String> = rx_b2.try_iter().collect();
    assert!(!served.iter().any(|l| l == "PEER SERVES 1"), "{served:?}");
    assert!(dir_b.join(".tcachedata").join("0-0").exists());

    let _ = std::fs::remove_dir_all(tmp);
}

/// Sends a message to a closure of a node, as another node would
fn send(port: u16, clos_to: &str, logs: Vec<Fragment>) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(b"CLOS cookie1 b \r\nHost: 127.0.0.1\r\nPort: 9999\r\n\r\n").unwrap();
    let mut ok = [0u8; 2];
    s.read_exact(&mut ok).unwrap();
    let body = encode(&Wire { last: false, clos_to: clos_to.into(), sess_to: "0".into(), clos_from: "7".into(), sess_from: "0".into(), logs });
    s.write_all(format!("{}{}", body.len(), body).as_bytes()).unwrap();
    std::thread::sleep(Duration::from_millis(500));
}

/// The endpoint of a program that finished is a closure on disk. A new process finds it not alive, and a message
/// addressed to it makes the program go on from there: the steps before it come from the log of the closures stored.
#[test]
fn a_message_resumes_an_endpoint_stored_by_a_previous_process() {
    let tmp = std::env::temp_dir().join(format!("transient-endpoint-{}", std::process::id()));
    let dir = tmp.join("b");
    let port = 9721;

    let (mut b1, rx1) = spawn(&dir, "endpoint", &path(port, "/go"));
    wait_line(&rx1, "RESULT");
    // the closures are written in the background, a few milliseconds after they are created: wait for the endpoint's file
    let endpoint_file = || std::fs::read_dir(dir.join(".tcachedata")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).find(|n| n != "0-0" && n != "jobs" && !n.starts_with('.'));
    let t0 = std::time::Instant::now();
    while endpoint_file().is_none() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    b1.0.kill().unwrap();
    b1.0.wait().unwrap();

    let key = endpoint_file().expect("an endpoint on disk");
    let (clos, session) = key.split_once('-').unwrap();
    assert_eq!(session, "0");

    // RESTART_FORGET=1 starts the new process with an empty directory: the test must then fail (negative control)
    let dir2 = if std::env::var("RESTART_FORGET").is_ok() { tmp.join("b-empty") } else { dir.clone() };
    let (_b2, rx2) = spawn(&dir2, "endpoint", &path(port, ""));
    wait_line(&rx2, "READY");
    send(port, clos, vec![Fragment::text("20/")]);
    // `a` comes from the log (1), `b` from the message (20); nothing is executed again
    assert_eq!(wait_line(&rx2, "RESULT"), "RESULT 1 20");
    let rest: Vec<String> = rx2.try_iter().collect();
    assert!(!rest.iter().any(|l| l.starts_with("STEP")), "{rest:?}");

    let _ = std::fs::remove_dir_all(tmp);
}

/// RunAtRestart.hs: the called node dies while it serves a `run_at` whose body has a job and a wait. Restarted with
/// `jobs exec`, it replays the job and responds to the caller, which is still waiting: the closure to respond to
/// and the node of the caller have to survive the restart.
#[test]
fn a_job_resumes_a_call_and_the_restarted_node_responds_to_the_caller() {
    let tmp = std::env::temp_dir().join(format!("transient-job-{}", std::process::id()));
    let (dir_a, dir_b) = (tmp.join("a"), tmp.join("b"));
    let (pa, pb) = (9731, 9732);

    let (mut b1, rx_b1) = spawn(&dir_b, "job", &path(pb, &add(pa)));
    wait_line(&rx_b1, "READY");
    let (_a, rx_a) = spawn(&dir_a, "job", &path(pa, &format!("{}/go", add(pb))));
    wait_line(&rx_b1, "N1 WORKING");

    // the called node dies during its wait
    b1.0.kill().unwrap();
    b1.0.wait().unwrap();
    assert!(dir_b.join(".tcachedata").join("jobs").read_dir().unwrap().next().is_some(), "the job is on disk");

    // RESTART_FORGET=1 starts the new process with an empty directory: the test must then fail (negative control)
    let dir_b2 = if std::env::var("RESTART_FORGET").is_ok() { tmp.join("b-empty") } else { dir_b.clone() };
    // (with `jobs/exec` in the command line the job runs at once, before the node says it is ready)
    let (_b2, rx_b2) = spawn(&dir_b2, "job", &path(pb, &format!("{}/jobs/exec", add(pa))));

    assert_eq!(wait_line(&rx_a, "RESULT"), "RESULT before after");
    // the part before the job is replayed from the log, the rest is executed once in the restarted process
    let seen: Vec<String> = rx_b2.try_iter().collect();
    assert!(seen.iter().filter(|l| *l == "N1 WORKING").count() == 1, "{seen:?}");
    let _ = std::fs::remove_dir_all(tmp);
}

/// runping.sh: the caller calls a node that is down; the send fails and the job is kept for that node. When the node
/// starts with its jobs, it connects to the known nodes: the caller gets the `Ping` and launches the job again.
#[test]
fn a_node_that_comes_up_pings_the_caller_which_launches_the_job_that_waited_for_it() {
    let tmp = std::env::temp_dir().join(format!("transient-ping-{}", std::process::id()));
    let (p0, p1) = (9761, 9762);

    let (_n0, rx0) = spawn(&tmp.join("n0"), "ping", &path(p0, &format!("{}/jobs/exec/go", add(p1))));
    wait_line(&rx0, "READY");
    // the call to the node that is down fails; nothing else happens
    std::thread::sleep(Duration::from_secs(2));
    assert!(rx0.try_iter().all(|l| !l.starts_with("RESULT")));

    // RESTART_FORGET=1: the node does not run its jobs, so it does not ping: the caller never launches the job (negative control)
    let jobs = if std::env::var("RESTART_FORGET").is_err() { "/jobs/exec" } else { "" };
    let (_n1, rx1) = spawn(&tmp.join("n1"), "ping", &path(p1, &format!("{}{jobs}", add(p0))));

    assert_eq!(wait_line(&rx0, "RESULT"), "RESULT 7");
    let work: Vec<String> = rx1.try_iter().filter(|l| l == "WORK").collect();
    assert_eq!(work.len(), 1, "{work:?}");
    let _ = std::fs::remove_dir_all(tmp);
}
