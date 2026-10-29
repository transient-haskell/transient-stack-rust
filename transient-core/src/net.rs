//! The network between nodes (`Transient.Move.Internals`): TCP connection, messages and listening. The format of the
//! messages is Haskell's, byte by byte (captured between two Haskell nodes):
//!
//! ```text
//! client → server   CLOS <cookie> b \r\nHost: 127.0.0.1\r\nPort: 9000\r\n\r\n
//! server → client   OK
//! then, messages in both directions:   <length in bytes, in decimal><message>
//! message   SMore/<destination closure>/<destination session>/<origin closure>/<origin session>/["u//1//","e//"]
//! ```
//!
//! The fragments are JSON: a fragment is a JSON string (with the slashes doubled, like the whole log) and the list
//! is an array.

use super::*;
use node::{MyNode, Node};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

// ---------------------------------------------------------------------------------------------------------------
// messages (`NodeMSG` inside a `StreamData`)

#[derive(Clone, Debug, PartialEq)]
pub struct Wire {
    /// `SLast` (the last message of the connection of the closure) or `SMore`
    pub last: bool,
    pub clos_to: String,
    pub sess_to: String,
    pub clos_from: String,
    pub sess_from: String,
    pub logs: Vec<Fragment>,
}

pub fn encode(w: &Wire) -> String {
    let frags: Vec<serde_json::Value> = w.logs.iter().map(|f| match f {
        Fragment::Text(t) => serde_json::json!(t),
        Fragment::Collect(ts) => serde_json::json!(ts),
    }).collect();
    format!(
        "{}/{}/{}/{}/{}/{}",
        if w.last { "SLast" } else { "SMore" },
        w.clos_to,
        w.sess_to,
        w.clos_from,
        w.sess_from,
        to_log(&frags)
    )
}

pub fn decode(s: &str) -> Option<Wire> {
    let (kind, s) = s.split_once('/')?;
    let last = match kind {
        "SMore" => false,
        "SLast" => true,
        _ => return None,
    };
    let mut parts = s.splitn(5, '/');
    let (clos_to, sess_to, clos_from, sess_from) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let (frags, _) = from_log::<Vec<serde_json::Value>>(parts.next()?)?;
    let logs = frags
        .into_iter()
        .map(|v| match v {
            serde_json::Value::Array(ts) => Fragment::Collect(ts.into_iter().map(|t| t.as_str().unwrap().to_string()).collect()),
            v => Fragment::Text(v.as_str().unwrap().to_string()),
        })
        .collect();
    Some(Wire { last, clos_to: clos_to.into(), sess_to: sess_to.into(), clos_from: clos_from.into(), sess_from: sess_from.into(), logs })
}

// ---------------------------------------------------------------------------------------------------------------
// the socket

/// Sends a message: its length in decimal and the message (`msend`)
pub fn msend(conn: &Connection, body: &str) {
    let kind = conn.kind.lock().unwrap().clone();
    match kind {
        Some(ConnKind::Net(s)) => {
            let mut s = s.lock().unwrap();
            s.write_all(format!("{}{}", body.len(), body).as_bytes()).expect("msend: connection broken");
        }
        Some(ConnKind::Http(s)) => s.send(body),
        Some(ConnKind::Self_) => {}
        None => panic!("msend out of connection context: use wormhole to connect"),
    }
}

type Reader = Arc<Mutex<BufReader<TcpStream>>>;

/// The next message, or `None` if the connection was closed
fn read_frame(r: &Reader) -> Option<String> {
    let mut r = r.lock().unwrap();
    let mut len = String::new();
    loop {
        let buf = r.fill_buf().ok()?;
        if buf.is_empty() {
            return None;
        }
        let n = buf.iter().take_while(|b| b.is_ascii_digit()).count();
        len.push_str(std::str::from_utf8(&buf[..n]).unwrap());
        let all = n == buf.len();
        r.consume(n);
        if !all {
            break;
        }
    }
    let mut body = vec![0u8; len.parse().expect("malformed data received: Int expected")];
    r.read_exact(&mut body).ok()?;
    Some(String::from_utf8(body).expect("mread: message is not UTF-8"))
}

/// The cookie that the nodes of a cluster share to accept each other's connections (`rcookie`). It is set with the console
/// command `cookie`; the default is the one of the original.
fn cluster_cookie() -> &'static Mutex<String> {
    static C: std::sync::OnceLock<Mutex<String>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new("cookie1".into()))
}

