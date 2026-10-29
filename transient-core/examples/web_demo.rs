//! A node that serves REST endpoints with `minput` (the `MultiUser.hs` and `AppMinput.hs` programs of the Haskell battery).
//!
//! ```text
//! web_demo <program> -p start/127.0.0.1/9300
//! ```
//!
//! The command line is the one of the Haskell programs (`start/<host>/<port>`, `saveonexit`...). The programs:
//!
//! ```text
//! multiuser       users that create an endpoint another user invokes (publish / published)
//! appminput       the applicative of two inputs and a final one
//! endpt           multiuser with the console command `endpt` (the interface of each endpoint)
//! post            inputs of different types: a POST body, a tuple and a list (WebPost.hs)
//! collectback     the handlers registered by the branches of a `collectc` are all there afterwards
//! collect         `collectc` of two calls to the endpoint `test`, restart-proof (CollectTest.hs)
//! ```

use std::time::Duration;
use transient_core::*;

/// Alice locks a number; the endpoint `enterguess` is published and invoked by other users, each in their own session
fn guessgame(name: String) -> Cloud<()> {
    cdo! {
        [name]
        cor(minput::<(), _>("guessgame", "play guessgame"), published("lock"));
        let lock = minput::<i64, _>("enterlock", format!("{name}: lock a number to guess it"));
        let guess = publish(true, "lock", minput::<i64, _>("enterguess", format!("guess the number entered by: {name}")));
        local_io({
            let name = name.clone();
            move || println!("GUESS {name} ({lock},{guess})")
        });
        minput::<(), _>("entered", vec![("entered number".to_string(), guess.to_string()), ("success".to_string(), if lock == guess { "True" } else { "False" }.to_string())])
    }
}

fn multiuser() -> Cloud<()> {
    cdo! {
        let name = minput::<String, _>("enterw", "enter your name");
        guessgame(name)
    }
}

/// Two inputs in an applicative, in any order, and an endpoint with the result
fn appminput() -> Cloud<()> {
    cdo! {
        let r = clift2(|a: String, b: String| a + &b, minput::<String, _>("t1", "first"), minput::<String, _>("t2", "second"));
        local_io({ let r = r.clone(); move || println!("MINPUT RESOLVED {r}") });
        minput::<(), _>("final", ("RESULT".to_string(), r))
    }
}

/// A POST body, a tuple and a list, as `WebPost.hs`
fn post_demo() -> Cloud<()> {
    cdo! {
        minput::<(), _>("start", "hello");
        let PostData(i) = minput::<PostData<i64>, _>("endp", "enter an int");
        local_io(move || println!("GOT {i}"));
        let ab = minput::<(i64, String), _>("pair", "pair");
        local_io({ let ab = ab.clone(); move || println!("PAIR ({},{:?})", ab.0, ab.1) });
        let xs = minput::<Vec<i64>, _>("list", "a list");
        local_io({ let xs = xs.clone(); move || println!("LIST [{}]", xs.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")) });
        minput::<(), _>("end", "done")
    }
}

/// `collectc` with `minput` and jobs: two calls to the endpoint `test` are collected, even if the node is restarted between
/// them (start it again with `jobs/exec`). The results go to files: the console of several threads gets mixed up.
fn collect_demo() -> Cloud<()> {
    let mark = |file: &'static str, text: String| local_io(move || {
        use std::io::Write;
        writeln!(std::fs::OpenOptions::new().create(true).append(true).open(file).unwrap(), "{text}").unwrap();
    });
    cdo! {
        run_jobs();
        let r = collectc(2, 0, minput::<String, _>("test", "enter string"));
        mark("collected.txt", format!("{r:?}"));
        local_io({ let r = r.clone(); move || println!("COLLECTED {r:?}") });
        mark("reached-results.txt", "1".into());
        minput::<(), _>("results", "see results");
        // this runs only when the endpoint `results` is invoked
        mark("results-called.txt", "1".into())
    }
}

/// Each branch registers an `on_undo` handler: after the collection `undo` runs the ones of both
fn collect_back_demo() -> Cloud<()> {
    cdo! {
        let _r = collectc(2, 0, cdo! {
            let x = minput::<String, _>("test", "enter string");
            on_all(on_undo(pure(x.clone()), { let x = x.clone(); lift_io(move || { println!("UNDO {x}"); x.clone() }) }))
        });
        on_all(undo::<()>())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(300));
        println!("READY");
    });
    let rest = &args[2..];
    let _: Result<(), String> = match args[1].as_str() {
        "multiuser" => keep_args(rest, init_node(multiuser())),
        "appminput" => keep_args(rest, init_node(appminput())),
        "endpt" => keep_args(rest, init_node(cor(option_endpoints(), multiuser()))),
        "post" => keep_args(rest, init_node(post_demo())),
        "collectback" => keep_args(rest, init_node(collect_back_demo())),
        "collect" => keep_args(rest, init_node(collect_demo())),
        other => panic!("unknown program {other}"),
    };
}
