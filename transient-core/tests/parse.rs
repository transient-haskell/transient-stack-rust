//! The parser (`parse.rs`). Each test reproduces a case of Exp13/Exp14, run against the original in Haskell.
//! The `double` cases that the original got wrong are marked: here the correct result is checked.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use transient_core::*;

/// Runs `parser` over `input` and returns its result and what is left unparsed
fn p<A: Clone + Send + Sync + 'static>(parser: Trans<A>, input: &str) -> Option<(A, String)> {
    let t = with_parse_string(input, tdo! { let x = parser; let rest = not_parsed(); pure((x, rest)) });
    t.run(&mut Ctx::new())
}

fn some<A>(a: A, rest: &str) -> Option<(A, String)> {
    Some((a, rest.to_string()))
}

/// The result of `parser` over a stream with those chunks and what is left read but unparsed
fn ps<A: Clone + Send + Sync + 'static>(parser: Trans<A>, chunks: &[&str]) -> Option<(A, String)> {
    let queue: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(chunks.iter().map(|c| c.to_string()).collect()));
    let more = move || {
        let mut q = queue.lock().unwrap();
        match q.pop_front() {
            None => StreamChunk::Done,
            Some(c) if q.is_empty() => StreamChunk::Last(c),
            Some(c) => StreamChunk::More(c),
        }
    };
    let t = with_parse_stream(more, tdo! { let x = parser; let buf = get_parse_buffer(); pure((x, buf)) });
    t.run(&mut Ctx::new())
}

// ---- numbers

#[test]
fn int_accepts_a_sign_but_not_spaces() {
    assert_eq!(p(int(), "12abc"), some(12, "abc"));
    assert_eq!(p(int(), "-5"), some(-5, ""));
    assert_eq!(p(int(), "+7"), some(7, ""));
    assert_eq!(p(int(), "007"), some(7, ""));
    assert_eq!(p(int(), " 3"), None);
    assert_eq!(p(int(), ""), None);
    assert_eq!(p(int(), "abc"), None);
}

#[test]
fn integer_and_hex() {
    assert_eq!(p(integer(), "123456789012345678901234567890x"), some(123456789012345678901234567890i128, "x"));
    assert_eq!(p(integer(), "-9"), some(-9, ""));
    assert_eq!(p(hex(), "ff"), some(255, ""));
    assert_eq!(p(hex(), "FFz"), some(255, "z"));
    assert_eq!(p(hex(), "g"), None);
    assert_eq!(p(hex(), "0x1"), some(0, "x1"));
    assert_eq!(p(hex(), "1A2b rest"), some(6699, " rest"));
}

#[test]
fn double_reads_what_the_original_read_right() {
    assert_eq!(p(double(), "3.5"), some(3.5, ""));
    assert_eq!(p(double(), "-3.5"), some(-3.5, ""));
    assert_eq!(p(double(), "2E1"), some(20.0, ""));
    assert_eq!(p(double(), "7"), some(7.0, ""));
    assert_eq!(p(double(), "0.25x"), some(0.25, "x"));
    assert_eq!(p(double(), ".5"), None);
    assert_eq!(p(double(), "abc"), None);
}

// The original lost the sign with a zero integer part ("-0.5" gave 0.5), threw "Negative exponent" with "1e2", "1.5e2"
// and "1e-2", and crashed with "3." (fromJust Nothing). Here it is read correctly.
#[test]
fn double_fixes_the_errors_of_the_original() {
    assert_eq!(p(double(), "-0.5"), some(-0.5, ""));
    assert_eq!(p(double(), "1e2"), some(100.0, ""));
    assert_eq!(p(double(), "1.5e2"), some(150.0, ""));
    assert_eq!(p(double(), "1e-2"), some(0.01, ""));
    assert_eq!(p(double(), "3."), some(3.0, "."));
}

// ---- atomicity

