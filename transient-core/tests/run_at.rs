//! `run_at` from a node to itself: no network, but it creates the same continuation points and the same log as a remote call.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use transient_core::*;

fn wait_for<T: Clone>(out: &Arc<Mutex<Vec<T>>>, n: usize) -> Vec<T> {
    let t0 = Instant::now();
    while out.lock().unwrap().len() < n && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
    }
    out.lock().unwrap().clone()
}

#[test]
fn self_run_at_returns_the_remote_result() {
    let node = create_node("localhost", 8000);
    let out = Arc::new(Mutex::new(Vec::new()));
    let (o, n) = (out.clone(), node.clone());
    let prog = cdo! {
        [o, n]
        let a = local_io(|| 1i64);
        let r = run_at(n, local_io(|| 10i64));
        on_all(lift_io({ let o = o.clone(); move || o.lock().unwrap().push(a + r) }))
    };
    let mut ctx = Ctx::new();
    init_node_at(node).run(&mut ctx);
    put_log(Log { hash: 30847, ..Log::default() }).run(&mut ctx);
    prog.run(&mut ctx);
    assert_eq!(wait_for(&out, 1), vec![11]);
    // the names of the closures, running the original in Haskell (SelfRunAt.hs): after the `getMyNode` of `initNode` the
    // hash of the path was 30847, and the closures were named 29644030 and 570625483
    let table = get_closures().run(&mut ctx).unwrap();
    let keys = table.keys();
    assert!(keys.contains(&"29644030-0".to_string()) && keys.contains(&"570625483-0".to_string()), "{keys:?}");
    assert_eq!(table.read("570625483-0").unwrap().prev, "29644030-0");
    assert_eq!(table.read("570625483-0").unwrap().local_log.render(), "u/10/");
}