/// The cookie of the cluster now
pub(crate) fn cluster_cookie_now() -> String {
    cluster_cookie().lock().unwrap().clone()
}

pub fn set_cluster_cookie(cookie: &str) {
    *cluster_cookie().lock().unwrap() = cookie.to_string();
}

fn handshake_line(my: &Node) -> String {
    format!("CLOS {} b \r\nHost: {}\r\nPort: {}\r\n\r\n", cluster_cookie().lock().unwrap(), my.host, my.port)
}

/// The nodes that already have an open connection (the pool of `mconnect`)
#[derive(Default)]
pub(crate) struct Pool(Mutex<HashMap<Node, Connection>>);

/// What is done with each message that arrives through a connection (`processMessage`): it is handed to the
/// continuation point it is addressed to
pub(crate) fn process_message(ctx: &mut Ctx, w: Wire) {
    let table = closures(ctx);
    // a session `#<source>-<new>` is a jump: the closure of the source session is copied under the new session, and the
    // message goes to the source one (it is the endpoint of another user). The same happens to a message for such a copy
    // whose source is alive and the copy is not (the node was restarted): the session goes on in the execution that the
    // source belongs to, which may hold state shared with the others (a `collectc`).
    let jump = match w.sess_to.strip_prefix('#').and_then(|r| r.split_once('-')) {
        Some((source, new_session)) => Some((source.to_string(), new_session.to_string())),
        None => table.read(&k_local_clos(&w.sess_to, &w.clos_to)).filter(|c| !c.live).and_then(|c| c.source).and_then(|src| {
            let alive = table.read(&src).is_some_and(|s| s.live);
            alive.then(|| (get_sess_closure(&src).0, w.sess_to.clone()))
        }),
    };
    let w = match jump {
        Some((source, new_session)) => {
            let source_key = k_local_clos(&source, &w.clos_to);
            let cl = table.read(&source_key).unwrap_or_else(|| panic!("request received for non existent closure {source_key}"));
            table.write(k_local_clos(&new_session, &w.clos_to), LocalClosure { session: new_session.clone(), source: Some(source_key), ..cl.clone() });
            if let Some(p) = ctx.get::<PrevClos>() {
                ctx.set(PrevClos { prev: cl.prev.clone(), ..(*p).clone() });
            }
            Wire { sess_to: source, ..w }
        }
        None => w,
    };
    let conn = (*ctx.get::<Connection>().expect("Listen: myNode not set, use init_node")).clone();
    let key = k_local_clos(&w.sess_to, &w.clos_to);
    let lc = table.read(&key).unwrap_or_else(|| panic!("request received for non existent closure {key}"));
    // a closure that is not alive (the node was restarted) is reached from the nearest one that is, replaying the
    // log of the closures in between
    let (mut logs, target) = if lc.live { (Vec::new(), lc) } else { get_closure_log(&table, &w.sess_to, &w.clos_to) };
    logs.extend(w.logs);
    let ev = target.evar.unwrap_or_else(|| panic!("NO EVAR IN CLOSURE {key}"));
    ev.write(Message {
        last: w.last,
        logs,
        session: w.sess_from.clone(),
        clos: w.clos_from.clone(),
        from_remote: true,
        conn: Some(conn),
        to_respond: Some(ClosToRespond(k_local_clos(&w.sess_from, &w.clos_from))),
        headers: ctx.get::<HttpHeaders>().map(|h| (*h).clone()),
    });
}

/// Reads the messages of a connection; each one goes on in its own thread (`parallelReadHandler`). When the other
/// end closes it, the connection is marked closed and leaves the pool: the next send opens a new one.
fn read_messages(reader: Reader, conn: Connection, pool: Option<Arc<Pool>>) -> Trans<()> {
    let frames = parallel(move || match read_frame(&reader) {
        Some(m) => StreamData::More(m),
        None => {
            *conn.kind.lock().unwrap() = None;
            if let (Some(pool), Some(node)) = (&pool, conn.remote_node.lock().unwrap().clone()) {
                pool.0.lock().unwrap().remove(&node);
            }
            StreamData::Done
        }
    });
    tdo! {
        let sd = frames;
        process_frame(sd)
    }
}

/// Decodes a message and hands it to the continuation point it addresses
fn process_frame(sd: StreamData<String>) -> Trans<()> {
    Trans::new(move |ctx| {
        let (StreamData::More(body) | StreamData::Last(body)) = &sd else { return None };
        let w = decode(body).unwrap_or_else(|| panic!("mread: BAD MESSAGE: error deserializing {body}"));
        process_message(ctx, w);
        None
    })
}

