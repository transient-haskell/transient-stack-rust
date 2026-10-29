//! Two nodes in the same process, each with its own port and closure table, talking over TCP with the Haskell
//! format.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use transient_core::*;

fn wait_for<T: Clone>(out: &Arc<Mutex<Vec<T>>>, n: usize) -> Vec<T> {
    let t0 = Instant::now();
    while out.lock().unwrap().len() < n && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    out.lock().unwrap().clone()
}

/// The same program in both nodes: it waits for the start signal (only one node receives it; the other one reads
/// it from the log), calls node B and records the result
fn program(b: Node, out: Arc<Mutex<Vec<i64>>>) -> Cloud<()> {
    cdo! {
        [b, out]
        let me = on_all(get_my_node());
        let _go = local(get_mailbox_key::<String, String>(format!("go-{}", me.port)));
        let a = local_io(|| 1i64);
        let r = run_at(b, local_io(|| 10i64));
        on_all(lift_io({ let out = out.clone(); move || out.lock().unwrap().push(a + r) }))
    }
}

#[test]
fn run_at_another_node_over_tcp() {
    let (a, b) = (create_node("127.0.0.1", 9701), create_node("127.0.0.1", 9702));
    let out = Arc::new(Mutex::new(Vec::new()));
    init_web_app(b.clone(), program(b.clone(), out.clone())).run(&mut Ctx::new());
    init_web_app(a.clone(), program(b.clone(), out.clone())).run(&mut Ctx::new());
    std::thread::sleep(Duration::from_millis(300));
    put_mailbox_key("go-9701".to_string(), "go".to_string());
    assert_eq!(wait_for(&out, 1), vec![11]);
}
