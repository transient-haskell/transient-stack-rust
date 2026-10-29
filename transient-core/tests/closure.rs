//! `endpoint`: the continuation is stored and a message with the rest of the log resumes it.

use std::sync::{Arc, Mutex};
use transient_core::*;

#[test]
fn endpoint_resumes_with_a_message() {
    let out = Arc::new(Mutex::new(Vec::new()));
    let o = out.clone();
    let prog = cdo! {
        [o]
        let a = local(lift_io(|| 1i64));
        on_all(endpoint_here());
        let b = local(lift_io(|| 20i64));
        on_all(lift_io({ let o = o.clone(); move || o.lock().unwrap().push(a + b) }))
    };
    let mut ctx = Ctx::new();
    init_node_at(create_node("127.0.0.1", 1)).run(&mut ctx);
    prog.run(&mut ctx);
    assert_eq!(*out.lock().unwrap(), vec![21]);

    // the endpoint is named after the hash of the path that leads to it: after a `local` it is hash_step(0, HASH_DONE)
    let name = hash_step(0, HASH_DONE).to_string();
    let table = get_closures().run(&mut ctx).unwrap();
    assert!(table.read(&k_local_clos("0", &name)).is_some());

    // a message with the rest of the log: `b` comes from the log (5), it is not executed; `a` is kept from the first execution
    send_to_closure(&table, "0", &name, vec![Fragment::text("5/")], false);
    assert_eq!(*out.lock().unwrap(), vec![21, 6]);

    // and another one: the continuation is still alive
    send_to_closure(&table, "0", &name, vec![Fragment::text("100/")], false);
    assert_eq!(*out.lock().unwrap(), vec![21, 6, 101]);
}
