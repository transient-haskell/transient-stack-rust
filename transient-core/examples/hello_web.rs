//! A web program of two chained `minput`: it asks for a name and then for the surnames, and shows the result, in a page.
//!
//! ```text
//! cd transient-rs/transient-core/examples/hello_web
//! cargo run --example hello_web -- --start localhost:8080
//! ```
//!
//! The command line is the one of the Haskell programs (`-p start/localhost/8080` and `--start localhost/8080` work too).
//! Open <http://localhost:8080/> in the browser: the node serves `./static/index.html` of the current directory, which is
//! why it is run from `examples/hello_web`. The page does not know the program: every response says what to call next
//! (`req.requrl`, with a `$string` where the value goes), and the page shows the message, asks for the value and follows
//! the link. The endpoints can be used without the page too:
//!
//! ```text
//! /start/u                      ->  { "msg":"What is your name?",   "req":{... "requrl":"http://localhost:8080/name/$string"}}
//! /name/Ana                     ->  { "msg":"Hello Ana, what are your surnames?", "req":{... ".../surnames/$stringx"}}
//! /surnames/Garcia%20Lopez      ->  {"fullname":"Ana Garcia Lopez","name":"Ana","surnames":"Garcia Lopez"}
//! ```
//!
//! (the second variable is `$stringx` because the names of the variables of a session are unique). The first response of
//! the node sets the cookie `state`: it is the session, so several people can use the program at the same time, each one
//! with their own name. A value cannot contain `/`.

use transient_core::*;

fn app() -> Cloud<()> {
    cdo! {
        // the entry point: the first response says what to do
        minput::<(), _>("start", "Welcome");
        let name = minput::<String, _>("name", "What is your name?");
        let surnames = minput::<String, _>("surnames", format!("Hello {name}, what are your surnames?"));
        local_io({
            let (name, surnames) = (name.clone(), surnames.clone());
            move || println!("{name} {surnames}")
        });
        moutput(&serde_json::json!({ "name": name, "surnames": surnames, "fullname": format!("{name} {surnames}") }))
    }
}

fn main() {
    // the page is `./static/index.html`: when the program is not run from a directory that has it, use the one of the example
    if !std::path::Path::new("static").exists() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/hello_web")).unwrap();
    }
    println!("Run it from examples/hello_web with: hello_web --start localhost:8080   and open http://localhost:8080/");
    let _: Result<(), String> = keep(init_node(app()));
}
