//! REST endpoints for clients that do not share the code of the program (`Transient.Move.Web`): `minput`, `output`,
//! `moutput`, `publish`, cookies and sessions.
//!
//! `minput ident msg` is an endpoint named `ident`: the HTTP request `GET /ident/<values>` makes the program go on from
//! there, reading the values from the path, and `POST /ident` with a JSON body does the same with the body. What the
//! program sends meanwhile (`msg` of the next `minput`, `output`...) is the response, chunked JSON that stays open. The
//! response of each `minput` also tells the client how to call it (`req`): the URL with a `$type` placeholder for each
//! value, so the client can follow the links (HATEOAS).
//!
//! # The headers of the response
//!
//! The response starts with the status line and two headers but not the blank line that ends them: the cookies that the
//! program sets (`set_cookie`) go inside the headers, and the first thing that is sent that is not a header (a message
//! or the end of the cookies) ends them. So a client that is not sent anything yet does not see the end of the headers.
//! (The original wrote the cookies after the blank line, as the first bytes of the body, where no client reads them as
//! cookies; it was fixed in the same session.)

use super::*;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Instant;

// ---------------------------------------------------------------------------------------------------------------
// the description of an endpoint

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Method {
    GET,
    POST,
}

/// How to call an endpoint: method, URL (with `$type` placeholders), headers and the default body of a POST (`HTTPReq`).
/// The fields are in alphabetical order, which is the order of the JSON that the original writes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpReq {
    pub reqbody: serde_json::Value,
    pub reqheaders: String,
    pub reqtype: Method,
    pub requrl: String,
}

impl HttpReq {
    pub fn new() -> HttpReq {
        HttpReq { reqbody: serde_json::Value::Null, reqheaders: String::new(), reqtype: Method::GET, requrl: String::new() }
    }

    /// `<>` of the original: POST wins, URLs and headers are concatenated, and one of the bodies has to be null
    pub fn append(&self, other: &HttpReq) -> HttpReq {
        use serde_json::Value::Null;
        let reqbody = match (&self.reqbody, &other.reqbody) {
            (Null, v) | (v, Null) => v.clone(),
            _ => panic!("Value monoid"),
        };
        HttpReq {
            reqbody,
            reqheaders: format!("{}{}", self.reqheaders, other.reqheaders),
            reqtype: if self.reqtype == Method::POST || other.reqtype == Method::POST { Method::POST } else { Method::GET },
            requrl: format!("{}{}", self.requrl, other.requrl),
        }
    }
}

impl Default for HttpReq {
    fn default() -> Self {
        HttpReq::new()
    }
}

/// An endpoint published under a key, to be discovered by other users (`InputData`)
#[derive(Clone, Debug)]
pub struct InputData {
    pub ident: String,
    pub message: String,
    pub req: HttpReq,
}

/// The endpoints of the program by name (`Endpoints`), for the console command `endpt`
#[derive(Clone, Default)]
struct Endpoints(std::collections::BTreeMap<String, HttpReq>);

/// The names of the variables of the URL of the endpoint being described, so that they are unique (`Vars`)
#[derive(Clone, Default)]
struct Vars(Vec<String>);

/// A variable name that was not used yet in this URL: the type name, with `x` added until it is unique (`varunique`)
fn var_unique(ctx: &mut Ctx, name: &str) -> String {
    let mut vars = ctx.get::<Vars>().map(|v| v.0.clone()).unwrap_or_default();
    let mut name = name.to_string();
    while vars.contains(&name) {
        name.push('x');
    }
    vars.push(name.clone());
    ctx.set(Vars(vars));
    name
}

/// The type of a value of an endpoint, to describe its URL (`ToHTTPReq`). A `GET` appends `/$type` to the URL; the
/// types that are sent in the body of a `POST` are `PostData`.
pub trait ToHttpReq: Loggable + 'static {
    /// The name of the variable of the URL: the lowercase name of the type
    fn type_name() -> String;

    /// Reads the value from the path of the request. The values of a tuple are separated by `/` there (`/pair/3/hello`),
    /// while in the log a tuple is JSON.
    fn from_path(input: &mut &str) -> Option<Self> {
        Self::deserialize(input)
    }

    /// Writes the value as `from_path` reads it
    fn to_path(&self, out: &mut String) {
        self.serialize(out)
    }

    fn to_http_req(ctx: &mut Ctx) -> HttpReq {
        let v = var_unique(ctx, &format!("${}", Self::type_name()));
        HttpReq { requrl: format!("/{v}"), ..HttpReq::new() }
    }
}

