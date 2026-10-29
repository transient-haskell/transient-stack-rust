//! Nodes and connections (`Transient.Move.Defs`).

use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Node {
    pub host: String,
    pub port: u16,
}

pub fn create_node(host: &str, port: u16) -> Node {
    Node { host: host.into(), port }
}

/// This node (`getMyNode`); set by `init_node`
#[derive(Clone)]
pub(crate) struct MyNode(pub Node);

pub fn get_my_node() -> Trans<Node> {
    Trans::new(|ctx| Some(ctx.get::<MyNode>().expect("get_my_node: use init_node").0.clone()))
}

#[derive(Clone, Debug)]
pub enum ConnKind {
    /// a call from a node to itself: no network
    Self_,
    /// a TCP socket to another node (`Node2Node`)
    Net(Arc<Mutex<std::net::TcpStream>>),
    /// a TCP socket to an HTTP client (`HTTP2Node`): what is sent is written as it is
    Http(Arc<HttpSock>),
}

/// The connection in the state (`Connection`). The cells are shared among the branches that inherit it.
#[derive(Clone)]
pub struct Connection {
    pub id: usize,
    pub my_node: Arc<Mutex<Node>>,
    pub remote_node: Arc<Mutex<Option<Node>>>,
    pub kind: Arc<Mutex<Option<ConnKind>>>,
    /// this node is the caller (`calling`)
    pub calling: bool,
    pub synchronous: bool,
}

/// The closure to respond to (`ClosToRespond`)
#[derive(Clone, Debug)]
pub struct ClosToRespond(pub String);

pub(crate) fn connection(ctx: &Ctx) -> Arc<Connection> {
    ctx.get::<Connection>().expect("teleport: No connection defined: use wormhole")
}


loggable_json!(Node);

/// The nodes this node knows, the first one being itself (`nodeList`). Shared by all the threads of the node.
#[derive(Clone, Default)]
pub(crate) struct NodeList(pub Arc<Mutex<Vec<Node>>>);

fn node_list(ctx: &Ctx) -> Arc<NodeList> {
    ctx.get::<NodeList>().expect("getNodes: Node not set. Initialize it with init_node")
}

/// The list of known nodes (`getNodes`)
pub fn get_nodes() -> Trans<Vec<Node>> {
    Trans::new(|ctx| Some(node_list(ctx).0.lock().unwrap().clone()))
}

/// Adds nodes to the list of known nodes; the ones that were already there stay where they are (`addNodes`)
pub fn add_nodes(nodes: Vec<Node>) -> Trans<()> {
    Trans::new(move |ctx| {
        let list = node_list(ctx);
        let mut known = list.0.lock().unwrap();
        for n in &nodes {
            if !known.contains(n) {
                known.push(n.clone());
            }
        }
        Some(())
    })
}

/// Removes nodes from the list (`delNodes`)
pub fn del_nodes(nodes: Vec<Node>) -> Trans<()> {
    Trans::new(move |ctx| {
        node_list(ctx).0.lock().unwrap().retain(|n| !nodes.contains(n));
        Some(())
    })
}

/// Replaces the list (`setNodes`)
pub fn set_nodes(nodes: Vec<Node>) -> Trans<()> {
    Trans::new(move |ctx| {
        *node_list(ctx).0.lock().unwrap() = nodes.clone();
        Some(())
    })
}
