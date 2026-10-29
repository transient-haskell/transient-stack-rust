//! Helpers for the tests that run `examples/node_demo.rs` as separate processes.
#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

pub fn example(name: &str) -> PathBuf {
    // target/debug/deps/restart-xxxx -> target/debug/examples/<name>
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().join("examples").join(name)
}

/// A child process that is killed when the test ends, also if it fails
pub struct Node(pub Child);

impl Node {
    /// Sends a signal to the process (`kill -<name> <pid>`)
    pub fn signal(&self, name: &str) {
        let status = Command::new("kill").arg(format!("-{name}")).arg(self.0.id().to_string()).status().unwrap();
        assert!(status.success());
    }

    /// Waits for the process to end, up to `secs` seconds; `None` if it is still alive
    pub fn wait_exit(&mut self, secs: u64) -> Option<std::process::ExitStatus> {
        let t0 = std::time::Instant::now();
        while t0.elapsed() < Duration::from_secs(secs) {
            if let Some(status) = self.0.try_wait().unwrap() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    /// Types a line in the console of the node
    pub fn type_line(&mut self, line: &str) {
        use std::io::Write;
        let stdin = self.0.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts an example of the library as a node, like the nodes of the Haskell battery are started:
/// `<example> <program> -p <path>`, with the directory of the node as the current directory (the closures are in
/// `.tcachedata` there). The output lines arrive through the channel.
pub fn start(example_name: &str, dir: &Path, program: &str, path: &str, env: &[(&str, &str)]) -> (Node, Receiver<String>) {
    std::fs::create_dir_all(dir).unwrap();
    let mut cmd = Command::new(example(example_name));
    // a short period of the writes, so that a process that is killed has written (the default is 10 s)
    cmd.env("TRANSIENT_SYNC_MS", "100").envs(env.iter().copied());
    let mut child = cmd
        .current_dir(dir)
        .arg(program)
        .arg("-p")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("run `cargo build --examples` first");
    let (tx, rx) = channel();
    let out = child.stdout.take().unwrap();
    std::thread::spawn(move || BufReader::new(out).lines().map_while(Result::ok).for_each(|l| tx.send(l).unwrap()));
    (Node(child), rx)
}

/// `<example> <args...>` as it is, with the directory of the node as the current directory: the options of the command line
/// (`web_demo collect --start 127.0.0.1:9100`)
pub fn start_args(example_name: &str, dir: &Path, args: &[&str]) -> (Node, Receiver<String>) {
    std::fs::create_dir_all(dir).unwrap();
    let mut child = Command::new(example(example_name))
        .env("TRANSIENT_SYNC_MS", "100")
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("run `cargo build --examples` first");
    let (tx, rx) = channel();
    let out = child.stdout.take().unwrap();
    std::thread::spawn(move || BufReader::new(out).lines().map_while(Result::ok).for_each(|l| tx.send(l).unwrap()));
    (Node(child), rx)
}

/// A port that nobody is listening on, different each time it is called in this process: the tests of a binary run in
/// parallel and the nodes of two of them must not share one
pub fn free_port() -> u16 {
    static GIVEN: std::sync::Mutex<Vec<u16>> = std::sync::Mutex::new(Vec::new());
    loop {
        let port = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let mut given = GIVEN.lock().unwrap();
        if !given.contains(&port) {
            given.push(port);
            return port;
        }
    }
}

/// `start/127.0.0.1/<port>` plus the rest of the command line (`/add/127.0.0.1/9001/n/go`...)
pub fn path(port: u16, rest: &str) -> String {
    format!("start/127.0.0.1/{port}{rest}")
}

/// `/add/127.0.0.1/<port>/n`: the node is added to the list of known nodes
pub fn add(port: u16) -> String {
    format!("/add/127.0.0.1/{port}/n")
}

/// A node of `node_demo`
pub fn spawn(dir: &Path, program: &str, path: &str) -> (Node, Receiver<String>) {
    start("node_demo", dir, program, path, &[])
}

/// A node of `node_demo` with environment variables
pub fn spawn_with(dir: &Path, program: &str, path: &str, env: &[(&str, &str)]) -> (Node, Receiver<String>) {
    start("node_demo", dir, program, path, env)
}

/// Another example (`web_demo`...) as a node
pub fn spawn_demo(example_name: &str, dir: &Path, port: u16, program: &str) -> (Node, Receiver<String>) {
    start(example_name, dir, program, &path(port, ""), &[])
}

/// Like `spawn_demo`, with more words in the path of the command line (`/jobs/exec`)
pub fn spawn_demo_args(example_name: &str, dir: &Path, port: u16, program: &str, rest: &str) -> (Node, Receiver<String>) {
    start(example_name, dir, program, &path(port, rest), &[])
}

pub fn wait_line(rx: &Receiver<String>, prefix: &str) -> String {
    loop {
        let l = rx.recv_timeout(Duration::from_secs(20)).unwrap_or_else(|_| panic!("timeout waiting for {prefix}"));
        if l.starts_with(prefix) {
            return l;
        }
    }
}