/// Opens a TCP connection to `node` and does the handshake
fn open_socket(my: &Node, node: &Node) -> TcpStream {
    let mut stream = TcpStream::connect((node.host.as_str(), node.port))
        .unwrap_or_else(|e| throw(ConnectionError { msg: format!("no connection: {e}"), node: node.clone() }));
    stream.write_all(handshake_line(my).as_bytes()).unwrap();
    let mut ok = [0u8; 2];
    stream.read_exact(&mut ok).unwrap();
    assert_eq!(&ok, b"OK", "connection refused by {node:?}");
    stream
}

/// Starts the thread that reads the responses of a connection, with that connection in its state
fn watch_connection(ctx: &mut Ctx, conn: &Connection, stream: TcpStream, pool: Arc<Pool>) {
    let old = ctx.get::<Connection>();
    ctx.set(conn.clone());
    fork(read_messages(Arc::new(Mutex::new(BufReader::new(stream))), conn.clone(), Some(pool))).run(ctx);
    match old {
        Some(o) => ctx.set((*o).clone()),
        None => ctx.del::<Connection>(),
    }
}

/// Connects to `node`, or reuses the open connection (`mconnect`)
pub fn mconnect(node: Node) -> Trans<Connection> {
    Trans::new(move |ctx| {
        let pool = (*ctx.get::<Arc<Pool>>().expect("mconnect: use init_node")).clone();
        if let Some(c) = pool.0.lock().unwrap().get(&node) {
            return Some(c.clone());
        }
        let my = ctx.get::<MyNode>().expect("mconnect: use init_node").0.clone();
        let stream = open_socket(&my, &node);
        let conn = Connection {
            id: gen_new_id(),
            my_node: Arc::new(Mutex::new(my)),
            remote_node: Arc::new(Mutex::new(Some(node.clone()))),
            kind: Arc::new(Mutex::new(Some(ConnKind::Net(Arc::new(Mutex::new(stream.try_clone().unwrap())))))),
            calling: true,
            synchronous: false,
        };
        pool.0.lock().unwrap().insert(node.clone(), conn.clone());
        // the responses arrive through the same connection: a thread reads them
        watch_connection(ctx, &conn, stream, pool);
        Some(conn)
    })
}

/// If the other end closed the connection, opens it again (the node may have been restarted). The connection keeps its
/// identity, so what is indexed by it (the last closure reached) is still valid.
pub(crate) fn reconnect_if_closed(ctx: &mut Ctx, conn: &Connection) {
    if conn.kind.lock().unwrap().is_some() {
        return;
    }
    let node = conn.remote_node.lock().unwrap().clone().expect("reconnect: no remote node");
    let pool = (*ctx.get::<Arc<Pool>>().expect("reconnect: use init_node")).clone();
    let my = ctx.get::<MyNode>().expect("reconnect: use init_node").0.clone();
    let stream = open_socket(&my, &node);
    *conn.kind.lock().unwrap() = Some(ConnKind::Net(Arc::new(Mutex::new(stream.try_clone().unwrap()))));
    pool.0.lock().unwrap().insert(node, conn.clone());
    watch_connection(ctx, conn, stream, pool);
}

/// Accepts connections from other nodes and reads their messages (`listen`). It never returns a result.
pub fn listen(node: Node) -> Trans<()> {
    // all the interfaces, whatever the host name of the node (`listenOn` of the original): a node called `localhost` that
    // listened only at the first address of that name would be unreachable for a client that tries the other one
    let listener = Arc::new(TcpListener::bind(("0.0.0.0", node.port)).unwrap_or_else(|e| panic!("resource busy: {node:?}: {e}")));
    println!("Connected to port: \"{}\"", node.port);
    let accepted = wait_events(move || Arc::new(Mutex::new(Some(listener.accept().expect("accept").0))));
    tdo! {
        let stream = accepted;
        serve_connection(stream)
    }
}

