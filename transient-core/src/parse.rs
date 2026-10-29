//! Incremental parser (`Transient.Parse`): the text to parse lives in the state, each primitive is atomic (if it
//! fails it does not advance) and they compose with `|`, `bind` and `lift2` (in serial mode).
//!
//! * The text is a shared buffer that grows on demand (`set_parse_stream` reads chunks when they are needed, as
//!   the original does with its lazy `ByteString`) and a position of its own in each branch. So `try_` and `sandbox`
//!   restore the position, and a chunk can cut a number or a string without the parser noticing.
//! * `set_parse_string` and `set_parse_stream` leave the execution in serial mode.
//! * It works with UTF-8 `char`s; the original works with bytes. It only differs with non-ASCII text (`t_take(n)` takes
//!   `n` characters).
//! * `|-` is not here (chaining two parsers with a producer in another thread).
//!
//! Bugs of the original that are **not** copied here: `double` lost the sign with a zero integer part (`-0.5` gave `0.5`),
//! threw an exception with exponents (`1e2`, `1.5e2`, `1e-2`) and with `3.`. Here it reads `-0.5`, `1e2`, `1.5e2` and `1e-2`,
//! and `3.` gives `3` and leaves the dot.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// A chunk of a stream, or its end
#[derive(Clone, Debug, PartialEq)]
pub enum StreamChunk {
    More(String),
    Last(String),
    Done,
}

type More = Box<dyn Fn() -> StreamChunk + Send + Sync>;

/// The text being parsed: shared by all the branches that inherit it
struct Source {
    buf: Mutex<String>,
    more: Option<More>,
    done: AtomicBool,
}

impl Source {
    /// Brings the next chunk. `false` if there is no more.
    fn pull(&self) -> bool {
        let Some(more) = &self.more else { return false };
        if self.done.load(Ordering::SeqCst) {
            return false;
        }
        match more() {
            StreamChunk::More(s) => {
                self.buf.lock().unwrap().push_str(&s);
                true
            }
            StreamChunk::Last(s) => {
                self.buf.lock().unwrap().push_str(&s);
                self.done.store(true, Ordering::SeqCst);
                true
            }
            StreamChunk::Done => {
                self.done.store(true, Ordering::SeqCst);
                false
            }
        }
    }

    fn at_eof(&self) -> bool {
        self.more.is_none() || self.done.load(Ordering::SeqCst)
    }
}

/// The parsing context of the state: the source and where this branch is in it
#[derive(Clone)]
pub(crate) struct ParseContext {
    src: Arc<Source>,
    pos: usize,
}

impl Default for ParseContext {
    fn default() -> Self {
        ParseContext::of_string(String::new())
    }
}

impl ParseContext {
    fn of_string(s: String) -> Self {
        ParseContext { src: Arc::new(Source { buf: Mutex::new(s), more: None, done: AtomicBool::new(false) }), pos: 0 }
    }
    fn of_stream(more: More) -> Self {
        ParseContext {
            src: Arc::new(Source { buf: Mutex::new(String::new()), more: Some(more), done: AtomicBool::new(false) }),
            pos: 0,
        }
    }
}

/// What a parsing primitive returns over the available text
pub enum Parsed<A> {
    /// the value and the bytes it consumes
    Ok(A, usize),
    Fail,
    /// it can not be decided with the text there is: more of the stream is needed
    NeedMore,
}

/// The basis of all the primitives (`withGetParseString`). `parser` receives the text that is left and whether it is the end of the
/// stream; if it can not decide and there is more, it returns `NeedMore` and it is called again with more text. If it fails, the
/// context does not change.
pub fn with_get_parse_string<A: Send + 'static>(parser: impl Fn(&str, bool) -> Parsed<A> + Send + Sync + 'static) -> Trans<A> {
    Trans::new(move |ctx| {
        let pc = ctx.parse.clone();
        loop {
            let eof = pc.src.at_eof();
            let r = {
                let buf = pc.src.buf.lock().unwrap();
                parser(&buf[pc.pos..], eof)
            };
            match r {
                Parsed::Ok(a, n) => {
                    ctx.parse.pos = pc.pos + n;
                    return Some(a);
                }
                Parsed::Fail => return None,
                Parsed::NeedMore => {
                    if !pc.src.pull() && pc.src.at_eof() {
                        // there is no more: it is decided with what there is
                        let r = {
                            let buf = pc.src.buf.lock().unwrap();
                            parser(&buf[pc.pos..], true)
                        };
                        return match r {
                            Parsed::Ok(a, n) => {
                                ctx.parse.pos = pc.pos + n;
                                Some(a)
                            }
                            _ => None,
                        };
                    }
                }
            }
        }
    })
}

