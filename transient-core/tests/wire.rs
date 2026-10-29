//! The format of the messages between nodes, with messages captured between two Haskell nodes (SeqFail).

use transient_core::*;

const CAPTURED: &[&str] = &[
    r#"SMore/0/0/1339560924/0/["e//w//f//w//w//\"go\"//[(\"127.0.0.1\",9000,[]),(\"127.0.0.1\",9101,[]),(\"127.0.0.1\",9002,[])]//e//","e//"]"#,
    r#"SMore/1339560924/0/975343475/0/["u//1//","e//"]"#,
    r#"SMore/975343475/0/957294121/0/["u//e//","u//2//","u//e//","e//"]"#,
];

#[test]
fn haskell_messages_round_trip() {
    for m in CAPTURED {
        let w = decode(m).unwrap_or_else(|| panic!("no decode: {m}"));
        assert_eq!(&encode(&w), m);
    }
    let w = decode(CAPTURED[1]).unwrap();
    assert_eq!((w.clos_to.as_str(), w.sess_to.as_str(), w.clos_from.as_str(), w.sess_from.as_str()), ("1339560924", "0", "975343475", "0"));
    assert_eq!(w.logs, vec![Fragment::text("u/1/"), Fragment::text("e/")]);
}

#[test]
fn fragments_carry_any_text() {
    // any text: quotes, slashes, commas, brackets and UTF-8
    let logs = vec![Fragment::text("a\"b\\c/,]}"), Fragment::text("é日本"), Fragment::text("")];
    let w = Wire { last: true, clos_to: "1".into(), sess_to: "0".into(), clos_from: "2".into(), sess_from: "0".into(), logs };
    assert_eq!(decode(&encode(&w)), Some(w));
}

#[test]
fn a_collect_fragment_travels_as_an_array_of_paths() {
    let w = Wire { last: false, clos_to: "1".into(), sess_to: "0".into(), clos_from: "2".into(), sess_from: "0".into(), logs: vec![Fragment::text("u/1/"), Fragment::Collect(vec!["a/".into(), "b//c/".into()])] };
    let text = encode(&w);
    assert!(text.ends_with(r#"["u//1//",["a//","b////c//"]]"#), "{text}");
    assert_eq!(decode(&text), Some(w));
}