#[test]
fn string_and_char_are_atomic() {
    assert_eq!(p(string("ab"), "abc"), some("ab".to_string(), "c"));
    assert_eq!(p(string("ab"), "ac"), None);
    assert_eq!(p(string("ab"), "a"), None);
    assert_eq!(p(string("ab") | string("ac"), "acd"), some("ac".to_string(), "d"));
    assert_eq!(p(t_char('a'), "abc"), some('a', "bc"));
    assert_eq!(p(t_char('a'), "xbc"), None);
    assert_eq!(p(any_char(), "xyz"), some('x', "yz"));
    assert_eq!(p(any_char(), ""), None);
}

#[test]
fn a_composite_parser_that_fails_midway_does_not_restore_the_position_unless_try() {
    let ab = || tdo! { t_char('a'); t_char('b') };
    assert_eq!(p(ab() | t_char('a'), "ac"), None);
    assert_eq!(p(try_(ab()) | t_char('a'), "ac"), some('a', "c"));
}

// ---- taking and skipping

#[test]
fn take_and_drop() {
    assert_eq!(p(t_take_while(|c| c.is_ascii_digit()), "123abc"), some("123".to_string(), "abc"));
    assert_eq!(p(t_take_while(|c| c.is_ascii_digit()), "abc"), some(String::new(), "abc"));
    assert_eq!(p(t_take_while_drop(|c| c.is_ascii_digit()), "123;abc"), some("123".to_string(), "abc"));
    assert_eq!(p(t_take(2), "abcd"), some("ab".to_string(), "cd"));
    assert_eq!(p(t_take(9), "ab"), some("ab".to_string(), ""));
    assert_eq!(p(tdo! { t_drop(2); not_parsed() }, "abcd"), some("cd".to_string(), ""));
}

#[test]
fn take_and_drop_until() {
    let colons = |s: &str| s.starts_with("::");
    assert_eq!(p(t_take_until(colons), "ab::cd"), some("ab".to_string(), "::cd"));
    assert_eq!(p(t_take_until(colons), "abcd"), some("abcd".to_string(), ""));
    assert_eq!(p(tdo! { t_drop_until(colons); not_parsed() }, "ab::cd"), some("::cd".to_string(), ""));
    assert_eq!(p(t_take_until_token("::"), "ab::cd"), some("ab".to_string(), "cd"));
    assert_eq!(p(t_take_until_token("::"), "abcd"), None);
    assert_eq!(p(tdo! { t_drop_until_token("::"); not_parsed() }, "ab::cd"), some("cd".to_string(), ""));
}

#[test]
fn spaces_lines_and_words() {
    assert_eq!(p(tdo! { drop_spaces(); not_parsed() }, "  \t\n x"), some("x".to_string(), ""));
    assert_eq!(p(tdo! { drop_till_end_of_line(); not_parsed() }, "ab\ncd"), some("\ncd".to_string(), ""));
    assert_eq!(p(parse_string(), "  hola mundo"), some("hola".to_string(), " mundo"));
    assert_eq!(p(parse_string(), "   "), some(String::new(), ""));
    assert_eq!(p(tdo! { t_put_str("XY"); not_parsed() }, "ab"), some("XYab".to_string(), ""));
    assert_eq!(p(is_done(), ""), some(true, ""));
    assert_eq!(p(is_done(), "a"), some(false, "a"));
}

// ---- combinators

#[test]
fn many_till_and_sep_by() {
    assert_eq!(p(many_till(any_char(), t_char(';')), "abc;def"), some(vec!['a', 'b', 'c'], "def"));
    assert_eq!(p(many_till(any_char(), t_char(';')), ";x"), some(vec![], "x"));
    assert_eq!(p(many_till(any_char(), t_char(';')), "abc"), None);
    assert_eq!(p(sep_by(int(), t_char(',')), "1,2,3x"), some(vec![1, 2, 3], "x"));
    assert_eq!(p(sep_by(int(), t_char(',')), "x"), some(vec![], "x"));
    assert_eq!(p(sep_by(int(), t_char(',')), "1,2,"), some(vec![1, 2], ","));
    assert_eq!(p(comma_sep(int()), "4,5"), some(vec![4, 5], ""));
    assert_eq!(p(semi_sep(int()), "4;5"), some(vec![4, 5], ""));
}