macro_rules! to_http_req_as {
    ($($t:ty => $name:expr),* $(,)?) => {$(
        impl ToHttpReq for $t {
            fn type_name() -> String { $name.into() }
        }
    )*};
}

// `Int` and `Integer` of Haskell; the rest of the integers have the name of the type
to_http_req_as!(i64 => "int", i128 => "integer", i32 => "int32", u32 => "word32", u64 => "word64", u16 => "word16", u8 => "word8", usize => "word", f64 => "double", bool => "bool", char => "char");

impl ToHttpReq for () {
    fn type_name() -> String {
        "u".into()
    }
    fn to_http_req(_: &mut Ctx) -> HttpReq {
        HttpReq { requrl: "/u".into(), ..HttpReq::new() }
    }
}

impl ToHttpReq for String {
    fn type_name() -> String {
        "string".into()
    }
}

impl<A: ToHttpReq + serde::Serialize + serde::de::DeserializeOwned> ToHttpReq for Vec<A> {
    fn type_name() -> String {
        format!("[{}]", A::type_name())
    }
    fn to_http_req(ctx: &mut Ctx) -> HttpReq {
        let open = HttpReq { requrl: "[".into(), ..HttpReq::new() };
        let close = HttpReq { requrl: "]".into(), ..HttpReq::new() };
        open.append(&A::to_http_req(ctx)).append(&close)
    }
}

macro_rules! to_http_req_tuple {
    ($(($($t:ident $v:ident),+))*) => {$(
        impl<$($t: ToHttpReq + serde::Serialize + serde::de::DeserializeOwned),+> ToHttpReq for ($($t,)+) {
            fn type_name() -> String { format!("({})", [$($t::type_name()),+].join(",")) }
            fn from_path(input: &mut &str) -> Option<Self> {
                let mut first = true;
                Some(($({
                    if !std::mem::take(&mut first) {
                        *input = input.trim_start().strip_prefix('/')?;
                    }
                    $t::from_path(input)?
                },)+))
            }
            fn to_path(&self, out: &mut String) {
                let ($($v,)+) = self;
                let mut first = true;
                $({
                    if !std::mem::take(&mut first) { out.push('/'); }
                    $v.to_path(out);
                })+
            }
            fn to_http_req(ctx: &mut Ctx) -> HttpReq {
                let mut req = HttpReq::new();
                $(req = req.append(&$t::to_http_req(ctx));)+
                req
            }
        }
    )*};
}
to_http_req_tuple!((A a, B b) (A a, B b, C c) (A a, B b, C c, D d));

/// A value that comes in the body of a `POST`, as JSON (`POSTData`). The default value of the type is the body that
/// `minput` offers to the client as a pattern.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PostData<A>(pub A);

impl<A: Serialize + serde::de::DeserializeOwned> Loggable for PostData<A> {
    fn serialize(&self, out: &mut String) {
        Json::serialize_ref(&self.0, out)
    }
    fn deserialize(input: &mut &str) -> Option<Self> {
        Json::<A>::deserialize(input).map(|j| PostData(j.0))
    }
}