/// Parses `x` (`setParseString`). It leaves the execution in serial mode.
pub fn set_parse_string(x: &str) -> Trans<()> {
    let x = x.to_string();
    Trans::new(move |ctx| {
        ctx.parse = ParseContext::of_string(x.clone());
        ctx.mode = Mode::Serial;
        Some(())
    })
}

/// Parses a stream of chunks: `more` is called, blocking, every time more text is needed (`setParseStream`).
/// It leaves the execution in serial mode.
pub fn set_parse_stream(more: impl Fn() -> StreamChunk + Send + Sync + 'static) -> Trans<()> {
    let more: Arc<dyn Fn() -> StreamChunk + Send + Sync> = Arc::new(more);
    Trans::new(move |ctx| {
        let m = more.clone();
        ctx.parse = ParseContext::of_stream(Box::new(move || m()));
        ctx.mode = Mode::Serial;
        Some(())
    })
}

/// Runs `parse` over `x` and restores the previous context at the end (`withParseString`)
pub fn with_parse_string<A: Send + 'static>(x: &str, parse: Trans<A>) -> Trans<A> {
    let x = x.to_string();
    Trans::new(move |ctx| {
        let saved = ctx.parse.clone();
        ctx.parse = ParseContext::of_string(x.clone());
        ctx.mode = Mode::Serial;
        after_each(parse.clone(), Arc::new(move |ctx: &mut Ctx| ctx.parse = saved.clone())).run(ctx)
    })
}

/// Runs `parse` over a stream and restores the previous context at the end (`withParseStream`)
pub fn with_parse_stream<A: Send + 'static>(more: impl Fn() -> StreamChunk + Send + Sync + 'static, parse: Trans<A>) -> Trans<A> {
    let more: Arc<dyn Fn() -> StreamChunk + Send + Sync> = Arc::new(more);
    Trans::new(move |ctx| {
        let saved = ctx.parse.clone();
        let m = more.clone();
        ctx.parse = ParseContext::of_stream(Box::new(move || m()));
        ctx.mode = Mode::Serial;
        let r = parse.run(ctx);
        ctx.parse = saved;
        r
    })
}

// ---------------------------------------------------------------------------------------------------------------
// primitives

/// Succeeds if what follows is exactly `s` (`string`, `symbol`)
pub fn string(s: &str) -> Trans<String> {
    let s = s.to_string();
    with_get_parse_string(move |text, eof| {
        if text.starts_with(&s) {
            Parsed::Ok(s.clone(), s.len())
        } else if !eof && s.starts_with(text) {
            Parsed::NeedMore
        } else {
            Parsed::Fail
        }
    })
}

/// Another name for `string` (`symbol`)
pub fn symbol(s: &str) -> Trans<String> {
    string(s)
}

/// Checks that the next character is `c` (`tChar`)
pub fn t_char(c: char) -> Trans<char> {
    with_get_parse_string(move |text, eof| match text.chars().next() {
        Some(x) if x == c => Parsed::Ok(c, c.len_utf8()),
        Some(_) => Parsed::Fail,
        None if eof => Parsed::Fail,
        None => Parsed::NeedMore,
    })
}

/// Reads a character; fails if nothing is left (`anyChar`)
pub fn any_char() -> Trans<char> {
    with_get_parse_string(|text, eof| match text.chars().next() {
        Some(c) => Parsed::Ok(c, c.len_utf8()),
        None if eof => Parsed::Fail,
        None => Parsed::NeedMore,
    })
}

/// The length of the integer with an optional sign (`-` or `+`) that starts the text, or `None`.
/// `Err(())` if it reaches the end of the text and it may go on (more is needed).
fn int_len(text: &str, eof: bool) -> Result<Option<usize>, ()> {
    let sign = usize::from(text.starts_with(['-', '+']));
    let digits = text[sign..].bytes().take_while(u8::is_ascii_digit).count();
    if sign + digits == text.len() && !eof {
        return Err(());
    }
    Ok((digits > 0).then_some(sign + digits))
}

