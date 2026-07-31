#![allow(warnings)]

pub use paste;
pub use fcomptime_macro::*;

#[cfg(feature = "async")]
pub use tokio;

pub use serde_json;

pub mod prelude;

use std::sync::{Mutex, OnceLock};
use std::collections::HashSet;
use std::backtrace::Backtrace;
use std::fmt;
use serde::Deserialize;

const RED: &str = "\x1b[41;1m";
const GREEN: &str = "\x1b[92m";
const YELLOW: &str = "\x1b[93m";
const BLUE: &str = "\x1b[94m";
const MAGENTA: &str = "\x1b[95m";
const CYAN: &str = "\x1b[96m";
const WHITE: &str = "\x1b[97m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

#[cfg(any(test, feature = "trace"))]
pub type Error = crate::TraceError;

#[cfg(not(any(test, feature = "trace")))]
pub type Error = Box<dyn std::error::Error>;

pub type Res<T = ()> = std::result::Result<T, Error>;

pub struct TraceError {
    pub inner: Box<dyn std::error::Error>,
    pub file: &'static str,
    pub line: u32,
    pub column: u32,
    pub backtrace: Backtrace,
    pub caller: bool,
    pub caller_thread: std::thread::ThreadId
}

impl fmt::Debug for TraceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.caller {
          writeln!(f, "{}{:?}{}\nOrigin: {}./{}:{}:{} thread id: {:?}{}", RED, self.inner, RESET, GREEN, self.file, self.line, self.column, self.caller_thread, RESET)?;
        } else {
          writeln!(f, "{}{:?}{}\nCaller: {}./{}:{}:{} thread id: {:?}{}", RED, self.inner, RESET, GREEN, self.file, self.line, self.column, self.caller_thread, RESET)?;
        }

        let bt = self.backtrace.to_string();
        let mut lines = bt.lines();

        let caller_file = self.file;
        let caller_line = self.line.to_string();

        let mut printed = false;
        let mut fallback: Option<String> = None;

        while let Some(_func) = lines.next() {
            if let Some(loc) = lines.next() {
                let l = loc.trim();

                if l.contains("src/")
                    && !l.contains("/rustc/")
                    && !l.contains("core/")
                    && !l.contains("std/")
                    && !l.contains("test/")
                    && !l.contains("FTest")
                {
                    if fallback.is_none() {
                        fallback = Some(l.to_string());
                    }
                    if !(l.contains(caller_file) && l.contains(&caller_line)) {
                        if let Some(loc) = l.strip_prefix("at ") {
                            writeln!(f, "Caller: {}{}{}", GREEN, loc, RESET)?;
                        } else {
                            writeln!(f, "Caller: {}{}{}", GREEN, l, RESET)?;
                        }
                        printed = true;
                        break;
                    }
                }
            }
        }

        if !printed {
            if let Some(l) = fallback {
                if let Some(loc) = l.strip_prefix("at ") {
                    writeln!(f, "Caller: {}{}{}", GREEN, loc, RESET)?;
                } else {
                    writeln!(f, "Caller: {}{}{}", GREEN, l, RESET)?;
                }
            }
        }

        Ok(())
    }
}

impl From<Box<dyn std::error::Error>> for TraceError {
    #[track_caller]
    fn from(err: Box<dyn std::error::Error>) -> Self {
        let loc = std::panic::Location::caller();
        Self {
            inner: err,
            file: loc.file(),
            line: loc.line(),
            column: loc.column(),
            backtrace: Backtrace::capture(),
            caller: true,
            caller_thread: std::thread::current().id()
        }
    }
}

impl From<String> for TraceError {
    #[track_caller]
    fn from(err: String) -> Self {
        let loc = std::panic::Location::caller();
        Self {
            inner: err.into(),
            file: loc.file(),
            line: loc.line(),
            column: loc.column(),
            backtrace: Backtrace::capture(),
            caller: true,
            caller_thread: std::thread::current().id()
        }
    }
}

impl From<std::io::Error> for TraceError {
    #[track_caller]
    fn from(err: std::io::Error) -> Self {
        let loc = std::panic::Location::caller();
        Self {
            inner: Box::new(err),
            file: loc.file(),
            line: loc.line(),
            column: loc.column(),
            backtrace: Backtrace::capture(),
            caller: true,
            caller_thread: std::thread::current().id()
        }
    }
}

impl From<serde_json::Error> for TraceError {
    #[track_caller]
    fn from(err: serde_json::Error) -> Self {
        let loc = std::panic::Location::caller();
        Self {
            inner: Box::new(err),
            file: loc.file(),
            line: loc.line(),
            column: loc.column(),
            backtrace: Backtrace::capture(),
            caller: true,
            caller_thread: std::thread::current().id()
        }
    }
}

