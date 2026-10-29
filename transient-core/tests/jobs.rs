//! `job`, `job_task`: when the record of a job is created and removed (`Transient.Move.Job`).

use std::sync::{Arc, Mutex};
use transient_core::*;

fn node() -> Node {
    create_node("127.0.0.1", 9741)
}

/// Runs `prog` in a node and returns the number of jobs of the node that `observe` saw at each of its calls
fn run(prog: impl Fn(Arc<dyn Fn() -> Trans<()> + Send + Sync>) -> Cloud<()>) -> (Vec<usize>, usize) {
    let mut ctx = Ctx::new();
    init_node_at(node()).run(&mut ctx);
    let table = get_closures().run(&mut ctx).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observe: Arc<dyn Fn() -> Trans<()> + Send + Sync> = {
        let (table, seen) = (table.clone(), seen.clone());
        Arc::new(move || {
            let (table, seen) = (table.clone(), seen.clone());
            lift_io(move || seen.lock().unwrap().push(table.jobs_for(&node()).len()))
        })
    };
    prog(observe).run(&mut ctx);
    std::thread::sleep(std::time::Duration::from_millis(100));
    let seen = seen.lock().unwrap().clone();
    (seen, table.jobs_for(&node()).len())
}

#[test]
fn the_next_job_removes_the_previous_one() {
    let (seen, last) = run(|observe| {
        cdo! {
            [observe]
            job();
            on_all(observe());
            job();
            on_all(observe())
        }
    });
    assert_eq!(seen, vec![1, 1]);
    assert_eq!(last, 1);
}

#[test]
fn a_job_task_is_closed_when_its_computation_ends() {
    let (seen, last) = run(|observe| {
        cdo! {
            [observe]
            let _r = job_task(cdo! {
                [observe]
                on_all(observe());
                local_io(|| 5i64)
            });
            on_all(observe())
        }
    });
    assert_eq!(seen, vec![1, 0]);
    assert_eq!(last, 0);
}