impl<A: Default + Serialize + serde::de::DeserializeOwned + 'static> ToHttpReq for PostData<A> {
    fn type_name() -> String {
        "postdata".into()
    }
    fn to_http_req(ctx: &mut Ctx) -> HttpReq {
        ctx.set(Vars(Vec::new()));
        HttpReq { reqtype: Method::POST, reqbody: serde_json::to_value(A::default()).unwrap(), ..HttpReq::new() }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// cookies and sessions

/// The cookies to send to the client with the next message (`Cookies`)
#[derive(Clone, Default)]
struct Cookies(Vec<(String, String)>);

/// Sets a cookie in the client. The value can have the options: `"value; Path=/; HttpOnly"` (`setCookie`)
pub fn set_cookie(name: &str, value_and_params: &str) -> Trans<()> {
    let (name, value) = (name.to_string(), value_and_params.to_string());
    Trans::new(move |ctx| {
        let mut cs = ctx.get::<Cookies>().map(|c| c.0.clone()).unwrap_or_default();
        cs.insert(0, (name.clone(), value.clone()));
        ctx.set(Cookies(cs));
        Some(())
    })
}

/// A cookie of the request being served (`getCookie`)
pub fn get_cookie(name: &str) -> Trans<Option<String>> {
    let name = name.to_string();
    Trans::new(move |ctx| Some(ctx.get::<HttpHeaders>().and_then(|h| cookie_of(&h, &name))))
}

/// Sends the pending `Set-Cookie` lines to the connection, raw (see the notes of the module)
fn send_cookies(ctx: &mut Ctx, conn: &Connection) {
    let cs = ctx.get::<Cookies>().map(|c| c.0.clone()).unwrap_or_default();
    if !cs.is_empty() {
        ctx.set(Cookies(Vec::new()));
        msend(conn, &cs.iter().map(|(n, v)| format!("Set-Cookie: {n}={v}\r\n")).collect::<String>());
        // the cookies are the last of the headers if nothing else is sent: the response may have no body yet
        if let Some(ConnKind::Http(s)) = &*conn.kind.lock().unwrap() {
            s.close_headers();
        }
    }
}

/// A new session identifier: five lowercase letters (`genSessionId`)
pub fn gen_session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEED: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let mut x = t ^ SEED.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed) ^ (gen_new_id() as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93);
    (0..5)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (b'a' + (x % 26) as u8) as char
        })
        .collect()
}

/// The session of the path, if it is not set yet, is `session` (`setSession`)
fn set_session(ctx: &mut Ctx, session: &str) {
    if let Some(p) = ctx.get::<PrevClos>() {
        if p.new_session.is_none() {
            ctx.set(PrevClos { new_session: Some(session.into()), ..(*p).clone() });
        }
    }
}

/// The session of the client: the `state` cookie, or a new one that is sent as a cookie (`initSession`). It is logged, so
/// a replay finds the same session.
fn init_session(id_session: String) -> Trans<()> {
    if id_session != "0" {
        return pure(());
    }
    let from_cookie_and_log = logged(Trans::new(|ctx| {
        let headers = ctx.get::<HttpHeaders>();
        if headers.as_ref().map_or(true, |h| h.headers.is_empty()) {
            panic!("initSession: no Headers");
        }
        match get_cookie("state").run(ctx)? {
            Some(c) if !c.is_empty() => Some(c),
            _ => {
                let s = gen_session_id();
                set_cookie("state", &format!("{s}; Path=/")).run(ctx);
                Some(s)
            }
        }
    }));
    or(
        tdo! {
            let session = from_cookie_and_log;
            set_session_of(session)
        },
        pure(()),
    )
}

/// Sets the session of the path (`setSession`)
fn set_session_of(session: String) -> Trans<()> {
    Trans::new(move |ctx| {
        set_session(ctx, &session);
        Some(())
    })
}

/// The state of the computations by session, kept for an hour: the state of a user that publishes an endpoint, for the
/// user that invokes it (`storeSessionData`)
fn session_store() -> &'static Mutex<HashMap<String, (Arc<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>, Instant)>> {
    static S: OnceLock<Mutex<HashMap<String, (Arc<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>, Instant)>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Keeps the state of this computation under the session of the closure, to be found when a link is followed
/// (`setSessionData`). The headers of the request are not kept.
fn set_session_data(ctx: &Ctx) {
    let session = ctx.get::<PrevClos>().map(|p| get_sess_closure(&p.prev).0).unwrap_or_else(|| "0".into());
    let mut data = (*ctx.data).clone();
    data.remove(&TypeId::of::<HttpHeaders>());
    session_store().lock().unwrap().insert(session, (Arc::new(data), Instant::now()));
}