impl From<&str> for TraceError {
    #[track_caller]
    fn from(err: &str) -> Self {
        let loc = std::panic::Location::caller();
        Self {
            inner: err.into(),
            file: loc.file(),
            line: loc.line(),
            column: loc.column(),
            backtrace: Backtrace::capture(),
            caller: true,
            caller_thread: std::thread::current().id()
        }
    }
}

#[macro_export]
macro_rules! init_comptime {
    () => {};
}

static COMPTIME_NAMES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

#[macro_export]
macro_rules! output {
    (str, $output:expr, $name:expr) => {
        #[cfg(test)]
        $crate::process_comptime(
            $output,
            $name,
            true,
            concat!(env!("CARGO_MANIFEST_DIR"), "/comptime/"),
        );
    };
    
    (raw, $output:expr, $name:expr) => {
        #[cfg(test)]
        $crate::process_comptime(
            $output,
            $name,
            false,
            concat!(env!("CARGO_MANIFEST_DIR"), "/comptime/"),
        );
    };
}

pub fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

pub fn unescape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[track_caller]
pub fn process_comptime<T: std::fmt::Display>(output: T, name: &str, is_str: bool, dir: &str) {
    let names = COMPTIME_NAMES.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    let mut names = match names.lock() {
        Ok(val) => val,
        Err(err) => err.into_inner(),
    };

    if !names.insert(name.to_string()) {
        let loc = std::panic::Location::caller();
        panic!("ERROR: Name '{}' is already exists! -> {}:{}:{}\n", name, loc.file(), loc.line(), loc.column());
    }

    let path = format!("{}{}", dir, name);
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent)
            .expect("ERROR: failed to create comptime directory");
    }

    let content = if is_str {
        format!("\"{}\"", escape_string(&output.to_string()))
    } else {
        output.to_string()
    };

    if let Err(err) = std::fs::write(&path, content) {
        let loc = std::panic::Location::caller();
        panic!("ERROR: {} -> {}:{}:{}\n", err, loc.file(), loc.line(), loc.column());
    }
}



#[macro_export]
macro_rules! call {
    (raw in, $name:literal, let mut $var:ident $body:block) => {
        #[allow(unexpected_cfgs)]
        {
            #[cfg(all(test, comptime_ready))]
            {
                let mut $var = include!(concat!(env!("CARGO_MANIFEST_DIR"), "/comptime/", $name));
                $body
            }
            
            #[cfg(all(test, not(comptime_ready)))]
            {
              let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/comptime/", $name));
                
                if path.exists() {
                } else {
                    std::eprintln!("comptime error: raw output not found yet");
                }
            }
        }
    };
    (raw in, $name:literal, let $var:ident $body:block) => {
        #[allow(unexpected_cfgs)]
        {
            #[cfg(all(test, comptime_ready))]
            {
                let $var = include!(concat!(env!("CARGO_MANIFEST_DIR"), "/comptime/", $name));
                $body
            }
            
            #[cfg(all(test, not(comptime_ready)))]
            {
              let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/comptime/", $name));
                
                if path.exists() {
                } else {
                    std::eprintln!("comptime error: raw output not found yet");
                }
            }
        }
    };
    (raw in, $name:literal, const $var:ident: $ty:ty $body:block) => {
        #[allow(unexpected_cfgs)]
        {
            #[cfg(all(test, comptime_ready))]
            {
                const $var: $ty = include!(concat!(env!("CARGO_MANIFEST_DIR"), "/comptime/", $name));
                $body
            }
            
            #[cfg(all(test, not(comptime_ready)))]
            {
              let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/comptime/", $name));
                
                if path.exists() {
                } else {
                    std::eprintln!("comptime error: raw output not found yet");
                }
            }
        }
    };
    (str in, $name:literal, $val:ident $body:block) => {
        #[cfg(test)]
        {
            if let Ok(content) = std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/comptime/",
                $name
            )) {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    let unquoted = trimmed
                        .strip_prefix('"')
                        .and_then(|s| s.strip_suffix('"'))
                        .unwrap_or(trimmed);
                    let $val = $crate::unescape_string(unquoted);
                    $body
                } else {
                    std::eprintln!("comptime error: output not found yet");
                }
            } else {
                std::eprintln!("comptime error: output not found yet");
            }
        }
    };
    (token, $name:literal, $default:expr) => {
        {
            #[allow(unreachable_code, unexpected_cfgs)]
            {
                #[cfg(all(test, not(comptime_ready)))]
                let _comptime_val = $default;

                #[cfg(any(not(test), comptime_ready))]
                let _comptime_val = $crate::comptime_include_expr!($name, $default);

                _comptime_val
            }
        }
    };
    (token, $name:literal) => {
        {
            #[allow(unreachable_code, unexpected_cfgs)]
            {
                #[cfg(all(test, not(comptime_ready)))]
                let _comptime_val = panic!("comptime error: output not found yet");

                #[cfg(any(not(test), comptime_ready))]
                let _comptime_val = $crate::comptime_include_expr!($name);

                _comptime_val
            }
        }
    };
    (full, $name:literal) => {
        #[cfg(test)]
        $crate::handle_default!();

        #[cfg(not(test))]
        $crate::comptime_include!($name);
    };
    (full, $name:literal, $($default:tt)*) => {
        #[cfg(test)]
        $crate::handle_default!($($default)*);

        #[cfg(not(test))]
        $crate::comptime_include!($name, $($default)*);
    };
    (partial, $name:literal, $($item:tt)*) => {
        #[cfg(not(test))]
        $crate::comptime_type!($name, $($item)*);
    };
    ($name:literal, $default:expr) => {
        {
            #[allow(unreachable_code, unexpected_cfgs)]
            {
                #[cfg(all(test, not(comptime_ready)))]
                let _comptime_val = $default;

                #[cfg(any(not(test), comptime_ready))]
                let _comptime_val = $crate::comptime_include_expr!($name, $default);

                _comptime_val
            }
        }
    };
    ($name:literal) => {
        {
            #[allow(unreachable_code, unexpected_cfgs)]
            {
                #[cfg(all(test, not(comptime_ready)))]
                let _comptime_val = panic!("comptime error: output not found yet");

                #[cfg(any(not(test), comptime_ready))]
                let _comptime_val = $crate::comptime_include_expr!($name);

                _comptime_val
            }
        }
    };
}

