//! `transient-loggable`: values as they are written in the log (compatible with the Haskell nodes).
//!
//! The log is text: each value is followed by `/`. Two kinds of values:
//!
//! * **Atoms**, in the Haskell format (`show`), checked by running the original: integers (`42`, `-5`),
//!   floats with Haskell's `show` (`3.5`, `1.0e-2`, `NaN`), booleans `t`/`f`, the unit `u` and characters
//!   as a Haskell literal (`'x'`, `'\n'`, `'\233'`).
//! * **Everything else** (strings, lists, tuples, `Option`, structures): **plain JSON**, with each `/` doubled
//!   (`//`) so that it is not mistaken for the separator. A single `/` closes the value. No Haskell `show`.
//!
//! Two points where this departs from what the original writes today (which has bugs, see the README of
//! `transient/tests/original-vs-rust`): strings are JSON (the original writes truncated Latin-1, does not double
//! slashes in `Text` and does not tell the empty string from `"[]"`), and the character `/` is written `'\47'` (the original
//! writes `'/'`, which it can not read back itself).

use serde::{de::DeserializeOwned, Serialize};

/// A value that can be written to the log and read from it
pub trait Loggable: Sized {
    /// Appends the value to `out` (without the final separator)
    fn serialize(&self, out: &mut String);

    /// Reads a value from the start of `input` and advances to the character that follows it (the separator `/` or the end)
    fn deserialize(input: &mut &str) -> Option<Self>;
}

/// The text of a value
pub fn to_log<T: Loggable>(x: &T) -> String {
    let mut s = String::new();
    x.serialize(&mut s);
    s
}

/// Reads a value from `s` and returns what follows
pub fn from_log<T: Loggable>(s: &str) -> Option<(T, &str)> {
    let mut input = s;
    let x = T::deserialize(&mut input)?;
    Some((x, input))
}

// ---------------------------------------------------------------------------------------------------------------
// doubling slashes

/// Doubles every `/` (`dupSlash`)
pub fn dup_slash(s: &str) -> String {
    s.replace('/', "//")
}

/// Reads up to the first undoubled `/`, undoing the doubled ones (`undupSlash`). Leaves `input` at that `/` or at the end.
pub fn undup_slash(input: &mut &str) -> String {
    let mut out = String::new();
    let mut rest = *input;
    loop {
        match rest.find('/') {
            None => {
                out.push_str(rest);
                rest = "";
                break;
            }
            Some(i) => {
                out.push_str(&rest[..i]);
                if rest[i..].starts_with("//") {
                    out.push('/');
                    rest = &rest[i + 2..];
                } else {
                    rest = &rest[i..];
                    break;
                }
            }
        }
    }
    *input = rest;
    out
}

/// The text up to the first `/` (undoing nothing), for the atoms
fn token<'a>(input: &mut &'a str) -> &'a str {
    let end = input.find('/').unwrap_or(input.len());
    let (t, rest) = input.split_at(end);
    *input = rest;
    t
}

// ---------------------------------------------------------------------------------------------------------------
// atoms

macro_rules! loggable_int {
    ($($t:ty),*) => {$(
        impl Loggable for $t {
            fn serialize(&self, out: &mut String) {
                out.push_str(&self.to_string());
            }
            fn deserialize(input: &mut &str) -> Option<Self> {
                let mut copy = *input;
                let t = token(&mut copy);
                let v = t.parse().ok()?;
                *input = copy;
                Some(v)
            }
        }
    )*};
}
loggable_int!(i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize);

impl Loggable for () {
    fn serialize(&self, out: &mut String) {
        out.push('u');
    }
    fn deserialize(input: &mut &str) -> Option<Self> {
        *input = input.strip_prefix('u')?;
        Some(())
    }
}

impl Loggable for bool {
    fn serialize(&self, out: &mut String) {
        out.push(if *self { 't' } else { 'f' });
    }
    fn deserialize(input: &mut &str) -> Option<Self> {
        let v = match input.chars().next()? {
            't' => true,
            'f' => false,
            _ => return None,
        };
        *input = &input[1..];
        Some(v)
    }
}