/// The state kept for the session of the `state` cookie, which is taken out (`getSessionData`). Nothing after an hour.
fn get_session_data(ctx: &Ctx) -> Option<Arc<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>> {
    let mut store = session_store().lock().unwrap();
    store.retain(|_, (_, created)| created.elapsed() < std::time::Duration::from_secs(3600));
    let key = ctx.get::<HttpHeaders>().and_then(|h| cookie_of(&h, "state"))?;
    store.remove(&key).map(|(data, _)| data)
}

/// Merges the state of the computation that created the endpoint (`stored`) with the state of the one that comes: the
/// stored state wins, except that the path goes on from the current one in the session of the stored one
fn merge_session(ctx: &mut Ctx, stored: &HashMap<TypeId, Arc<dyn Any + Send + Sync>>) {
    let current = ctx.data.clone();
    let mut merged: HashMap<TypeId, Arc<dyn Any + Send + Sync>> = (*current).clone();
    for (k, v) in stored {
        merged.insert(*k, v.clone());
    }
    if let (Some(x), Some(y)) = (stored.get(&TypeId::of::<PrevClos>()), current.get(&TypeId::of::<PrevClos>())) {
        let x = x.clone().downcast::<PrevClos>().unwrap();
        let y = y.clone().downcast::<PrevClos>().unwrap();
        merged.insert(TypeId::of::<PrevClos>(), Arc::new(PrevClos { new_session: Some(get_sess_closure(&x.prev).0), ..(*y).clone() }));
    }
    ctx.data = Arc::new(merged);
}

/// A value as it comes in the path of a request (`ToHttpReq::from_path`), to read it with `logged`
struct ByPath<A>(A);