#[macro_export]
macro_rules! comptime_source {
    ($($t:tt)*) => {
        #[cfg(all(test, feature = "comptime"))]
        mod comptime_setup {
            #[allow(unused_imports)]
            use super::*;
            $crate::parse!($($t)*);
        }
    };
}

#[macro_export]
macro_rules! handle_default {
    ($($any:tt)*) => {
      $($any)*
    };
}

#[macro_export]
macro_rules! assign {
    ($name:literal) => {
      include!(concat!(env!("CARGO_MANIFEST_DIR"), "/comptime/", $name));
    };
}

#[macro_export]
macro_rules! parse {
	($name:ident { $($body:tt)* } $($rest:tt)*) => {
		#[test]
		fn $name() -> Result<(), $crate::TraceError> {
        $($body)*
        Ok(())
		}
		$crate::parse!($($rest)*);
	};
	($item:item $($rest:tt)*) => {
		$item
		$crate::parse!($($rest)*);
	};
	($e:expr; $($rest:tt)*) => {
		$e;
		$crate::parse!($($rest)*);
	};
	() => {};
}

#[macro_export]
macro_rules! call_scope {
    ($($t:tt)*) => {
        #[cfg(all(not(test), not(comptime_ready)))]
        #[allow(unexpected_cfgs)]
        {
            $($t)*
        }
    };
}

#[macro_export]
macro_rules! source {
    ($($t:tt)*) => {
        #[cfg(test)]
        {
            $($t)*
        }
    };
}

#[cfg(all(feature = "async"))]
#[macro_export]
macro_rules! async_source {
    ($($t:tt)*) => {
        #[cfg(test)]
        {
           let _ = async { $($t)* };
        }
    };
}

#[derive(Debug, Deserialize)]
pub struct Info {
    pub name: String,
    #[serde(default)]
    pub line: Option<usize>,
    #[serde(default)]
    pub generics: Vec<String>,
    #[serde(default, rename = "where")]
    pub where_clause: Vec<WhereClause>,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    #[serde(default, rename = "return_type")]
    pub return_type: Option<String>,
    #[serde(default)]
    pub callers: Vec<Caller>,
}

#[derive(Debug, Deserialize)]
pub struct WhereClause { pub generic: String, pub bounds: String }

#[derive(Debug, Deserialize)]
pub struct Parameter { pub name: String,
    #[serde(rename = "type")]
    pub type_: String }

#[derive(Debug, Deserialize)]
pub struct Caller {
    pub generics: Vec<String>,
    pub values: Vec<String>,
    pub line: usize,
}

pub fn read_comptime_data(path: &str) -> Option<Info> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Info>(&content).ok()
}

#[macro_export]
macro_rules! get {
    ($filename:expr) => {
        $crate::read_comptime_data(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/comptime/",
            $filename,
            ".json"
        ))
    };
}
#[cfg(test)]
mod tests {
    use crate::{escape_string, unescape_string};

    #[test]
    fn escape_unescape_roundtrip() {
        let s = "hello \"world\" with \\ backslash\nnewline\ttab\rreturn";
        let escaped = escape_string(s);
        assert_eq!(escaped, "hello \\\"world\\\" with \\\\ backslash\\nnewline\\ttab\\rreturn");
        assert_eq!(unescape_string(&escaped), s);
    }

    #[test]
    fn unescape_plain() {
        assert_eq!(unescape_string("plain text"), "plain text");
    }
}