fn parse_int<T: std::str::FromStr + Send + 'static>() -> Trans<T> {
    with_get_parse_string(|text, eof| match int_len(text, eof) {
        Err(()) => Parsed::NeedMore,
        Ok(None) => Parsed::Fail,
        Ok(Some(n)) => text[..n].trim_start_matches('+').parse().map_or(Parsed::Fail, |v| Parsed::Ok(v, n)),
    })
}

/// An integer, with an optional `+` or `-`; it does not skip spaces (`int`)
pub fn int() -> Trans<i64> {
    parse_int()
}

/// A big integer (up to 128 bits) (`integer`)
pub fn integer() -> Trans<i128> {
    parse_int()
}

/// A hexadecimal number without a prefix: `"ff"` is 255, `"0x1"` is 0 and leaves `x1` (`hex`)
pub fn hex() -> Trans<i64> {
    with_get_parse_string(|text, eof| {
        let n = text.bytes().take_while(u8::is_ascii_hexdigit).count();
        if n == text.len() && !eof {
            Parsed::NeedMore
        } else if n == 0 {
            Parsed::Fail
        } else {
            i64::from_str_radix(&text[..n], 16).map_or(Parsed::Fail, |v| Parsed::Ok(v, n))
        }
    })
}

/// A floating point number: optional sign, integer part (mandatory), optional `.fraction` and `e`/`E` exponent
/// (`double`). More complete than the original, which failed with exponents.
pub fn double() -> Trans<f64> {
    with_get_parse_string(|text, eof| {
        let b = text.as_bytes();
        let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
        let sign = usize::from(text.starts_with(['-', '+']));
        let int_part = digits(sign);
        if int_part == 0 {
            return if sign == text.len() && !eof { Parsed::NeedMore } else { Parsed::Fail };
        }
        let mut end = sign + int_part;
        // it needs to know what comes next to decide whether there is a fraction or an exponent
        if end == text.len() && !eof {
            return Parsed::NeedMore;
        }
        if b.get(end) == Some(&b'.') {
            let frac = digits(end + 1);
            if end + 1 + frac == text.len() && !eof {
                return Parsed::NeedMore;
            }
            if frac > 0 {
                end += 1 + frac;
            }
        }
        if end == text.len() && !eof {
            return Parsed::NeedMore;
        }
        if matches!(b.get(end), Some(b'e' | b'E')) {
            let esign = usize::from(matches!(b.get(end + 1), Some(b'-' | b'+')));
            let exp = digits(end + 1 + esign);
            if end + 1 + esign + exp == text.len() && !eof {
                return Parsed::NeedMore;
            }
            if exp > 0 {
                end += 1 + esign + exp;
            }
        }
        text[..end].parse().map_or(Parsed::Fail, |v| Parsed::Ok(v, end))
    })
}

/// Takes the characters that satisfy `cond`; if there is none it returns the empty string (`tTakeWhile`)
pub fn t_take_while(cond: impl Fn(char) -> bool + Send + Sync + 'static) -> Trans<String> {
    with_get_parse_string(move |text, eof| {
        let n = text.find(|c| !cond(c)).unwrap_or(text.len());
        if n == text.len() && !eof {
            Parsed::NeedMore
        } else {
            Parsed::Ok(text[..n].to_string(), n)
        }
    })
}

/// Like `t_take_while` and it also discards the next character (`tTakeWhile'`)
pub fn t_take_while_drop(cond: impl Fn(char) -> bool + Send + Sync + 'static) -> Trans<String> {
    with_get_parse_string(move |text, eof| {
        let n = text.find(|c| !cond(c)).unwrap_or(text.len());
        if n == text.len() && !eof {
            return Parsed::NeedMore;
        }
        let drop = text[n..].chars().next().map_or(0, char::len_utf8);
        Parsed::Ok(text[..n].to_string(), n + drop)
    })
}

/// Takes `n` characters, or the ones that are left (`tTake`)
pub fn t_take(n: usize) -> Trans<String> {
    with_get_parse_string(move |text, eof| {
        let end = text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
        if text.chars().count() < n && !eof {
            Parsed::NeedMore
        } else {
            Parsed::Ok(text[..end].to_string(), end)
        }
    })
}

/// Discards `n` characters (`tDrop`)
pub fn t_drop(n: usize) -> Trans<()> {
    t_take(n).map(|_| ())
}

