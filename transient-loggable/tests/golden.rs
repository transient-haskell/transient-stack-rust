//! Golden tests: what the original in Haskell writes (`transient/tests/original-vs-rust/LogFormat*.hs`) for the
//! atoms, and the reads it does. Strings follow the new rule (JSON), not the original's (see `lib.rs`).

use serde::{Deserialize, Serialize};
use transient_loggable::*;

fn ser<T: Loggable>(x: T) -> String {
    to_log(&x)
}

#[test]
fn integers() {
    assert_eq!(ser(0i64), "0");
    assert_eq!(ser(-5i32), "-5");
    assert_eq!(ser(i64::MAX), "9223372036854775807");
    assert_eq!(ser(123456789012345678901234567890u128), "123456789012345678901234567890");
}

#[test]
fn doubles_like_haskell_show() {
    let cases: &[(f64, &str)] = &[
        (0.1, "0.1"),
        (1.0, "1.0"),
        (100.0, "100.0"),
        (0.01, "1.0e-2"),
        (1e7, "1.0e7"),
        (9999999.0, "9999999.0"),
        (123456.789, "123456.789"),
        (1.5e10, "1.5e10"),
        (1e-3, "1.0e-3"),
        (-2.5, "-2.5"),
        (0.0, "0.0"),
        (-0.0, "-0.0"),
        (f64::NAN, "NaN"),
        (f64::INFINITY, "Infinity"),
        (f64::NEG_INFINITY, "-Infinity"),
        (5e-324, "5.0e-324"),
        (3.14159, "3.14159"),
        (0.1 + 0.2, "0.30000000000000004"),
    ];
    for (x, want) in cases {
        assert_eq!(ser(*x), *want, "{x:?}");
    }
}

#[test]
fn floats_like_haskell_show() {
    assert_eq!(ser(3.14f32), "3.14");
    assert_eq!(ser(0.01f32), "1.0e-2");
    assert_eq!(ser(16777216f32), "1.6777216e7");
}

#[test]
fn bool_and_unit() {
    assert_eq!(ser(true), "t");
    assert_eq!(ser(false), "f");
    assert_eq!(ser(()), "u");
}

#[test]
fn chars_like_haskell_show() {
    let cases: &[(char, &str)] = &[
        ('a', "'a'"),
        ('\n', "'\\n'"),
        ('\'', "'\\''"),
        ('\\', "'\\\\'"),
        ('é', "'\\233'"),
        ('€', "'\\8364'"),
        ('"', "'\"'"),
        // the original writes `'/'`, which it can not read back itself; `'\47'` it can
        ('/', "'\\47'"),
        ('\0', "'\\NUL'"),
        ('\x7f', "'\\DEL'"),
    ];
    for (c, want) in cases {
        assert_eq!(ser(*c), *want, "{c:?}");
        assert_eq!(from_log::<char>(want).map(|r| r.0), Some(*c), "leer {want}");
    }
}

#[test]
fn reads_like_haskell_does() {
    assert_eq!(from_log::<i64>("42/resto"), Some((42, "/resto")));
    assert_eq!(from_log::<i64>("-5/x"), Some((-5, "/x")));
    assert_eq!(from_log::<f64>("3.5/x"), Some((3.5, "/x")));
    assert_eq!(from_log::<f64>("3/x"), Some((3.0, "/x")));
    assert_eq!(from_log::<f64>("1.0e-2/x"), Some((0.01, "/x")));
    assert_eq!(from_log::<bool>("t/x"), Some((true, "/x")));
    assert_eq!(from_log::<()>("u/x"), Some(((), "/x")));
    assert_eq!(from_log::<char>("'x'/y"), Some(('x', "/y")));
    assert_eq!(from_log::<char>("'\\233'/y"), Some(('é', "/y")));
    assert_eq!(from_log::<char>("'\\n'/y"), Some(('\n', "/y")));
    assert_eq!(from_log::<char>("'\\47'/y"), Some(('/', "/y")));
    assert_eq!(from_log::<i64>("/x"), None);
    assert_eq!(from_log::<i64>("abc/x"), None);
}

#[test]
fn doubles_round_trip() {
    let mut x = 0x9e3779b97f4a7c15u64;
    for _ in 0..5000 {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let f = f64::from_bits(x);
        let text = ser(f);
        let (back, rest) = from_log::<f64>(&text).unwrap();
        assert!(rest.is_empty());
        assert!(back == f || (back.is_nan() && f.is_nan()), "{f:?} -> {text} -> {back:?}");
    }
}