impl<A: ToHttpReq> Loggable for ByPath<A> {
    fn serialize(&self, out: &mut String) {
        self.0.to_path(out)
    }
    fn deserialize(input: &mut &str) -> Option<Self> {
        A::from_path(input).map(ByPath)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// minput and the output

/// A composable web input: an endpoint `ident`. `msg` goes to the client (JSON) as the response for this step, with how
/// to call the endpoint. The value is read from the path of the request (`GET`) or from the body (`POST`, with
/// `PostData`) (`minput`).
///
/// ```text
/// GET /ident/7                          ->   the program goes on with 7
/// { "msg":..., "req":{"reqbody":null,"reqheaders":"","reqtype":"GET","requrl":"http://host:port/ident/$int"}}
/// ```
pub fn minput<A, P>(ident: &str, msg: P) -> Cloud<A>
where
    A: Loggable + ToHttpReq + Clone + Send + Sync + 'static,
    P: Serialize,
{
    let (ident, msg) = (ident.to_string(), serde_json::to_string(&msg).unwrap());
    on_all(sandbox_data::<ClosToRespond, _>(Trans::new(move |ctx| {
        ctx.del::<ClosToRespond>();
        ctx.mode = if ctx.mode == Mode::Remote { Mode::Remote } else { Mode::Parallel };
        local(minput_step::<A>(ident.clone(), msg.clone())).un_cloud().run(ctx)
    })))
}

/// `minput` with the message first: in a sequence of inputs it is more natural to answer the previous one (`followup`)
pub fn followup<A, P>(msg: P, ident: &str) -> Cloud<A>
where
    A: Loggable + ToHttpReq + Clone + Send + Sync + 'static,
    P: Serialize,
{
    minput(ident, msg)
}

fn minput_step<A>(ident: String, msg: String) -> Trans<A>
where
    A: Loggable + ToHttpReq + Clone + Send + Sync + 'static,
{
    Trans::new(move |ctx| {
        let log = ctx.log.clone();
        // if the connection is not available, the alternative computation runs
        let conn = (*ctx.get::<Connection>()?).clone();
        let mynode = ctx.get::<MyNode>().expect("minput: use init_node").0.clone();
        let prev = ctx.get::<PrevClos>().expect("minput: no execution state. Use init_node");
        let id_session = prev.new_session.clone().unwrap_or_else(|| get_sess_closure(&prev.prev).0);
        let urlbase = format!("http://{}:{}/{}", mynode.host, mynode.port, ident);

        let params = A::to_http_req(ctx);
        let httpreq = HttpReq { requrl: urlbase.clone(), ..HttpReq::new() }.append(&params);
        let httpreqsess = HttpReq { requrl: format!("{urlbase}/S{id_session}"), ..HttpReq::new() }.append(&params);

        // publish immediately if we are inside a publish block
        let (visible, keys) = ctx.get::<Publishing>().map(|p| (p.visible, p.keys.clone())).unwrap_or((true, Vec::new()));
        let idata = InputData { ident: ident.clone(), message: msg.clone(), req: httpreqsess };
        for k in &keys {
            published_map().lock().unwrap().entry(k.clone()).or_default().insert(0, idata.clone());
        }

        // the endpoints are in a cell shared with the console command `endpt`, if it was created before
        let mut endpts = get_rstate::<Endpoints>().run(ctx).map(|e| e.0).unwrap_or_default();
        endpts.insert(ident.clone(), httpreq.clone());
        set_rstate(Endpoints(endpts)).run(ctx);

        let pstring = give_parse_string().run(ctx)?;
        let is_self = matches!(*conn.kind.lock().unwrap(), Some(ConnKind::Self_));
        let tosend = format!("{{ \"msg\":{msg}, \"req\":{}}}", serde_json::to_string(&httpreq).unwrap());
        let (conn_send, ident2) = (conn.clone(), ident.clone());

        // what is sent to the client before waiting for the request
        if !log.recover || pstring.is_empty() {
            if is_self {
                // (string "e/" >> string "e/") <|> return "e/"
                or(tdo! { string("e/"); string("e/") }, pure("e/".to_string())).run(ctx);
            } else {
                send_cookies(ctx, &conn_send);
                if visible && !log.recover {
                    msend(&conn_send, &chunk(&tosend));
                }
            }
        }
        let read_response = logged(Trans::<ByPath<A>>::new(|_| panic!("minput: insuficient parameters"))).map(|b| b.0);
        // (the names of the variables of the URL are kept: the next input of this session gets `$intx` if it is also an `Int`)
        tdo! {
            [read_response, id_session]
            endpoint_etc(ident2);
            let r = read_response;
            init_session(id_session);
            pure(r)
        }
        .run(ctx)
    })
}

/// The endpoint with the name of the input and, if another session's computation left its state, its merge with this
/// one's (`endpointEtc`)
fn endpoint_etc(ident: String) -> Trans<()> {
    tdo! {
        endpoint(Some(ident));
        merge_stored_session()
    }
}

/// If another session's computation left its state, merges it with this one's
fn merge_stored_session() -> Trans<()> {
    Trans::new(|ctx| {
        if let Some(stored) = get_session_data(ctx) {
            merge_session(ctx, &stored);
        }
        Some(())
    })
}

/// Sends JSON to the client as a chunk of the response (`output`)
pub fn output<A: Serialize>(x: &A) -> Trans<()> {
    send_fragment(serde_json::to_string(x).unwrap())
}

/// The last output of a flow, or a stream to the client: the response is chunked, and it stays open until every `minput`
/// and `moutput` have finished (`moutput`)
pub fn moutput<A: Serialize>(x: &A) -> Cloud<()> {
    local(output(x))
}

/// Sends a raw JSON fragment over the current chunked HTTP connection (`sendFragment`)
fn send_fragment(tosend: String) -> Trans<()> {
    Trans::new(move |ctx| {
        let conn = (*ctx.get::<Connection>()?).clone();
        msend(&conn, &chunk(&tosend));
        Some(())
    })
}

// ---------------------------------------------------------------------------------------------------------------
// publishing

/// Whether the endpoints of the block are shown to the client that creates them, and the keys they are published under
#[derive(Clone)]
struct Publishing {
    visible: bool,
    keys: Vec<String>,
}

fn published_map() -> &'static Mutex<HashMap<String, Vec<InputData>>> {
    static P: OnceLock<Mutex<HashMap<String, Vec<InputData>>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// Makes the `minput`s of `inp` discoverable under `key`, so that other users can use them (`publish`)
pub fn publish<A: Loggable + Clone + Send + Sync + 'static>(visible: bool, key: &str, inp: Cloud<A>) -> Cloud<A> {
    publishn(visible, &[key], inp)
}

/// `publish` under several keys (`publishn`)
pub fn publishn<A: Loggable + Clone + Send + Sync + 'static>(visible: bool, keys: &[&str], inp: Cloud<A>) -> Cloud<A> {
    let keys: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
    // a job to force the execution, so that the endpoint is in the map of published ones
    cdo! {
        [inp, keys]
        job();
        on_all(sandbox_data::<Publishing, _>(Trans::new({
            let (inp, keys) = (inp.clone(), keys.clone());
            move |ctx| {
                ctx.set(Publishing { visible, keys: keys.clone() });
                inp.run(ctx)
            }
        })))
    }
}