#[test]
fn brackets_and_between() {
    assert_eq!(p(parens(int()), "(12)x"), some(12, "x"));
    assert_eq!(p(parens(int()), "(12x"), None);
    assert_eq!(p(braces(int()), "{1}"), some(1, ""));
    assert_eq!(p(brackets(int()), "[1]"), some(1, ""));
    assert_eq!(p(angles(int()), "<1>"), some(1, ""));
    assert_eq!(p(between(t_char('['), t_char(']'), int()), "[7]"), some(7, ""));
}

#[test]
fn chain_many_builds_from_the_end() {
    let aaa = chain_many(|c: char, mut s: String| { s.insert(0, c); s }, t_char('a'), String::new);
    assert_eq!(p(aaa, "aaab"), some("aaa".to_string(), "b"));
}

// ---- the applicative in serial mode (Exp14)

#[test]
fn the_applicative_in_serial_mode_sequences_the_parsers() {
    let pair = || lift2(|a, b| (a, b), int(), tdo! { t_char(','); int() });
    assert_eq!(p(pair(), "3,4rest"), some((3, 4), "rest"));
    assert_eq!(p(pair(), "3;4"), None);
    // the first fails: the second does not even start
    let ab = lift2(|a, b| (a, b), t_char('x'), t_char('a')) | tdo! { let r = not_parsed(); pure(('-', r.chars().next().unwrap())) };
    assert_eq!(p(ab, "ab"), some(('-', 'a'), ""));
    // the second fails after consuming the first: the position is not restored
    let ax = lift2(|a, b| (a, b), t_char('a'), t_char('x')) | tdo! { let r = not_parsed(); pure(('-', r.chars().next().unwrap())) };
    assert_eq!(p(ax, "ab"), some(('-', 'b'), ""));
}

#[test]
fn setting_the_input_leaves_the_execution_in_serial_mode() {
    // `with_parse_string` leaves the mode serial: a `lift2` of parsers does not run the second if the first fails
    let t = with_parse_string("x", lift2(|a: char, b: char| (a, b), t_char('y'), t_char('x')));
    assert_eq!(t.run(&mut Ctx::new()), None);
}

// ---- streams by chunks

#[test]
fn a_chunk_can_cut_a_number_or_a_string() {
    assert_eq!(ps(int(), &["12", "34", "ab"]), some(1234, "ab"));
    assert_eq!(ps(string("abc"), &["a", "b", "cd"]), some("abc".to_string(), "d"));
    assert_eq!(ps(int(), &["12"]), some(12, ""));
    assert_eq!(ps(double(), &["3", ".", "25"]), some(3.25, ""));
}

#[test]
fn take_while_across_chunks() {
    let r = ps(t_take_while(|c| c.is_ascii_digit()), &["1", "23", "4x", "5"]).unwrap();
    assert_eq!(r.0, "1234");
    assert!(r.1.starts_with('x'));
}

#[test]
fn one_byte_per_chunk() {
    let t = tdo! { let a = int(); t_char(','); let b = int(); pure((a, b)) };
    assert_eq!(ps(t, &["1", "2", ",", "3", "4"]), some((12, 34), ""));
}

#[test]
fn a_token_cut_by_a_chunk() {
    assert_eq!(ps(t_take_until_token("::"), &["ab:", ":cd"]), some("ab".to_string(), "cd"));
    assert_eq!(ps(is_done(), &[]), some(true, ""));
}

#[test]
fn with_parse_string_restores_the_previous_input() {
    let t = tdo! {
        set_parse_string("fuera");
        let x = with_parse_string("dentro", t_take(3));
        let y = t_take(2);
        pure((x, y))
    };
    assert_eq!(t.run(&mut Ctx::new()), Some(("den".to_string(), "fu".to_string())));
}