// ---- strings and structures: JSON with doubled slashes

#[test]
fn strings_are_json_with_doubled_slashes() {
    assert_eq!(ser("hello".to_string()), "\"hello\"");
    assert_eq!(ser("a/b".to_string()), "\"a//b\"");
    assert_eq!(ser("a//b".to_string()), "\"a////b\"");
    assert_eq!(ser("a\"b".to_string()), "\"a\\\"b\"");
    assert_eq!(ser("a\nb".to_string()), "\"a\\nb\"");
    assert_eq!(ser("ñandú €".to_string()), "\"ñandú €\"");
    // the empty string is not mistaken for the text "[]"
    assert_eq!(ser(String::new()), "\"\"");
    assert_eq!(ser("[]".to_string()), "\"[]\"");
}

#[test]
fn strings_round_trip() {
    for s in ["", "[]", "hello", "a/b", "a//b", "a/", "/a", "///", "a\"b", "a\\b", "a\nb\tc", "ñandú €", "𝄞", "a b", "x:y;z"] {
        let text = ser(s.to_string());
        let input = format!("{text}/resto");
        let (back, rest) = from_log::<String>(&input).unwrap();
        assert_eq!(back, s, "{text}");
        assert_eq!(rest, "/resto");
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct P {
    name: String,
    age: u32,
}
loggable_json!(P);

#[test]
fn a_struct_is_json() {
    let p = P { name: "ana/x".into(), age: 30 };
    assert_eq!(ser(p), "{\"name\":\"ana//x\",\"age\":30}");
    let (q, rest) = from_log::<P>("{\"name\":\"ana//x\",\"age\":30}/siguiente").unwrap();
    assert_eq!(q, P { name: "ana/x".into(), age: 30 });
    assert_eq!(rest, "/siguiente");
}

#[test]
fn lists_tuples_and_options_are_json() {
    assert_eq!(ser(vec![1, 2, 3]), "[1,2,3]");
    assert_eq!(ser(vec!["a".to_string(), "b/c".to_string()]), "[\"a\",\"b//c\"]");
    assert_eq!(ser((3.5f64, true)), "[3.5,true]");
    assert_eq!(ser(Some(7)), "7");
    assert_eq!(ser(None::<i32>), "null");
    assert_eq!(from_log::<Vec<String>>("[\"a\",\"b//c\"]/x"), Some((vec!["a".to_string(), "b/c".to_string()], "/x")));
    assert_eq!(from_log::<Option<i32>>("null/x"), Some((None, "/x")));
}

#[test]
fn a_whole_log_round_trips() {
    let mut log = String::new();
    for part in [to_log(&42i64), to_log(&"hola/mundo \"x\"".to_string()), to_log(&(3.5f64, true)), to_log(&())] {
        log.push_str(&part);
        log.push('/');
    }
    let mut rest = log.as_str();
    assert_eq!(<i64 as Loggable>::deserialize(&mut rest), Some(42));
    rest = rest.strip_prefix('/').unwrap();
    assert_eq!(<String as Loggable>::deserialize(&mut rest), Some("hola/mundo \"x\"".to_string()));
    rest = rest.strip_prefix('/').unwrap();
    assert_eq!(<(f64, bool) as Loggable>::deserialize(&mut rest), Some((3.5, true)));
    rest = rest.strip_prefix('/').unwrap();
    assert_eq!(<() as Loggable>::deserialize(&mut rest), Some(()));
    assert_eq!(rest, "/");
}

/// A string that does not start with quotes is read raw (what a URL or the console gives), up to the first single `/`
#[test]
fn strings_are_also_read_raw() {
    assert_eq!(from_log::<String>("alice/rest"), Some(("alice".to_string(), "/rest")));
    assert_eq!(from_log::<String>("a//b/rest"), Some(("a/b".to_string(), "/rest")));
    assert_eq!(from_log::<String>("alice"), Some(("alice".to_string(), "")));
    // quoted strings are JSON
    assert_eq!(from_log::<String>("\"a b\"/x"), Some(("a b".to_string(), "/x")));
    // what is written is always JSON
    assert_eq!(to_log(&"alice".to_string()), "\"alice\"");
}
