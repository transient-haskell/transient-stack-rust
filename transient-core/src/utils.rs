//! Node start-up (`Transient.Move.Utils`).

use super::*;
use node::{MyNode, Node, NodeList};

/// Prepares a computation in a node whose address is known, without listening to other nodes: sets this node, the base
/// connection and the first continuation point. For tests; a program starts with `init_node`.
pub fn init_node_at(node: Node) -> Trans<()> {
    tdo! {
        init_state(node, Arc::new(Closures::default()));
        first_cont()
    }
}

/// Sets this node and the base connection (`defConnection`)
fn init_state(node: Node, table: Arc<Closures>) -> Trans<()> {
    Trans::new(move |ctx| {
        ctx.set(MyNode(node.clone()));
        ctx.set(NodeList(Arc::new(Mutex::new(vec![node.clone()]))));
        ctx.set(Arc::new(Pool::default()));
        table.start_flusher_for(ctx.exit.clone());
        ctx.set(table.clone());
        ctx.set(Connection {
            id: gen_new_id(),
            my_node: Arc::new(Mutex::new(node.clone())),
            remote_node: Arc::new(Mutex::new(Some(node.clone()))),
            kind: Arc::new(Mutex::new(Some(ConnKind::Self_))),
            calling: true,
            synchronous: false,
        });
        Some(())
    })
}


/// Starts a node with its program (`initNode`): it listens to the other nodes, it has the continuation point 0 that
/// the programs of the others reach, and it runs `app` inside a wormhole to itself. The closures are kept in memory.
pub fn init_web_app<A: Loggable + Send + Sync + 'static>(node: Node, app: Cloud<A>) -> Trans<A> {
    init_web_app_with(Closures::default(), node, app)
}

/// Like `init_web_app`, with the closures persisted in `dir`: a node restarted with the same directory finds the
/// closures of its previous life and can resume the calls addressed to them.
pub fn init_web_app_in<A: Loggable + Send + Sync + 'static>(dir: &std::path::Path, node: Node, app: Cloud<A>) -> Trans<A> {
    init_web_app_with(Closures::open(dir), node, app)
}

/// Like `init_web_app_in`, with the policy of the cache (when to write, how many closures to keep in memory)
pub fn init_web_app_in_with<A: Loggable + Send + Sync + 'static>(dir: &std::path::Path, policy: SyncPolicy, node: Node, app: Cloud<A>) -> Trans<A> {
    init_web_app_with(Closures::open_with(dir, policy), node, app)
}

fn init_web_app_with<A: Loggable + Send + Sync + 'static>(table: Closures, node: Node, app: Cloud<A>) -> Trans<A> {
    let listening = tdo! {
        listen(node.clone());
        empty::<()>()
    };
    let app = wormhole(node.clone(), app).un_cloud();
    tdo! {
        [listening, app]
        init_state(node, Arc::new(table));
        listening | first_cont() | pure(());
        abduce();
        app
    }
}

/// Console commands to see and extend the list of known nodes (`inputNodes`). It never gives a result, so it is used as an
/// alternative of the program: `input_nodes() | app`.
///
/// * `nodes` lists the known nodes.
/// * `add <host> <port> <y|n>` adds a node. `y` is for connecting to it to exchange the lists of nodes (`connect'`),
///   which is not ported yet: the node is added anyway.
pub fn input_nodes<A: Loggable + Clone + Send + Sync + 'static>() -> Cloud<A> {
    list_nodes() | add_new()
}

fn list_nodes<A: Send + 'static>() -> Cloud<A> {
    local(tdo! {
        option("nodes", "list nodes");
        let nodes = get_nodes();
        lift_io(move || {
            println!("list of nodes known in this node:");
            nodes.iter().enumerate().for_each(|(i, n)| println!("{i}\t{n:?}"));
        })
    })
    .then_empty()
}

