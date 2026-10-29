//! REST endpoints with `minput` (`Transient.Move.Web`), against the behaviour of the Haskell original: the responses
//! compared here were captured from the Haskell programs `MultiUser.hs` and `AppMinput.hs` with curl.

mod common;
use common::*;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// A GET request; the response is read for `wait` ms, since it is chunked and stays open (like `curl --raw -sN -m 2`)
fn get(port: u16, path: &str, cookie: Option<&str>, wait: u64) -> String {
    request(port, "GET", path, cookie, "", wait)
}

/// A request with a body (`POST`)
fn request(port: u16, method: &str, path: &str, cookie: Option<&str>, body: &str, wait: u64) -> String {
    // the node prints READY after a fixed delay: under load it may not be listening yet
    let mut s = (0..30).find_map(|_| TcpStream::connect(("127.0.0.1", port)).ok().or_else(|| { std::thread::sleep(Duration::from_millis(100)); None })).expect("the node listens");
    let cookie = cookie.map(|c| format!("Cookie: state={c}\r\n")).unwrap_or_default();
    let body_headers = if body.is_empty() { String::new() } else { format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()) };
    write!(s, "{method} /{path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{cookie}{body_headers}\r\n{body}").unwrap();
    s.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let (mut out, t0) = (Vec::new(), Instant::now());
    let mut buf = [0u8; 4096];
    while t0.elapsed() < Duration::from_millis(wait) {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(_) => {}
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn cookie_of(response: &str) -> String {
    let i = response.find("state=").expect("a cookie in the response") + 6;
    response[i..].chars().take_while(|c| c.is_ascii_lowercase()).collect()
}

fn tmp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("transient-web-{name}-{}", std::process::id()))
}

/// The headers of a response whose first message is sent: they are closed by it
const HEAD: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n";
/// The headers while nothing is sent: they are open, so that the cookies can still be added to them
const HEAD_OPEN: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n";

/// A message as the node sends it: its length in hexadecimal, the message and the line break (the length depends on the
/// number of digits of the port in the URL)
fn chunked(message: &str) -> String {
    format!("{:X}\r\n{message}\r\n", message.len())
}