/// Sends to the client the endpoints published under `key`. Call it at the start of a session to show what is pending
/// (`published`)
pub fn published(key: &str) -> Cloud<()> {
    let key = key.to_string();
    local(Trans::new(move |ctx| {
        if !ctx.log.recover {
            let datas = published_map().lock().unwrap().get(&key).cloned().unwrap_or_default();
            for d in datas {
                send_option(ctx, &d.message, &d.req);
            }
        }
        None
    }))
}

/// Sends one discoverable endpoint to the client, keeping the state of this computation so that following the link
/// restores the context (`sendOption`)
fn send_option(ctx: &mut Ctx, msg: &str, req: &HttpReq) {
    set_session_data(ctx);
    if let Some(conn) = ctx.get::<Connection>() {
        msend(&conn, &chunk(&format!("{{ \"msg\":{msg}, \"req\":{}}}", serde_json::to_string(req).unwrap())));
    }
}

/// The URL of the current point of the computation (`getURL`)
pub fn get_url() -> Trans<String> {
    Trans::new(|ctx| {
        let (sess, clos) = ctx.get::<PrevClos>().map_or(("0".to_string(), "0".to_string()), |p| get_sess_closure(&p.prev));
        let n = ctx.get::<MyNode>().expect("getURL: use init_node").0.clone();
        Some(format!("http://{}:{}/{}/S{}/{}", n.host, n.port, clos, sess, ctx.log.part_log.render()))
    })
}

/// Prints the URL of the current point (`showURL`)
pub fn show_url() -> Trans<()> {
    tdo! {
        let u = get_url();
        lift_io(move || println!("{u}"))
    }
}

/// The console command `endpt`: how to invoke each endpoint of the program (`optionEndpoints`)
pub fn option_endpoints<A: Loggable + Clone + Send + Sync + 'static>() -> Cloud<A> {
    // the cell of the endpoints is created first, so that it is shared with the code that comes after
    cdo! {
        on_all_deep(new_rstate(Endpoints::default()).map(|_| ()));
        option_endpoint_command()
    }
}

fn option_endpoint_command<A: Send + 'static>() -> Cloud<A> {
    local(tdo! {
        option("endpt", "info about a endpoint");
        let endpts = list_endpoints();
        let ident = input::<String>("enter the endpoint for which you want to know the interface");
        show_how_to_call(endpts, ident)
    })
    .then_empty()
}

/// Prints the names of the endpoints of the program and gives their descriptions
fn list_endpoints() -> Trans<std::collections::BTreeMap<String, HttpReq>> {
    Trans::new(|ctx| {
        let endpts = get_rstate::<Endpoints>().run(ctx).map(|e| e.0).unwrap_or_default();
        println!("endpoints available: {:?}", endpts.keys().collect::<Vec<_>>());
        Some(endpts)
    })
}

/// Prints the `curl` command that invokes the endpoint `ident`
fn show_how_to_call(endpts: std::collections::BTreeMap<String, HttpReq>, ident: String) -> Trans<()> {
    lift_io(move || match endpts.get(&ident) {
        None => println!("No such endpoint: {ident}"),
        Some(r) => {
            println!("\nYou can invoque this endpoint with:");
            let post = if r.reqtype == Method::GET {
                String::new()
            } else {
                format!("-H 'content-type: application/json' -XPOST -d \"{}\" ", serde_json::to_string(&r.reqbody).unwrap())
            };
            println!("\ncurl {post}{}", r.requrl);
        }
    })
}

// ---------------------------------------------------------------------------------------------------------------
// JSON as a log value (`Transient.Move.JSON`)

/// A value that is written to the log as JSON (`AsJSON`)
pub type AsJson<A> = Json<A>;