/// The first line of a connection tells what it is: a node (`CLOS <cookie> b`) or an HTTP client
fn serve_connection(stream: Arc<Mutex<Option<TcpStream>>>) -> Trans<()> {
    Trans::new(move |ctx| {
        let stream = stream.lock().unwrap().take()?;
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        if !line.starts_with("CLOS") {
            return serve_http(ctx, stream, reader, line);
        }
        // CLOS <cookie> b \r\nHost: h\r\nPort: p\r\n\r\n
        let (mut host, mut port) = (String::new(), 0u16);
        let their_cookie = line.split_whitespace().nth(1).unwrap_or("").to_string();
        if their_cookie != *cluster_cookie().lock().unwrap() {
            let _ = (&stream).write_all(b"NOK");
            panic!("connection attempt with bad cookie");
        }
        loop {
            line.clear();
            reader.read_line(&mut line).ok()?;
            match line.trim_end().split_once(": ") {
                Some(("Host", h)) => host = h.into(),
                Some(("Port", p)) => port = p.parse().expect("port"),
                _ if line.trim().is_empty() => break,
                _ => {}
            }
        }
        (&stream).write_all(b"OK").unwrap();
        let my = ctx.get::<MyNode>().expect("listen: use init_node").0.clone();
        let conn = Connection {
            id: gen_new_id(),
            my_node: Arc::new(Mutex::new(my)),
            remote_node: Arc::new(Mutex::new(Some(Node { host, port }))),
            kind: Arc::new(Mutex::new(Some(ConnKind::Net(Arc::new(Mutex::new(stream)))))),
            calling: false,
            synchronous: false,
        };
        ctx.set(conn.clone());
        read_messages(Arc::new(Mutex::new(reader)), conn, None).run(ctx)
    })
}

/// Serves the requests of an HTTP client: each one goes on in its own thread (`parMany cutHTTPRequest`)
fn serve_http(ctx: &mut Ctx, stream: TcpStream, reader: BufReader<TcpStream>, first_line: String) -> Option<()> {
    let reader = Arc::new(Mutex::new(reader));
    let first = Mutex::new(Some(first_line));
    let requests = parallel(move || match read_request(&mut reader.lock().unwrap(), first.lock().unwrap().take()) {
        Some(r) => StreamData::More(r),
        None => StreamData::Done,
    });
    let stream = Arc::new(HttpSock::new(stream));
    tdo! {
        [stream]
        let sd = requests;
        serve_request(stream, sd)
    }
    .run(ctx)
}

/// Serves one request of the connection, in its own thread
fn serve_request(stream: Arc<HttpSock>, sd: StreamData<HttpRequest>) -> Trans<()> {
    Trans::new(move |ctx| {
        let (StreamData::More(req) | StreamData::Last(req)) = &sd else { return None };
        handle_request(ctx, stream.clone(), req.clone());
        None
    })
}

/// Routes a request to the continuation point it addresses (`listenNew`, the HTTP branch)
fn handle_request(ctx: &mut Ctx, stream: Arc<HttpSock>, req: HttpRequest) {
    let my = ctx.get::<MyNode>().expect("listen: use init_node").0.clone();
    let head = req.head;
    let path = unescape(head.uri.trim_start_matches('/'));
    let first = path.split('/').next().unwrap_or("");
    // the browser asks for the icon of every page: it is served if `./static` has it, and it is a 404 if not (it is not a closure)
    if matches!(first, "" | "file" | "index.html" | "favicon.ico") {
        serve_page(&stream, &path);
        return;
    }
    if matches!(first, "api" | "relay") {
        // the REST api and the relay are not served by this node
        stream.write(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        return;
    }
    let body = if head.method == "POST" { format!("/{}", String::from_utf8_lossy(&req.body)) } else { String::new() };
    let cookie = cookie_of(&head, "state");
    let target = format!("{path}{body}");
    let mut routed = route(&target, cookie.as_deref());
    // a cookie that names a session that this node does not know at all (from another run, or from another node of the same
    // host: the cookies do not depend on the port) is ignored, as if the request had none. The original answers with an error.
    // A known session where the closure does not exist is a consumed endpoint: that one gets no answer.
    if cookie.is_some() && cookie.as_deref() == Some(routed.1.as_str()) && !closures(ctx).knows_session(&routed.1) {
        routed = route(&target, None);
    }
    let (clos_to, sess_to, clos_from, sess_from, rest) = routed;
    let name = format!("httpnode-{}", sess_to.trim_start_matches('#').split('-').next_back().unwrap_or(""));
    let conn = Connection {
        id: gen_new_id(),
        my_node: Arc::new(Mutex::new(my)),
        remote_node: Arc::new(Mutex::new(Some(Node { host: name, port: 0 }))),
        kind: Arc::new(Mutex::new(Some(ConnKind::Http(stream.clone())))),
        calling: false,
        synchronous: false,
    };
    ctx.set(conn);
    ctx.set(head);
    stream.start_response();
    process_message(ctx, Wire { last: false, clos_to, sess_to, clos_from, sess_from, logs: vec![Fragment::text(rest)] });
}
