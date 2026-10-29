//! The widgets of `axiom/tests/widgets.hs` as a program of the node: a person (name and age), radio buttons, check boxes,
//! a select and a link. Each widget is an endpoint (`minput`) and the widgets are alternatives (`cor`, the `<|>` of the
//! original): an event in one of them does not touch the others.
//!
//! ```text
//! cargo run --offline --example widgets -- --start localhost:8000
//! ```
//!
//! It serves `./static/index.html`; when the current directory has no `static`, the one of the example is used (and the
//! closures of the node are stored there, in `.tcachedata`).
//!
//! Open <http://localhost:8000/>. The page (`./static/index.html`) draws the widgets, and calls the endpoint of each one
//! when something happens in it:
//!
//! ```text
//! /person/Ana/30                  ->  {"name":"Ana","age":30}
//! /radio/green                    ->  {"radio":"green"}
//! /checks/["Red","Green"]         ->  {"checks":["Red","Green"]}      (the list is JSON, as in the log)
//! /select/blue                    ->  {"select":"blue"}
//! /hi/u                           ->  {"hi":"This link says Hi!"}
//! ```
//!
//! In the original the widgets are drawn by the code that runs in the browser (compiled to wasm), here by a page that
//! knows the endpoints. What the program does with the values is the same: it receives them and answers.

use serde_json::json;
use transient_core::*;

/// `Person <$> inputString <*> inputInt <** inputSubmit "send"`: the name and the age arrive together, with the button
fn person() -> Cloud<()> {
    cdo! {
        let (name, age) = minput::<(String, i64), _>("person", "Enter the name and the age (Int)");
        local_io({
            let name = name.clone();
            move || println!("Person {{name = {name:?}, age = {age}}}")
        });
        moutput(&json!({ "name": name, "age": age }))
    }
}

/// `getRadio`: one of red, green or blue
fn radio() -> Cloud<()> {
    cdo! {
        let r = minput::<String, _>("radio", "red, green or blue");
        moutput(&json!({ "radio": r }))
    }
}

/// `getCheckBoxes`: the names of the boxes that are checked
fn check_boxes() -> Cloud<()> {
    cdo! {
        let rs = minput::<Vec<String>, _>("checks", "Red, Green and blue");
        moutput(&json!({ "checks": rs }))
    }
}

/// `getSelect`
fn select() -> Cloud<()> {
    cdo! {
        let r = minput::<String, _>("select", "red, green or blue");
        moutput(&json!({ "select": r }))
    }
}

/// `wlink "Hi!" (toElem "This link say Hi!")`
fn link() -> Cloud<()> {
    cdo! {
        minput::<(), _>("hi", "Hi!");
        moutput(&json!({ "hi": "This link says Hi!" }))
    }
}

fn widgets() -> Cloud<()> {
    person() | radio() | check_boxes() | select() | link()
}

fn main() {
    // the page is `./static/index.html`: when the program is not run from a directory that has it, use the one of the example
    if !std::path::Path::new("static").exists() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/widgets")).unwrap();
    }
    println!("Run it with: widgets --start localhost:8000   and open http://localhost:8000/");
    // the tests pass the name of the program first (`<example> <program> -p <path>`)
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = if args.first().is_some_and(|a| !a.starts_with('-')) { &args[1..] } else { &args[..] };
    let _: Result<(), String> = keep_args(rest, init_node(widgets()));
}
