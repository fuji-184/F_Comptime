#![allow(warnings)]

pub use paste;
pub use fcomptime_macro::*;

use std::sync::{Mutex, OnceLock};
use std::collections::HashSet;
use std::backtrace::Backtrace;
use std::fmt;

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

        while let Some(_func) = lines.next() {
            if let Some(loc) = lines.next() {
                let l = loc.trim();

                if self.caller {
                  
                if l.contains("src/")
                    && !l.contains("/rustc/")
                    && !l.contains("core/")
                    && !l.contains("std/")
                    && !l.contains("test/")
                    && !l.contains("FTest")
                {
                    if !(l.contains(caller_file) && l.contains(&caller_line)) {
                        if let Some(loc) = l.strip_prefix("at ") {
                          writeln!(f, "Caller: {}{}{}", GREEN, loc, RESET)?;
                        } else {
                          writeln!(f, "Caller: {}{}{}", GREEN, l, RESET)?;
                        }
                        break;
                    }
                }
                
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
    () => {
        #[cfg(test)]
        pub(crate) static comptime_NAMES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = 
            std::sync::OnceLock::new();
    };
}

#[macro_export]
macro_rules! comptime_output {
    (str, $output:expr, $name:expr) => {{
        if cfg!(test) {
            $crate::process_comptime(&crate::comptime_NAMES, $output, $name, true);
        } else {
            
        }
    }};
    
    (raw, $output:expr, $name:expr) => {{
        if cfg!(test) {
            $crate::process_comptime(&crate::comptime_NAMES, $output, $name, false);
        } else {
            
        }
    }};
}

#[track_caller]
pub fn process_comptime<T: std::fmt::Display>(
    mutex_lock: &std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>>,
    output: T, 
    name: &str, 
    is_str: bool
) {
    let mutex = mutex_lock.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    let mut names = match mutex.lock() {
      Ok(val) => val,
      Err(err) => {
        panic!("ERROR: '{}', failed to lock mutex because there is panic, please fix the panic to make mutex lock successfully\n", err);
      }
    };

    if !names.insert(name.to_string()) {
        let loc = std::panic::Location::caller();
        panic!("ERROR: Name '{}' is already exists! -> {}:{}:{}\n", name, loc.file(), loc.line(), loc.column());
    }

    std::fs::create_dir_all("./comptime").unwrap();
    let path = format!("./comptime/{}", name);

    if is_str {
        if let Err(err) = std::fs::write(path, format!("\"{}\"", output)) {
          let loc = std::panic::Location::caller();
          panic!("ERROR: {} -> {}:{}:{}\n", err, loc.file(), loc.line(), loc.column());
        }
    } else {
        if let Err(err) = std::fs::write(path, format!("{}", output)) {
          let loc = std::panic::Location::caller();
          panic!("ERROR: {} -> {}:{}:{}\n", err, loc.file(), loc.line(), loc.column());
        }
    }
}

#[macro_export]
macro_rules! comptime {
    (full, $name:literal) => {
        #[cfg(test)]
        $crate::handle_default!();

        #[cfg(not(test))]
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/comptime/", $name));
    };
    (full, $name:literal, $($default:tt)*) => {
        #[cfg(test)]
        $crate::handle_default!($($default)*);

        #[cfg(not(test))]
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/comptime/", $name));
    };
    ($name:literal) => {
        if cfg!(test) {
            //$crate::handle_default!($($default)*)
            Default::default()
        } else {
            include!(concat!(env!("CARGO_MANIFEST_DIR"), "/comptime/", $name))
        }
    };
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
	() => {};
}

#[macro_export]
macro_rules! comptime_scope {
    ($($t:tt)*) => {
        #[cfg(not(test))]
        {
            $($t)*
        }
    };
}