/// Takes until `cond`, which receives what is left, holds; it does not consume that point. If it never holds it takes everything
/// (`tTakeUntil`). With a stream, `cond` sees only what has already been read.
pub fn t_take_until(cond: impl Fn(&str) -> bool + Send + Sync + 'static) -> Trans<String> {
    with_get_parse_string(move |text, eof| {
        let n = text.char_indices().map(|(i, _)| i).find(|&i| cond(&text[i..])).unwrap_or(text.len());
        if n == text.len() && !eof {
            Parsed::NeedMore
        } else {
            Parsed::Ok(text[..n].to_string(), n)
        }
    })
}

/// Discards until `cond` holds, without consuming that point (`tDropUntil`)
pub fn t_drop_until(cond: impl Fn(&str) -> bool + Send + Sync + 'static) -> Trans<()> {
    t_take_until(cond).map(|_| ())
}

/// Takes until `token`, which is consumed and discarded. It fails if it does not appear (`tTakeUntilToken`)
pub fn t_take_until_token(token: &str) -> Trans<String> {
    let token = token.to_string();
    with_get_parse_string(move |text, eof| match text.find(&token) {
        Some(i) => Parsed::Ok(text[..i].to_string(), i + token.len()),
        None if eof => Parsed::Fail,
        None => Parsed::NeedMore,
    })
}

/// Discards until `token`, which is also consumed. It fails if it does not appear (`tDropUntilToken`)
pub fn t_drop_until_token(token: &str) -> Trans<()> {
    t_take_until_token(token).map(|_| ())
}

/// Discards the spaces at the start (`dropSpaces`)
pub fn drop_spaces() -> Trans<()> {
    t_take_while(char::is_whitespace).map(|_| ())
}

/// Discards until the next line break, without consuming it (`dropTillEndOfLine`)
pub fn drop_till_end_of_line() -> Trans<()> {
    t_take_while(|c| c != '\n').map(|_| ())
}

/// Discards spaces and takes until the next space (`parseString`). With only spaces it gives the empty string.
pub fn parse_string() -> Trans<String> {
    tdo! {
        drop_spaces();
        t_take_while(|c| !c.is_whitespace())
    }
}

/// Adds `s` at the start of what is left to parse (`tPutStr`)
pub fn t_put_str(s: &str) -> Trans<()> {
    let s = s.to_string();
    Trans::new(move |ctx| {
        let old = ctx.parse.clone();
        let (rest, seen) = {
            let buf = old.src.buf.lock().unwrap();
            (buf[old.pos..].to_string(), buf.len())
        };
        // the new source goes on reading from the original stream what arrives after
        let src = old.src.clone();
        let seen = Mutex::new(seen);
        let more: Option<More> = src.more.is_some().then(|| {
            Box::new(move || {
                if !src.pull() {
                    return StreamChunk::Done;
                }
                let buf = src.buf.lock().unwrap();
                let mut seen = seen.lock().unwrap();
                let chunk = buf[*seen..].to_string();
                *seen = buf.len();
                if src.done.load(Ordering::SeqCst) {
                    StreamChunk::Last(chunk)
                } else {
                    StreamChunk::More(chunk)
                }
            }) as More
        });
        ctx.parse = ParseContext {
            src: Arc::new(Source { buf: Mutex::new(format!("{s}{rest}")), more, done: AtomicBool::new(false) }),
            pos: 0,
        };
        Some(())
    })
}

/// `true` if the stream has ended or nothing already read is left (`isDone`). It does not read more.
pub fn is_done() -> Trans<bool> {
    Trans::new(|ctx| {
        let buf = ctx.parse.src.buf.lock().unwrap();
        Some(ctx.parse.src.done.load(Ordering::SeqCst) || ctx.parse.pos >= buf.len())
    })
}

/// What is left to parse, reading the stream to the end; it consumes everything (`notParsed`). It does not work with infinite streams.
pub fn not_parsed() -> Trans<String> {
    Trans::new(|ctx| {
        while ctx.parse.src.pull() {}
        let buf = ctx.parse.src.buf.lock().unwrap();
        let rest = buf[ctx.parse.pos..].to_string();
        drop(buf);
        ctx.parse.pos += rest.len();
        Some(rest)
    })
}

/// What is left to parse, reading the stream to the end, **without consuming it** (`giveParseString`)
pub fn give_parse_string() -> Trans<String> {
    Trans::new(|ctx| {
        while ctx.parse.src.pull() {}
        let buf = ctx.parse.src.buf.lock().unwrap();
        Some(buf[ctx.parse.pos..].to_string())
    })
}

