//! The list of known nodes (`getNodes`, `addNodes`, `delNodes`, `setNodes`).

use transient_core::*;

fn node(port: u16) -> Node {
    create_node("127.0.0.1", port)
}

#[test]
fn the_list_starts_with_this_node_and_new_nodes_go_after_it() {
    let mut ctx = Ctx::new();
    init_node_at(node(1)).run(&mut ctx);
    assert_eq!(get_nodes().run(&mut ctx), Some(vec![node(1)]));
    add_nodes(vec![node(2), node(3)]).run(&mut ctx);
    // a node that is already there is not added again
    add_nodes(vec![node(3), node(4)]).run(&mut ctx);
    assert_eq!(get_nodes().run(&mut ctx), Some(vec![node(1), node(2), node(3), node(4)]));
    del_nodes(vec![node(2)]).run(&mut ctx);
    assert_eq!(get_nodes().run(&mut ctx), Some(vec![node(1), node(3), node(4)]));
    set_nodes(vec![node(1)]).run(&mut ctx);
    assert_eq!(get_nodes().run(&mut ctx), Some(vec![node(1)]));
}

#[test]
fn a_node_is_a_loggable_value() {
    assert_eq!(to_log(&node(8000)), "{\"host\":\"127.0.0.1\",\"port\":8000}");
    assert_eq!(from_log::<Node>("{\"host\":\"h\",\"port\":1}/rest"), Some((create_node("h", 1), "/rest")));
}
