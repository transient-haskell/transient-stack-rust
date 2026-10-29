//! A node of a small distributed program, to be run as separate processes (see `tests/restart.rs`, `tests/applicative.rs`
//! and `tests/save_on_exit.rs`). It is started like the nodes of the Haskell battery:
//!
//! ```text
//! node_demo <program> -p start/127.0.0.1/9000/saveonexit/add/127.0.0.1/9001/n/jobs/exec/go
//! ```
//!
//! `start/<host>/<port>` is the address of the node, `add/<host>/<port>/n` adds a node to the list of known ones,
//! `jobs/exec` resumes the jobs, `saveonexit` saves the state when the process ends and `go` starts the program in the node
//! that gets it. The closures are in `.tcachedata`, in the current directory. The programs:
//!
//! ```text
//! calls               two calls to the first known node with a pause in between
//! endpoint            a program with an endpoint, to be resumed by a message
//! job                 a call to the first known node whose body has a job (RunAtRestart.hs)
//! app:<d1>,<d2>[,..][:job]
//!                     an applicative of calls to the known nodes, each one taking <d> ms
//!                     (AppWait.hs, AppThree.hs, AppRestart.hs). With `job`, a job follows it.
//! ping                a job and a call to the first known node, that may be down (runping.sh)
//! ```
//!
//! The console reads standard input too: `jobs exec` or `save` can be typed.

use std::time::Duration;
use transient_core::*;

/// The node that the program calls: the first one that was added
fn peer(nodes: &[Node]) -> Node {
    nodes[1].clone()
}

fn calls() -> Cloud<()> {
    cdo! {
        local(option("go", "run the test"));
        let nodes = local(get_nodes());
        let first = run_at(peer(&nodes), local_io(|| {
            println!("PEER SERVES 1");
            1i64
        }));
        let _pause = local_io(|| {
            println!("CALLER WAITS");
            std::thread::sleep(Duration::from_secs(4));
            0i64
        });
        let second = run_at(peer(&nodes), local_io(|| {
            println!("PEER SERVES 2");
            2i64
        }));
        on_all(lift_io(move || println!("RESULT {first} {second}")))
    }
}

/// A step, an endpoint and another step. After a restart, a message addressed to the endpoint resumes the program there.
fn endpoint_program() -> Cloud<()> {
    cdo! {
        local(option("go", "run the test"));
        let a = local_io(|| {
            println!("STEP A");
            1i64
        });
        on_all(endpoint_here());
        let b = local_io(|| {
            println!("STEP B");
            2i64
        });
        on_all(lift_io(move || println!("RESULT {a} {b}")))
    }
}

/// The body of the call has a job and a wait. If the called node dies during the wait and is restarted with
/// `jobs exec`, the job is replayed and the node responds to the caller, which is still waiting.
fn job_program() -> Cloud<()> {
    cdo! {
        run_jobs();
        local(option("go", "run the test"));
        let nodes = local(get_nodes());
        let r = run_at(peer(&nodes), cdo! {
            let x = local_io(|| "before".to_string());
            job();
            let _wait = local_io(|| {
                println!("N1 WORKING");
                std::thread::sleep(Duration::from_millis(std::env::var("RR_DELAY").ok().and_then(|d| d.parse().ok()).unwrap_or(6000)));
                0i64
            });
            cpure(format!("{x} after"))
        });
        on_all(lift_io({ let r = r.clone(); move || println!("RESULT {r}") }))
    }
}

/// A call to a node that takes `d` ms to give its number `i`
fn term(node: Node, d: u64, i: i64) -> Cloud<i64> {
    run_at(node, local_io(move || {
        std::thread::sleep(Duration::from_millis(d));
        println!("TERM{i} executed");
        i
    }))
}

/// The applicative of the calls to all the known nodes, then a call to each of them with the result, as AppWait.hs and
/// AppThree.hs do. With `with_job`, a job follows the applicative (AppRestart.hs).
fn app_program(delays: Vec<u64>, with_job: bool) -> Cloud<()> {
    cdo! {
        [delays]
        run_jobs();
        local(option("go", "run the test"));
        let nodes = local(get_nodes());
        app_with(nodes[1..].to_vec(), delays, with_job)
    }
}

fn app_with(peers: Vec<Node>, delays: Vec<u64>, with_job: bool) -> Cloud<()> {
    let mut terms = peers.iter().zip(&delays).enumerate().map(|(k, (p, d))| term(p.clone(), *d, k as i64 + 1));
    let first = terms.next().unwrap().map(|x| vec![x]);
    let all = terms.fold(first, |acc, t| clift2(|mut v: Vec<i64>, x| { v.push(x); v }, acc, t));
    cdo! {
        [peers]
        let r = all;
        local_io({ let r = r.clone(); move || println!("APPLICATIVE RESOLVED {r:?}") });
        after_applicative(with_job, peers, r)
    }
}

/// With a job, the program is as AppRestart.hs: a job after the applicative and the end. Without it, a call to each node.
fn after_applicative(with_job: bool, peers: Vec<Node>, r: Vec<i64>) -> Cloud<()> {
    if with_job {
        cdo! {
            [r]
            job();
            local_io(move || println!("AFTER JOB {r:?}"))
        }
    } else {
        calls_to_peers(peers, r)
    }
}

/// A call to each node with the result of the applicative, in sequence, and the end
fn calls_to_peers(peers: Vec<Node>, r: Vec<i64>) -> Cloud<()> {
    match peers.split_first() {
        None => local_io(|| println!("TEST END")),
        Some((peer, rest)) => {
            let (rest, k) = (rest.to_vec(), peer.port);
            cdo! {
                [rest, r]
                let back = run_at(peer.clone(), local_io({ let r = r.clone(); move || { println!("AFTER APPLICATIVE AT {k} {r:?}"); r.clone() } }));
                local_io(move || println!("BACK FROM {k} {back:?}"));
                calls_to_peers(rest, r)
            }
        }
    }
}

/// The caller makes a job and calls the first known node. If it is down, the call fails and the job is kept for it; when it is
/// up and runs its jobs, it pings the known nodes and the caller launches the job again (runping.sh).
fn ping_program() -> Cloud<()> {
    cor(
        input_nodes(),
        cdo! {
            run_jobs();
            local(option("go", "run the test"));
            let nodes = local(get_nodes());
            job();
            let r = run_at(peer(&nodes), local_io(|| {
                println!("WORK");
                7i64
            }));
            local_io(move || println!("RESULT {r}"))
        },
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let program = args[1].clone();
    // the nodes can be added in the command line, and the program has to see them: they are the alternative of the program
    let with_nodes = |app: Cloud<()>| cor(input_nodes(), app);
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(500));
        println!("READY");
    });
    let rest = &args[2..];
    let _: Result<(), String> = match program.as_str() {
        "calls" => keep_args(rest, init_node(with_nodes(calls()))),
        "endpoint" => keep_args(rest, init_node(with_nodes(endpoint_program()))),
        "job" => keep_args(rest, init_node(with_nodes(job_program()))),
        "ping" => keep_args(rest, init_node(ping_program())),
        p if p.starts_with("app:") => {
            let mut spec: Vec<&str> = p[4..].split(':').collect();
            let with_job = spec.last() == Some(&"job");
            if with_job {
                spec.pop();
            }
            let delays = spec.join(":").split(',').map(|d| d.parse().unwrap()).collect();
            keep_args(rest, init_node(with_nodes(app_program(delays, with_job))))
        }
        other => panic!("unknown program {other}"),
    };
}