/// What has been read and not parsed, without reading more (`getParseBuffer`)
pub fn get_parse_buffer() -> Trans<String> {
    Trans::new(|ctx| {
        let buf = ctx.parse.src.buf.lock().unwrap();
        Some(buf[ctx.parse.pos..].to_string())
    })
}

/// Empties what has been read and not parsed (`clearParseBuffer`)
pub fn clear_parse_buffer() -> Trans<()> {
    Trans::new(|ctx| {
        ctx.parse.pos = ctx.parse.src.buf.lock().unwrap().len();
        Some(())
    })
}

// ---------------------------------------------------------------------------------------------------------------
// combinators

/// `op <$> a <*> b()` in serial: runs `a`, then what `b` builds (it is built when it runs, so recursive
/// definitions are not infinite) and combines the two results
fn then2<A, B, C>(a: Trans<A>, b: impl Fn() -> Trans<B> + Send + Sync + 'static, op: Arc<dyn Fn(A, B) -> C + Send + Sync>) -> Trans<C>
where
    A: Send + 'static,
    B: Send + 'static,
    C: Send + 'static,
{
    let b = Arc::new(b);
    tdo! {
        [op, b]
        let x = a.map(Taken::new);
        let y = b().map(Taken::new);
        Trans::new(move |_| Some(op(x.take()?, y.take()?)))
    }
}

/// Applies `p` until `end` succeeds; `end` is consumed. There may be no `p` at all (`manyTill`)
pub fn many_till<A: Send + 'static, B: Send + 'static>(p: Trans<A>, end: Trans<B>) -> Trans<Vec<A>> {
    chain_many_till(cons, p, end, Vec::new)
}

type Make<C> = Arc<dyn Fn() -> C + Send + Sync>;

/// Like `many_till`, with the way of chaining the pieces and how the value is built for when it ends
/// (`chainManyTill`)
pub fn chain_many_till<A, B, C>(
    op: impl Fn(A, C) -> C + Send + Sync + 'static,
    p: Trans<A>,
    end: Trans<B>,
    empty_val: impl Fn() -> C + Send + Sync + 'static,
) -> Trans<C>
where
    A: Send + 'static,
    B: Send + 'static,
    C: Send + 'static,
{
    chain_many_till_dyn(Arc::new(op), p, end, Arc::new(empty_val))
}

fn chain_many_till_dyn<A, B, C>(op: Arc<dyn Fn(A, C) -> C + Send + Sync>, p: Trans<A>, end: Trans<B>, empty_val: Make<C>) -> Trans<C>
where
    A: Send + 'static,
    B: Send + 'static,
    C: Send + 'static,
{
    // scan = (end >> return mempty) <|> (op <$> p <*> scan)
    let e2 = empty_val.clone();
    let done = tdo! {
        [e2]
        end.clone();
        Trans::new(move |_| Some(e2()))
    };
    let (p2, end2, op2) = (p.clone(), end, op.clone());
    let more = then2(p, move || chain_many_till_dyn(op2.clone(), p2.clone(), end2.clone(), empty_val.clone()), op);
    or(done, more)
}

/// Runs `open`, `p` and `close`, and gives the result of `p` (`between`)
pub fn between<O: Send + 'static, C: Send + 'static, A: Send + 'static>(open: Trans<O>, close: Trans<C>, p: Trans<A>) -> Trans<A> {
    let keep: Arc<dyn Fn(A, C) -> A + Send + Sync> = Arc::new(|a, _| a);
    tdo! {
        [p, close, keep]
        open;
        then2(p, { let close = close.clone(); move || close.clone() }, keep)
    }
}

pub fn parens<A: Send + 'static>(p: Trans<A>) -> Trans<A> {
    between(symbol("("), symbol(")"), p)
}
pub fn braces<A: Send + 'static>(p: Trans<A>) -> Trans<A> {
    between(symbol("{"), symbol("}"), p)
}
pub fn angles<A: Send + 'static>(p: Trans<A>) -> Trans<A> {
    between(symbol("<"), symbol(">"), p)
}
pub fn brackets<A: Send + 'static>(p: Trans<A>) -> Trans<A> {
    between(symbol("["), symbol("]"), p)
}
pub fn semi() -> Trans<String> {
    symbol(";")
}
pub fn comma() -> Trans<String> {
    symbol(",")
}
pub fn dot() -> Trans<String> {
    symbol(".")
}
pub fn colon() -> Trans<String> {
    symbol(":")
}