/// The JSON of a value, as it is written in the log (`serializeToJSON`)
pub fn serialize_to_json<A: Serialize + serde::de::DeserializeOwned>(x: &A) -> String {
    let mut out = String::new();
    Json::serialize_ref(x, &mut out);
    out
}

/// Reads a JSON value from the start of `input` (`deserializeJSON`)
pub fn deserialize_json<A: Serialize + serde::de::DeserializeOwned>(input: &mut &str) -> Option<A> {
    Json::<A>::deserialize(input).map(|j| j.0)
}

// ---------------------------------------------------------------------------------------------------------------
// HTTP client

/// Sends a raw HTTP request to `node` and reads the response as a value of type `A` (`rawHTTP`). It runs in a new thread. The
/// body of a chunked response is a stream: each chunk is a value and the rest of the computation runs for each one.
/// Anything but a 2xx status is an error that contains the request, the body and the headers.
///
/// ```text
/// raw_http::<String>(node, "GET /enterw/alice HTTP/1.1\r\nHost: localhost\r\n\r\n")
/// ```
pub fn raw_http<A: Loggable + Send + 'static>(node: Node, request: &str) -> Trans<A> {
    let request = request.to_string();
    tdo! {
        [node, request]
        abduce();
        raw_http_request(node, request)
    }
}

fn raw_http_request<A: Loggable + Send + 'static>(node: Node, request: String) -> Trans<A> {
    use std::io::{BufRead, BufReader, Read, Write};
    {
        Trans::new(move |ctx| {
            let mut stream = std::net::TcpStream::connect((node.host.as_str(), node.port))
                .unwrap_or_else(|e| throw(ConnectionError { msg: format!("no connection: {e}"), node: node.clone() }));
            stream.write_all(request.as_bytes()).unwrap();
            let mut reader = BufReader::new(stream);
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            let code = first.split_whitespace().nth(1).unwrap_or("").to_string();
            if first.is_empty() {
                panic!("No HTTP header received");
            }
            let mut headers = Vec::new();
            loop {
                let mut l = String::new();
                reader.read_line(&mut l).unwrap();
                let l = l.trim_end();
                if l.is_empty() {
                    break;
                }
                if let Some((k, v)) = l.split_once(':') {
                    headers.push((k.trim().to_string(), v.trim().to_string()));
                }
            }
            let head = HttpHeaders { method: String::new(), uri: String::new(), version: first.trim_end().into(), headers };
            let chunked = head.get("Transfer-Encoding") == Some("chunked");
            let read_body = |reader: &mut BufReader<std::net::TcpStream>| -> Vec<String> {
                if chunked {
                    let mut chunks = Vec::new();
                    loop {
                        let mut size = String::new();
                        if reader.read_line(&mut size).unwrap_or(0) == 0 {
                            break;
                        }
                        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        let mut buf = vec![0u8; n];
                        reader.read_exact(&mut buf).unwrap();
                        let mut crlf = [0u8; 2];
                        let _ = reader.read_exact(&mut crlf);
                        chunks.push(String::from_utf8_lossy(&buf).into_owned());
                    }
                    chunks
                } else {
                    let mut body = Vec::new();
                    match head.get("Content-Length").and_then(|l| l.parse::<usize>().ok()) {
                        Some(n) => {
                            body = vec![0u8; n];
                            reader.read_exact(&mut body).unwrap();
                        }
                        None => {
                            reader.read_to_end(&mut body).unwrap();
                        }
                    }
                    vec![String::from_utf8_lossy(&body).into_owned()]
                }
            };
            let bodies = read_body(&mut reader);
            if !code.starts_with('2') {
                panic!("Transient.Move.Web: ERROR in REQUEST: \n{request}{}\nRESPONSE HEADERS:\n {head:?}", bodies.concat());
            }
            let value = |b: &str| from_log::<A>(b).map(|(v, _)| v).unwrap_or_else(|| panic!("error parsing <{b}> to {}", std::any::type_name::<A>()));
            let mut values = bodies.iter().map(|b| value(b));
            let last = values.next_back();
            // every chunk but the last goes on in this thread, one after another; the last one is the result
            for v in values {
                run_cont_loop(ctx, Box::new(v));
            }
            last
        })
    }
}