#[test]
fn multiuser_follows_the_responses_of_the_original() {
    let dir = tmp("multiuser");
    let port = free_port();
    let (_node, rx) = spawn_demo("web_demo", &dir, port, "multiuser");
    wait_line(&rx, "READY");

    // the first call creates the session: the cookie is in the headers, and the message of the next input follows
    let r1 = get(port, "enterw/alice", None, 800);
    let alice = cookie_of(&r1);
    assert_eq!(alice.len(), 5);
    let m1 = format!("{{ \"msg\":\"play guessgame\", \"req\":{{\"reqbody\":null,\"reqheaders\":\"\",\"reqtype\":\"GET\",\"requrl\":\"http://127.0.0.1:{port}/guessgame/u\"}}}}");
    assert_eq!(r1, format!("{HEAD_OPEN}Set-Cookie: state={alice}; Path=/\r\n\r\n{}", chunked(&m1)));

    // each response carries the message of the next input and how to call it
    let r2 = get(port, "guessgame/u", Some(&alice), 800);
    let m2 = format!("{{ \"msg\":\"alice: lock a number to guess it\", \"req\":{{\"reqbody\":null,\"reqheaders\":\"\",\"reqtype\":\"GET\",\"requrl\":\"http://127.0.0.1:{port}/enterlock/$int\"}}}}");
    assert_eq!(r2, format!("{HEAD}{}", chunked(&m2)));
    let r3 = get(port, "enterlock/7", Some(&alice), 800);
    let m3 = format!("{{ \"msg\":\"guess the number entered by: alice\", \"req\":{{\"reqbody\":null,\"reqheaders\":\"\",\"reqtype\":\"GET\",\"requrl\":\"http://127.0.0.1:{port}/enterguess/$intx\"}}}}");
    assert_eq!(r3, format!("{HEAD}{}", chunked(&m3)));

    // bob invokes the endpoint that alice created, from his own session
    let bob = cookie_of(&get(port, "enterw/bob", None, 800));
    get(port, "guessgame/u", Some(&bob), 800);
    let r = get(port, &format!("enterguess/S{alice}/3"), Some(&bob), 800);
    assert!(r.starts_with(HEAD), "{r:?}");
    let carol = cookie_of(&get(port, "enterw/carol", None, 800));
    get(port, "guessgame/u", Some(&carol), 800);
    get(port, &format!("enterguess/S{alice}/7"), Some(&carol), 800);

    let mut seen = Vec::new();
    while let Ok(l) = rx.recv_timeout(Duration::from_millis(500)) {
        seen.push(l);
    }
    assert!(seen.contains(&"GUESS alice (7,3)".to_string()) && seen.contains(&"GUESS alice (7,7)".to_string()), "{seen:?}");
    assert!(!seen.iter().any(|l| l.to_lowercase().contains("error")), "{seen:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// AppMinput.hs: two inputs in an applicative, called in either order, resolve with the values in the order of the terms
#[test]
fn an_applicative_of_two_inputs_resolves_in_any_order() {
    for (i, first) in ["t1", "t2"].iter().enumerate() {
        let dir = tmp(&format!("appminput{i}"));
        let port = free_port();
        let (_node, rx) = spawn_demo("web_demo", &dir, port, "appminput");
        wait_line(&rx, "READY");
        let (a, b) = if *first == "t1" { ("t1/hello", "t2/world") } else { ("t2/world", "t1/hello") };
        let r1 = get(port, a, None, 600);
        let r2 = get(port, b, None, 600);
        // the first call has nothing to answer until the other one comes: its headers stay open. The second one gets the
        // cookie of the new session and the message of the next input.
        assert_eq!(r1, HEAD_OPEN, "{first}: {r1:?}");
        let message = format!("{{ \"msg\":[\"RESULT\",\"helloworld\"], \"req\":{{\"reqbody\":null,\"reqheaders\":\"\",\"reqtype\":\"GET\",\"requrl\":\"http://127.0.0.1:{port}/final/u\"}}}}");
        assert!(r2.starts_with(&format!("{HEAD_OPEN}Set-Cookie: state=")) && r2.contains("; Path=/\r\n\r\n"), "{first}: {r2:?}");
        assert!(r2.ends_with(&format!("; Path=/\r\n\r\n{}", chunked(&message))), "{first}: {r2:?}");
        assert_eq!(wait_line(&rx, "MINPUT RESOLVED"), "MINPUT RESOLVED helloworld");
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// mur.sh: alice creates the endpoint `enterguess` and the process ends (SIGTERM, which saves the state because the
/// published endpoint has a job). The process is started again in the same directory and the endpoint is invoked by alice's
/// session, by another session (bob) and by nobody in particular (no cookie).
fn restart_and_invoke(variant: &str) {
    let dir = tmp(&format!("mur-{variant}"));
    let port = free_port();
    let (mut n1, rx1) = spawn_demo("web_demo", &dir, port, "multiuser");
    wait_line(&rx1, "READY");
    let alice = cookie_of(&get(port, "enterw/alice", None, 800));
    get(port, "guessgame/u", Some(&alice), 800);
    get(port, "enterlock/7", Some(&alice), 800);
    n1.signal("TERM");
    n1.wait_exit(10).expect("the node ends with the signal");

    // RESTART_FORGET=1 starts the new process with an empty directory: the test must then fail (negative control)
    let dir2 = if std::env::var("RESTART_FORGET").is_ok() { dir.join("empty") } else { dir.clone() };
    let (_n2, rx2) = spawn_demo("web_demo", &dir2, port, "multiuser");
    wait_line(&rx2, "READY");
    let response = match variant {
        "alice" => get(port, "enterguess/7", Some(&alice), 1500),
        "bob" => {
            let bob = cookie_of(&get(port, "enterw/bob", None, 800));
            get(port, &format!("enterguess/S{alice}/7"), Some(&bob), 1500)
        }
        _ => get(port, &format!("enterguess/S{alice}/7"), None, 1500),
    };
    let mut seen = Vec::new();
    while let Ok(l) = rx2.recv_timeout(Duration::from_millis(500)) {
        seen.push(l);
    }
    assert!(seen.contains(&"GUESS alice (7,7)".to_string()), "{variant}: {seen:?}");
    assert!(response.contains("entered number"), "{variant}: {response:?}");
    assert!(!seen.iter().any(|l| l.to_lowercase().contains("error")), "{variant}: {seen:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_restarted_node_serves_the_endpoint_to_the_same_session() {
    restart_and_invoke("alice");
}

#[test]
fn a_restarted_node_serves_the_endpoint_to_another_session() {
    restart_and_invoke("bob");
}

#[test]
fn a_restarted_node_serves_the_endpoint_without_a_cookie() {
    restart_and_invoke("nocookie");
}

/// WebPost.hs: a POST body, a tuple and a list. The responses are the ones that the Haskell original gave (captured with curl).
#[test]
fn inputs_of_different_types_describe_their_urls_like_the_original() {
    let dir = tmp("post");
    let port = free_port();
    let (_node, rx) = spawn_demo("web_demo", &dir, port, "post");
    wait_line(&rx, "READY");
    let chunk = |msg: &str, url: &str| {
        let body = format!("{{ \"msg\":\"{msg}\", \"req\":{{\"reqbody\":null,\"reqheaders\":\"\",\"reqtype\":\"GET\",\"requrl\":\"http://127.0.0.1:{port}/{url}\"}}}}");
        format!("{HEAD}{:X}\r\n{body}\r\n", body.len())
    };

    let c = cookie_of(&get(port, "start/u", None, 800));
    assert_eq!(get(port, "start/u", Some(&c), 800), HEAD_OPEN);
    // the body of a POST is the value; the response describes the next input, a tuple of an Int and a String
    let r = request(port, "POST", "endp", Some(&c), "5", 800);
    assert_eq!(r, chunk("pair", "pair/$int/$string"));
    // a list: its URL has the brackets, and the variable is unique in the session
    assert_eq!(get(port, "pair/3/hello", Some(&c), 800), chunk("a list", "list[/$intx]"));
    assert_eq!(get(port, "list/%5B1,2,3%5D", Some(&c), 800), chunk("done", "end/u"));

    let mut seen = Vec::new();
    while let Ok(l) = rx.recv_timeout(Duration::from_millis(500)) {
        seen.push(l);
    }
    assert!(seen.contains(&"GOT 5".to_string()) && seen.contains(&"PAIR (3,\"hello\")".to_string()) && seen.contains(&"LIST [1,2,3]".to_string()), "{seen:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// `raw_http` against the node itself: the body of a chunked response, and an error status
#[test]
fn raw_http_reads_the_responses() {
    use transient_core::*;
    let dir = tmp("rawhttp");
    let port = free_port();
    // a server of one response, to see what the client reads
    let server = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    std::thread::spawn(move || {
        for (i, conn) in server.incoming().take(2).enumerate() {
            let mut s = conn.unwrap();
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let reply = if i == 0 {
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n42\r\n1\r\n7\r\n0\r\n\r\n"
            } else {
                "HTTP/1.1 404 Not Found\r\nContent-Length: 3\r\n\r\nbad"
            };
            s.write_all(reply.as_bytes()).unwrap();
        }
    });
    let node = create_node("127.0.0.1", port);
    let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let o = out.clone();
    // each chunk is a value, and the rest of the computation runs for each
    let t = tdo! {
        [o]
        let v = raw_http::<i64>(node.clone(), "GET /x HTTP/1.1\r\nHost: h\r\n\r\n");
        lift_io(move || o.lock().unwrap().push(v))
    };
    t.run(&mut Ctx::new());
    let t0 = Instant::now();
    while out.lock().unwrap().len() < 2 && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut got = out.lock().unwrap().clone();
    got.sort();
    assert_eq!(got, vec![7, 42]);

    // anything but 2xx is an error with the request and the body
    let errors = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let e = errors.clone();
    let t = tdo! {
        on_exception(move |x: SomeException| {
            let e = e.clone();
            lift_io(move || e.lock().unwrap().push(x.to_string()))
        });
        raw_http::<String>(create_node("127.0.0.1", port), "GET /y HTTP/1.1\r\n\r\n").map(|_| ())
    };
    t.run(&mut Ctx::new());
    let t0 = Instant::now();
    while errors.lock().unwrap().is_empty() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let errors = errors.lock().unwrap().clone();
    assert!(errors.iter().any(|m| m.contains("ERROR in REQUEST") && m.contains("GET /y") && m.contains("bad")), "{errors:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// The console command `endpt` tells how to call each endpoint (`optionEndpoints`)
#[test]
fn the_console_tells_how_to_call_an_endpoint() {
    let dir = tmp("endpt");
    let port = free_port();
    let (mut node, rx) = spawn_demo("web_demo", &dir, port, "endpt");
    wait_line(&rx, "READY");
    node.type_line("endpt");
    node.type_line("enterw");
    assert_eq!(wait_line(&rx, "endpoints available:"), "endpoints available: [\"enterw\"]");
    assert_eq!(wait_line(&rx, "curl "), format!("curl http://127.0.0.1:{port}/enterw/$string"));
    let _ = std::fs::remove_dir_all(dir);
}

/// `get_url` gives the URL of the point where the computation is. (In the original `getURL` never gives a result: it looks for a
/// `Maybe PrevClos` in the state and there is never one.)
#[test]
fn the_url_of_the_current_point() {
    use transient_core::*;
    let mut ctx = Ctx::new();
    init_node_at(create_node("127.0.0.1", 8123)).run(&mut ctx);
    assert_eq!(get_url().run(&mut ctx), Some("http://127.0.0.1:8123/0/S0/".to_string()));
}

/// `servePage`: the files of `./static` of the node are served, `/` is `index.html`, and nothing outside `static` is
#[test]
fn the_node_serves_the_files_of_its_static_directory() {
    let dir = tmp("static");
    std::fs::create_dir_all(dir.join("static")).unwrap();
    std::fs::write(dir.join("static/index.html"), "<h1>hello</h1>").unwrap();
    std::fs::write(dir.join("static/style.css"), "body{}").unwrap();
    std::fs::write(dir.join("secret.txt"), "secret").unwrap();
    let port = free_port();
    let (_node, rx) = spawn_demo("web_demo", &dir, port, "multiuser");
    wait_line(&rx, "READY");

    let page = get(port, "", None, 500);
    assert!(page.starts_with("HTTP/1.0 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\nContent-Length: 14\r\n"), "{page:?}");
    assert!(page.ends_with("\r\n\r\n<h1>hello</h1>"), "{page:?}");
    assert_eq!(get(port, "index.html", None, 500), page);
    let css = get(port, "file/style.css", None, 500);
    assert!(css.contains("Content-Type: text/css") && css.ends_with("body{}"), "{css:?}");
    // what is not there, and what is outside of `static`, is not served
    for path in ["file/nope.css", "file/../secret.txt", "file/..%2fsecret.txt", "file/%2e%2e/secret.txt"] {
        let r = get(port, path, None, 500);
        assert!(r.starts_with("HTTP/1.0 404 Not Found"), "{path}: {r:?}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// CollectTest.hs / runcol.sh: `collectc` gathers two calls to the endpoint `test`; the node ends (SIGTERM, which saves the
/// state because of the job) and is started again with `jobs/exec` between them. The collection goes on from the first
/// call: the second completes it, and the endpoint that follows (`results`) exists and works.
#[test]
fn collectc_survives_a_restart() {
    collectc_restart("collectc", &[false], false);
}

/// The same, but the node is killed (SIGKILL): nothing is saved on exit, only what the job wrote before going on
#[test]
fn collectc_survives_a_kill() {
    collectc_restart("collectc-kill", &[true], false);
}

/// Several stops in a row, of both kinds, with nothing arriving in between: every restart replays the collection from
/// the closures on disk and must not duplicate or lose the first result
#[test]
fn collectc_survives_several_stops() {
    collectc_restart("collectc-several", &[true, false, true, true, false], false);
}

/// Both calls come from the same session, so the second one reaches a closure that already exists: the closure keeps the
/// two paths (`Fragment::Collect`) and a restarted node has to replay both
#[test]
fn collectc_keeps_the_paths_of_one_session_across_stops() {
    collectc_restart("collectc-session", &[true, false], true);
}

/// Starts the node, makes the first call, stops it (`true` = killed, `false` = SIGTERM) and starts it again with `jobs/exec`
/// for each stop; the last start receives the second call
fn collectc_restart(name: &str, stops: &[bool], same_session: bool) {
    let dir = tmp(name);
    let port = free_port();
    let (node, rx) = spawn_demo("web_demo", &dir, port, "collect");
    wait_line(&rx, "READY");
    // the first call gets no response (nothing is sent until the collection ends), so the session is chosen by the caller:
    // the endpoint of session 0 is invoked from it (`S0`) and the session gets its own copy, where the second call arrives
    let session = same_session.then(|| "mysession".to_string());
    get(port, if same_session { "test/S0/a" } else { "test/a" }, session.as_deref(), 800);
    let mut current = (node, rx);
    for (i, &kill) in stops.iter().enumerate() {
        if kill {
            current.0 .0.kill().unwrap();
            current.0 .0.wait().unwrap();
        } else {
            current.0.signal("TERM");
            current.0.wait_exit(10).expect("the node ends with the signal");
        }
        // RESTART_FORGET=1 starts the new process with an empty directory: the test must then fail (negative control)
        let dir2 = if std::env::var("RESTART_FORGET").is_ok() { dir.join("empty") } else { dir.clone() };
        current = spawn_demo_args("web_demo", &dir2, port, "collect", "/jobs/exec");
        wait_line(&current.1, "READY");
        std::thread::sleep(Duration::from_millis(1000));
        assert!(!dir.join("collected.txt").exists(), "restart {i}: one call is not enough");
    }
    let last = get(port, "test/b", session.as_deref(), 1000);
    std::thread::sleep(Duration::from_millis(500));
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
    let mut collected: Vec<String> = serde_json::from_str(read("collected.txt").lines().next().unwrap_or("[]")).unwrap_or_default();
    collected.sort();
    assert_eq!(collected, ["a", "b"], "{:?}", read("collected.txt"));
    assert_eq!(read("collected.txt").lines().count(), 1, "the collection ends once");
    assert_eq!(read("reached-results.txt").lines().count(), 1);
    assert_eq!(read("results-called.txt").lines().count(), 0);
    // the cookie is only in the response when the session is new
    let cookie = session.clone().unwrap_or_else(|| cookie_of(&last));
    get(port, "results/u", Some(&cookie), 1000);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(read("results-called.txt").lines().count(), 1, "the endpoint `results` is invoked after the collection");
    let _ = std::fs::remove_dir_all(dir);
}

/// Without restart: the two calls complete the collection and the program goes on
#[test]
fn collectc_gathers_calls_of_different_sessions() {
    let dir = tmp("collectc-plain");
    let port = free_port();
    let (_n, rx) = spawn_demo("web_demo", &dir, port, "collect");
    wait_line(&rx, "READY");
    get(port, "test/a", None, 500);
    assert!(!dir.join("collected.txt").exists(), "one call is not enough");
    get(port, "test/b", None, 500);
    wait_line(&rx, "COLLECTED");
    let mut collected: Vec<String> = serde_json::from_str(std::fs::read_to_string(dir.join("collected.txt")).unwrap().lines().next().unwrap()).unwrap();
    collected.sort();
    assert_eq!(collected, ["a", "b"]);
    let _ = std::fs::remove_dir_all(dir);
}

/// The handlers that the branches of a `collectc` register are all in the state that goes on after it
#[test]
fn collectc_joins_the_handlers_of_its_branches() {
    let dir = tmp("collectc-back");
    let port = free_port();
    let (_n, rx) = spawn_demo("web_demo", &dir, port, "collectback");
    wait_line(&rx, "READY");
    get(port, "test/a", None, 500);
    get(port, "test/b", None, 500);
    let mut undone = vec![wait_line(&rx, "UNDO"), wait_line(&rx, "UNDO")];
    undone.sort();
    assert_eq!(undone, ["UNDO a", "UNDO b"]);
    let _ = std::fs::remove_dir_all(dir);
}

/// The collection completes, the node is killed before `results` is invoked and restarted: the endpoint `results` is still
/// there. A closure is written in the background (every 10 s by default, like the cache of the original), and a job only
/// protects the ones before it: the test writes every 50 ms, otherwise the kill comes before the write of `results`
#[test]
fn collectc_endpoint_after_it_survives_a_kill() {
    let dir = tmp("collectc-after");
    let port = free_port();
    let (mut n1, rx1) = start("web_demo", &dir, "collect", &path(port, ""), &[("TRANSIENT_SYNC_MS", "50")]);
    wait_line(&rx1, "READY");
    get(port, "test/a", None, 500);
    let last = get(port, "test/b", None, 800);
    wait_line(&rx1, "COLLECTED");
    std::thread::sleep(Duration::from_millis(300));
    n1.0.kill().unwrap();
    n1.0.wait().unwrap();
    let (_n2, rx2) = spawn_demo_args("web_demo", &dir, port, "collect", "/jobs/exec");
    wait_line(&rx2, "READY");
    std::thread::sleep(Duration::from_millis(1000));
    get(port, "results/u", Some(&cookie_of(&last)), 1000);
    std::thread::sleep(Duration::from_millis(500));
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
    assert_eq!(read("results-called.txt").lines().count(), 1, "the endpoint `results` works after the restart");
    let _ = std::fs::remove_dir_all(dir);
}

/// `examples/widgets.rs`, the widgets of `axiom/tests/widgets.hs`: each one is an endpoint that can be used again and again,
/// and an event in one does not touch the others
#[test]
fn the_widgets_answer_their_events_independently() {
    let dir = tmp("widgets");
    std::fs::create_dir_all(dir.join("static")).unwrap();
    let port = free_port();
    let (_n, rx) = spawn_demo("widgets", &dir, port, "widgets");
    wait_line(&rx, "Run it");
    std::thread::sleep(Duration::from_millis(800));
    let body = |path: &str| get(port, path, None, 400).split("\r\n").find(|l| l.starts_with('{')).unwrap_or_default().to_string();
    assert_eq!(body("person/Ana/30"), r#"{"age":30,"name":"Ana"}"#);
    assert_eq!(body("radio/green"), r#"{"radio":"green"}"#);
    assert_eq!(body("checks/%5B%22Red%22%2C%22Green%22%5D"), r#"{"checks":["Red","Green"]}"#);
    assert_eq!(body("select/blue"), r#"{"select":"blue"}"#);
    assert_eq!(body("hi/u"), r#"{"hi":"This link says Hi!"}"#);
    // the same widget again, after the others
    assert_eq!(body("person/Luis/41"), r#"{"age":41,"name":"Luis"}"#);
    assert_eq!(body("radio/blue"), r#"{"radio":"blue"}"#);
    // a cookie of another run names a session that this node does not have: it is ignored (before, nothing answered)
    let stale = get(port, "person/Eva/22", Some("zxbju"), 400);
    assert!(stale.contains(r#"{"age":22,"name":"Eva"}"#), "{stale:?}");
    // the browser asks for the icon: it is not a closure, so it is a 404 and the node goes on
    assert!(get(port, "favicon.ico", Some("zxbju"), 400).starts_with("HTTP/1.0 404 Not Found"));
    assert_eq!(body("hi/u"), r#"{"hi":"This link says Hi!"}"#);
    let _ = std::fs::remove_dir_all(dir);
}

/// The options of the command line, in any order and not transformed at the start: the node ends (SIGTERM, which saves because of
/// the job) and is started again with `--exec --jobs --start`, the options of the program in the order opposite to the one
/// they are executed in (`start`, then `jobs`, then `exec`). The collection goes on from the first call.
#[test]
fn the_options_of_the_command_line_work_in_any_order() {
    let dir = tmp("options");
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let (mut n1, rx1) = start_args("web_demo", &dir, &["collect", "--saveonexit", "--start", &addr]);
    wait_line(&rx1, "READY");
    get(port, "test/a", None, 800);
    n1.signal("TERM");
    n1.wait_exit(10).expect("the node ends with the signal");

    let (_n2, rx2) = start_args("web_demo", &dir, &["collect", "--exec", "--jobs", "--start", &addr]);
    wait_line(&rx2, "READY");
    std::thread::sleep(Duration::from_millis(1000));
    get(port, "test/b", None, 1000);
    std::thread::sleep(Duration::from_millis(500));
    let mut collected: Vec<String> = serde_json::from_str(std::fs::read_to_string(dir.join("collected.txt")).unwrap_or_default().lines().next().unwrap_or("[]")).unwrap_or_default();
    collected.sort();
    assert_eq!(collected, ["a", "b"]);
    let _ = std::fs::remove_dir_all(dir);
}