/// Haskell's `show` for a float, from its shortest digits that identify it (`floatToDigits`):
/// decimal notation if the value is in [0.1, 10^7) and scientific (`d.ddde<exp>`) otherwise
fn haskell_show_float(digits_and_exp: Option<(String, i32)>, neg: bool, special: Option<&str>) -> String {
    if let Some(s) = special {
        return s.to_string();
    }
    let sign = if neg { "-" } else { "" };
    let Some((ds, e)) = digits_and_exp else { return format!("{sign}0.0") };
    // the value is 0.d1d2d3... × 10^e
    if (0..=7).contains(&e) {
        let e = e as usize;
        let body = if e == 0 {
            format!("0.{ds}")
        } else if ds.len() <= e {
            format!("{ds}{}.0", "0".repeat(e - ds.len()))
        } else {
            format!("{}.{}", &ds[..e], &ds[e..])
        };
        format!("{sign}{body}")
    } else {
        let rest = if ds.len() > 1 { &ds[1..] } else { "0" };
        format!("{sign}{}.{rest}e{}", &ds[..1], e - 1)
    }
}

/// The shortest digits and the exponent (value = 0.ds × 10^e) of a finite, non-zero float
fn float_digits(sci: &str) -> (String, i32) {
    // `{:e}` gives the format "d.ddde<exp>" with the shortest digits that identify it
    let (mant, exp) = sci.split_once('e').unwrap();
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    (digits, exp.parse::<i32>().unwrap() + 1)
}

macro_rules! loggable_float {
    ($t:ty) => {
        impl Loggable for $t {
            fn serialize(&self, out: &mut String) {
                let x = *self;
                let special = if x.is_nan() {
                    Some("NaN")
                } else if x.is_infinite() {
                    Some(if x > 0.0 { "Infinity" } else { "-Infinity" })
                } else {
                    None
                };
                let digits = (x != 0.0 && special.is_none()).then(|| float_digits(&format!("{:e}", x.abs())));
                out.push_str(&haskell_show_float(digits, x.is_sign_negative(), special));
            }
            fn deserialize(input: &mut &str) -> Option<Self> {
                let mut copy = *input;
                let t = token(&mut copy);
                let v = match t {
                    "NaN" => <$t>::NAN,
                    "Infinity" => <$t>::INFINITY,
                    "-Infinity" => <$t>::NEG_INFINITY,
                    _ if t.chars().all(|c| c.is_ascii_digit() || "+-.eE".contains(c)) => t.parse().ok()?,
                    _ => return None,
                };
                *input = copy;
                Some(v)
            }
        }
    };
}
loggable_float!(f64);
loggable_float!(f32);

const ASCII_NAMES: [&str; 33] = [
    "NUL", "SOH", "STX", "ETX", "EOT", "ENQ", "ACK", "BEL", "BS", "HT", "LF", "VT", "FF", "CR", "SO", "SI", "DLE", "DC1",
    "DC2", "DC3", "DC4", "NAK", "SYN", "ETB", "CAN", "EM", "SUB", "ESC", "FS", "GS", "RS", "US", "SP",
];

/// Haskell's `show` for a `Char`, except `/`, which is written `'\47'` so that the separator does not confuse it
fn haskell_show_char(c: char) -> String {
    let body = match c {
        '\'' => "\\'".to_string(),
        '/' => "\\47".to_string(),
        '\\' => "\\\\".to_string(),
        '\x7f' => "\\DEL".to_string(),
        '\x07' => "\\a".to_string(),
        '\x08' => "\\b".to_string(),
        '\x0c' => "\\f".to_string(),
        '\n' => "\\n".to_string(),
        '\r' => "\\r".to_string(),
        '\t' => "\\t".to_string(),
        '\x0b' => "\\v".to_string(),
        c if (c as u32) < 32 => format!("\\{}", ASCII_NAMES[c as usize]),
        c if (c as u32) > 127 => format!("\\{}", c as u32),
        c => c.to_string(),
    };
    format!("'{body}'")
}