/// Zero or more `v` chained with `op` while `v` succeeds (`chainMany`)
pub fn chain_many<A, C>(op: impl Fn(A, C) -> C + Send + Sync + 'static, v: Trans<A>, empty_val: impl Fn() -> C + Send + Sync + 'static) -> Trans<C>
where
    A: Send + 'static,
    C: Send + 'static,
{
    chain_many_dyn(Arc::new(op), v, Arc::new(empty_val))
}

fn chain_many_dyn<A, C>(op: Arc<dyn Fn(A, C) -> C + Send + Sync>, v: Trans<A>, empty_val: Make<C>) -> Trans<C>
where
    A: Send + 'static,
    C: Send + 'static,
{
    // (op <$> v <*> chainMany op v) <|> return mempty
    let (v2, op2, e2, e3) = (v.clone(), op.clone(), empty_val.clone(), empty_val);
    or(
        then2(v, move || chain_many_dyn(op2.clone(), v2.clone(), e2.clone()), op),
        Trans::new(move |_| Some(e3())),
    )
}

/// One or more `p` separated by `sep`, chained with `op` (`chainSepBy1`)
pub fn chain_sep_by1<A, S, C>(
    op: impl Fn(A, C) -> C + Send + Sync + 'static,
    p: Trans<A>,
    sep: Trans<S>,
    empty_val: impl Fn() -> C + Send + Sync + 'static,
) -> Trans<C>
where
    A: Send + 'static,
    S: Send + 'static,
    C: Send + 'static,
{
    chain_sep_by1_dyn(Arc::new(op), p, sep, Arc::new(empty_val))
}

fn chain_sep_by1_dyn<A, S, C>(op: Arc<dyn Fn(A, C) -> C + Send + Sync>, p: Trans<A>, sep: Trans<S>, empty_val: Make<C>) -> Trans<C>
where
    A: Send + 'static,
    S: Send + 'static,
    C: Send + 'static,
{
    let (p2, op2) = (p.clone(), op.clone());
    // x <- p; xs <- chainMany op (try $ sep >> p); return (x `op` xs)
    then2(
        p,
        move || {
            let p = p2.clone();
            let next = try_(tdo! {
                [p]
                sep.clone();
                p
            });
            chain_many_dyn(op2.clone(), next, empty_val.clone())
        },
        op,
    )
}

/// Zero or more `p` separated by `sep`, chained with `op` (`chainSepBy`)
pub fn chain_sep_by<A, S, C>(
    op: impl Fn(A, C) -> C + Send + Sync + 'static,
    p: Trans<A>,
    sep: Trans<S>,
    empty_val: impl Fn() -> C + Send + Sync + 'static,
) -> Trans<C>
where
    A: Send + 'static,
    S: Send + 'static,
    C: Send + 'static,
{
    let empty_val: Make<C> = Arc::new(empty_val);
    let e = empty_val.clone();
    or(chain_sep_by1_dyn(Arc::new(op), p, sep, empty_val), Trans::new(move |_| Some(e())))
}

fn cons<A>(a: A, mut rest: Vec<A>) -> Vec<A> {
    rest.insert(0, a);
    rest
}

/// One or more `p` separated by `sep` (`sepBy1`)
pub fn sep_by1<A: Send + 'static, S: Send + 'static>(p: Trans<A>, sep: Trans<S>) -> Trans<Vec<A>> {
    chain_sep_by1(cons, p, sep, Vec::new)
}

/// Zero or more `p` separated by `sep` (`sepBy`)
pub fn sep_by<A: Send + 'static, S: Send + 'static>(p: Trans<A>, sep: Trans<S>) -> Trans<Vec<A>> {
    or(sep_by1(p, sep), Trans::new(|_| Some(Vec::new())))
}

/// Zero or more `p` separated by commas (`commaSep`)
pub fn comma_sep<A: Send + 'static>(p: Trans<A>) -> Trans<Vec<A>> {
    sep_by(p, comma())
}

/// Zero or more `p` separated by semicolons (`semiSep`)
pub fn semi_sep<A: Send + 'static>(p: Trans<A>) -> Trans<Vec<A>> {
    sep_by(p, semi())
}

pub fn comma_sep1<A: Send + 'static>(p: Trans<A>) -> Trans<Vec<A>> {
    sep_by1(p, comma())
}

pub fn semi_sep1<A: Send + 'static>(p: Trans<A>) -> Trans<Vec<A>> {
    sep_by1(p, semi())
}