fn add_new<A: Send + 'static>() -> Cloud<A> {
    cdo! {
        local(option("add", "add a new node"));
        let host = local(input::<String>("Hostname of the node (none): "));
        let port = local(input::<u16>("port? "));
        let connectit = local(input::<char>("connect to the node to interchange node lists? (n) "));
        local(tdo! {
            [host]
            add_nodes(vec![create_node(&host, port)]);
            lift_io(move || {
                println!("Added node: {host}:{port}");
                if connectit == 'y' {
                    println!("(connecting to exchange the lists of nodes is not supported: the node is added only)");
                }
            })
        });
        cempty::<A>()
    }
}

/// The parameters of this node, read from the console or the command line: `start <host> <port>`. After `start`, the host
/// has the default `localhost`, and a `retry` tries the next ports if the port is busy. `cookie <value>` sets the
/// cookie of the cluster instead (`getNodeParams`).
///
/// ```text
/// program -p start/localhost/8000/add/localhost/8001/n
/// ```
pub fn get_node_params() -> Trans<Node> {
    let start = tdo! {
        option("start", "re/start node");
        let host = input_default("localhost".to_string(), |_| true, "hostname of this node. (Must be reachable, default:localhost)? ");
        let retry = input_default("noretry".to_string(), |r| r == "retry", "if you want to retry with higher port numbers when the port is busy, write 'retry'");
        let port = input::<u16>("port to listen? ");
        free_port(host, retry, port)
    };
    let cookie = tdo! {
        option("cookie", "set the cookie");
        let c = input::<String>("cookie: ");
        set_cookie_and_stop(c)
    };
    or(start, cookie)
}

/// The node at `host` and the port, or the next free one if the port is busy and the answer was `retry`
fn free_port(host: String, retry: String, port: u16) -> Trans<Node> {
    Trans::new(move |ctx| {
        let mut port = port;
        while std::net::TcpListener::bind(("0.0.0.0", port)).is_err() {
            println!("Port busy: {port}");
            if retry != "retry" {
                exit_left(&format!("{port}: port busy")).run(ctx);
                return None;
            }
            println!("retrying with next port");
            port += 1;
        }
        Some(create_node(&host, port))
    })
}

/// Sets the cookie of the cluster and stops: `cookie` gives no node
fn set_cookie_and_stop(cookie: String) -> Trans<Node> {
    Trans::new(move |_| {
        set_cluster_cookie(&cookie);
        None
    })
}

/// Starts a node whose address comes from the console or the command line, and runs `app` in it (`initNode`). The
/// closures are kept in `.tcachedata`, in the current directory, as TCache does; `TRANSIENT_SYNC_MS` sets the period of
/// the writes (see `SyncPolicy::from_env`).
///
/// ```text
/// program -p start/localhost/8000/saveonexit/add/localhost/8001/n/jobs/exec/go
/// program --start localhost:8000
/// program --start localhost/8000
/// ```
pub fn init_node<A: Loggable + Send + Sync + 'static>(app: Cloud<A>) -> Trans<A> {
    tdo! {
        [app]
        let node = get_node_params();
        start_node(node, app)
    }
}

/// Like `init_node`, but with `host` and `port` when the program has no arguments (`initNodeDef`)
pub fn init_node_def<A: Loggable + Send + Sync + 'static>(host: &str, port: u16, app: Cloud<A>) -> Trans<A> {
    let (host, no_args) = (host.to_string(), std::env::args().len() <= 1);
    let default = Trans::new(move |_| no_args.then(|| create_node(&host, port)));
    tdo! {
        [app]
        let node = or(default, get_node_params());
        start_node(node, app)
    }
}

fn start_node<A: Loggable + Send + Sync + 'static>(node: Node, app: Cloud<A>) -> Trans<A> {
    println!("(\"trying port\",(\"{}\",{},[]))", node.host, node.port);
    init_web_app_in_with(std::path::Path::new(".tcachedata"), SyncPolicy::from_env(), node, app)
}