fn haskell_read_char(s: &str) -> Option<char> {
    let body = s.strip_prefix('\'')?.strip_suffix('\'')?;
    let Some(esc) = body.strip_prefix('\\') else {
        let mut it = body.chars();
        let c = it.next()?;
        return it.next().is_none().then_some(c);
    };
    let simple = match esc {
        "\\" => Some('\\'),
        "'" => Some('\''),
        "\"" => Some('"'),
        "a" => Some('\x07'),
        "b" => Some('\x08'),
        "f" => Some('\x0c'),
        "n" => Some('\n'),
        "r" => Some('\r'),
        "t" => Some('\t'),
        "v" => Some('\x0b'),
        "DEL" => Some('\x7f'),
        _ => None,
    };
    if simple.is_some() {
        return simple;
    }
    if let Some(i) = ASCII_NAMES.iter().position(|n| *n == esc) {
        return char::from_u32(i as u32);
    }
    if let Some(hex) = esc.strip_prefix('x') {
        return char::from_u32(u32::from_str_radix(hex, 16).ok()?);
    }
    if let Some(oct) = esc.strip_prefix('o') {
        return char::from_u32(u32::from_str_radix(oct, 8).ok()?);
    }
    char::from_u32(esc.parse().ok()?)
}

impl Loggable for char {
    fn serialize(&self, out: &mut String) {
        out.push_str(&haskell_show_char(*self));
    }
    fn deserialize(input: &mut &str) -> Option<Self> {
        let mut copy = *input;
        let t = token(&mut copy);
        let c = haskell_read_char(t)?;
        *input = copy;
        Some(c)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// JSON: strings, structures, lists, tuples...

/// A value written as JSON (with the slashes doubled). For your own structures
/// `loggable_json!(MyType);` is enough, or `Json(value)` to write a loose one.
#[derive(Clone, Debug, PartialEq)]
pub struct Json<T>(pub T);

fn serialize_json<T: Serialize>(x: &T, out: &mut String) {
    out.push_str(&dup_slash(&serde_json::to_string(x).expect("serializable to JSON")));
}

fn deserialize_json<T: DeserializeOwned>(input: &mut &str) -> Option<T> {
    let mut copy = *input;
    let text = undup_slash(&mut copy);
    let v = serde_json::from_str(&text).ok()?;
    *input = copy;
    Some(v)
}

impl<T: Serialize + DeserializeOwned> Loggable for Json<T> {
    fn serialize(&self, out: &mut String) {
        serialize_json(&self.0, out);
    }
    fn deserialize(input: &mut &str) -> Option<Self> {
        deserialize_json(input).map(Json)
    }
}

/// Makes a type with serde's `Serialize` and `Deserialize` a `Loggable`, written as JSON
#[macro_export]
macro_rules! loggable_json {
    ($($t:ty),* $(,)?) => {$(
        impl $crate::Loggable for $t {
            fn serialize(&self, out: &mut String) {
                $crate::Json::serialize_ref(self, out)
            }
            fn deserialize(input: &mut &str) -> Option<Self> {
                $crate::Json::<$t>::deserialize(input).map(|j| j.0)
            }
        }
    )*};
}

impl<T: Serialize + DeserializeOwned> Json<T> {
    #[doc(hidden)]
    pub fn serialize_ref(x: &T, out: &mut String) {
        serialize_json(x, out)
    }
}

macro_rules! loggable_as_json {
    ($(($($g:ident),*) $t:ty;)*) => {$(
        impl<$($g: Serialize + DeserializeOwned),*> Loggable for $t {
            fn serialize(&self, out: &mut String) {
                serialize_json(self, out)
            }
            fn deserialize(input: &mut &str) -> Option<Self> {
                deserialize_json(input)
            }
        }
    )*};
}

loggable_as_json! {
    (T) Vec<T>;
    (T) Option<T>;
    (A, B) (A, B);
    (A, B, C) (A, B, C);
    (A, B, C, D) (A, B, C, D);
}

/// A string is written as JSON. It is also read from raw text, up to the first undoubled `/`: that is what comes in a URL
/// (`/enterw/alice`) or is typed in the console, and what the logs of the first version had. It is never written like that.
impl Loggable for String {
    fn serialize(&self, out: &mut String) {
        serialize_json(self, out)
    }
    fn deserialize(input: &mut &str) -> Option<Self> {
        if input.starts_with('"') {
            deserialize_json(input)
        } else {
            Some(undup_slash(input))
        }
    }
}

// any JSON value (`Loggable Value` of `Transient.Move.JSON`)
loggable_json!(serde_json::Value);

impl<K: Serialize + DeserializeOwned + Ord, V: Serialize + DeserializeOwned> Loggable for std::collections::BTreeMap<K, V> {
    fn serialize(&self, out: &mut String) {
        serialize_json(self, out)
    }
    fn deserialize(input: &mut &str) -> Option<Self> {
        deserialize_json(input)
    }
}
