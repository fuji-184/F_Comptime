# F_Comptime

F_Comptime is a compile-time execution framework for Rust that allows you to run arbitrary code during compilation and seamlessly embed the outputs back into your codebase. It bridges the gap between compile-time evaluation and full runtime capabilities.

---

## Key Features & Purpose

### 1. Unrestricted Compile-Time Power

Can run code that contains loops, dynamic allocations, complex conditional logic, file system operations, or networking at compile time. No scope limitation, can call any code or library that is used in the main crate, without duplicated compilation artifacts.

### 2. Easy to use

Does not need to write complex procedural macro syntax when want to generate values, tokens, or types dynamically.

### The key insight

It uses **Rust's own test harness** as the execution engine, not a separate project evaluation environment.

---

## Comparison to Crabtime

While library like Crabtime solve a similar problem, F_Comptime solves the limitations in Crabtime

| Feature | F_Comptime | Crabtime |
| --- | --- | --- |
| **Accessibility** | **Can use any other code in the project.** Using Rust test, it can access other functions, structs, type, even code from different files as long as it is imported in the current file, just like normal Rust coding. It has the same capability of a test function | **Can't use code outside the macro** Using new separated project, it has the same limitation of proc macro in the aspect of running as separated project |
| **Partial Embed** | **Can embed token partially** Using macro to embed multiple raw tokens to code partially. | **Can't embed partially**. |
| **Async Support** | **Can use async code** Using tokio test to run async-await. | **Need additional setup** Need to setup async dependency and async runtime creation for every Crabtime macro scope. |
| **Dependency** | **Shared dependencies with the main crate** Reuse the same compilation cache of the main crate dependencies. | **Copy dependencies to new separated project** Duplicating compilation artifacts that consumes SSD space. |

## Crates

| Crate | Role |
|---|---|
| `fcomptime` | Main library |
| `fcomptime_macro` | Internal proc-macro crate |
| `cargo-comptime` | Cargo subcommand — orchestrates test → build pipeline |

---

## Macros Reference

### `init_comptime!()`

Declares the global name registry used to prevent duplicate output names. Call once at the crate root.

```rust
init_comptime!();
```

---

### `#[comptime]` (proc-macro attribute)

Placed on a function, it enables the code inside the function can contain code that runs before compile time.

```rust
#[comptime]
fn function() {
    
}
```

---

### `source! { ... }`

The code that will be run at compile time. It can uses other code ouside the block.

```rust
#[comptime]
fn function() {
    let data = load_data();
    
    source! {
        // do something with data
    }
}
```

---

### `async_source! { ... }`

Async version of `source! { ... }`. Can call async code

Setup: Enables the feature `async`

```rust
#[comptime]
fn function() {
    async_source {
        // the async code
    }
}
```

---

### `output! { ... }`

To set the output of the evaluation.
- `str` — wraps the value in quotes : `"value"`
- `raw` — writes the value as is (for numeric amd raw code)
- separate the output with comma `,` for use in partial call

```rust
#[comptime]
fn function() {
    let data = load_data();
    
    source! {
        data
        output!(raw, data.len(), "data_len");
    }
    
    source! {
      output!(str, "hello world", "greeting");
    }
}
```

---

### `call_scope! { ... }`

The scope of the output and any code that uses the output. Used to toggle between test run and normal run.

---

### `call!("name")`
### `call!(full, "name")`
### `call!(full, "name", default)`
### `call!(partial, "name", code)`

Embed the output in the current line.

- `call!("name")` — embed as expression (must be called inside call_scope)
- `call!(full, "name")` — embed as non expression (top level declaration).
- `call!(full, "name", default_expr)` — embed as non expression (top level declaration) with fallback item.
- `call!(partial, "name", code)` — embed partial token.

```rust
let result = call!("result");

call!(full, "result");

call!(full, "result", const A: i32 = 10;);

call!(partial, "result", 
    struct #1 {
      result: #2
    }
);
```

---


### `comptime_source! { ... }`

Creates top level comptime source declaration. It is like `build.rs` but can call code from main crate directly and put back some output to main crate

```rust
comptime_source! {
    read_config {
        let res = std::fs::read_to_string("./config").unwrap();
        
        // do some logic with the read result
        
        // return some output to the code and call it anywhere
        output!(raw, some_output, "my_output");
    }
}
```

---

Only active when `cfg(all(test, feature = "comptime"))`.

The feature `comptime` makes sure the comptime code and pure test code can run be run independently

---

## `cargo-comptime` Subcommand

Install once:

```bash
cargo install --path ./cargo-comptime
```

Then use:

```bash
cargo comptime check
cargo comptime build
cargo comptime build --release
cargo comptime run
cargo comptime init config
cargo comptime path/to/comptime.config
```

### Cargo-Comptime Config

```bash
cargo comptime init config
```

Creates `comptime.config`:

```
# Define custom commands here, for example
clear
cargo build --release
```

Run by passing the config path:

```bash
cargo comptime comptime.config
```

---

### The Generated Result

The results can be inspected easily inside folder `comptime` in the same folder of `Cargo.toml`. 

---

## Step-by-Step Usage Guide

Walkthrough to initialize and run the project

### Add feature `comptime` to your `Cargo.toml`

```toml
[dependencies]
fcomptime = "0.1"

[features]
# important to toggle comptime mode and normal mode
comptime = []
```

### Call `init_comptime!()` 1 time in the crate root

```rust
use fcomptime::{
    comptime,
    source,
    output,
    call_scope,
    call,
    comptime_source
};

init_comptime!();

fn main() {
  
}
```

### Writing code as usual. Reference the macro reference above to use the comptime

### Running

#### Manual Run

```bash
cargo test --features=comptime
cargo run
```

### Auto Run With Cargo-Comptime

```bash
cargo comptime run
```

Or using cargo check in both methods

```bash
cargo comptime check
```

---

## How to share the comptime logic cross project/crate

* write the code in pure function, without any comptime macro
* then the caller is the one that turns it into comptime mode by calling it inside comptime macro

```rust
// project a
pub fn code_like_usual(input: i32) -> i32 {
    input * 2
}

// project b
use a::code_like_usual;

#[comptime]
fn any_name() {
    source! {
        let res = code_like_usual(10);
        output!(raw, res, "my_output");
    }

    call_scope! {
        let res = call!("my_output");
        println!("{}", res);
    }
}
```

---

## Add `/comptime/` folder to `.gitignore` to exclude it from the commit

```
/comptime/
```

---

## Known Limitation
- Can not use comptime output inside other comptime yet (same limitation in Crabtime about this one)

---

## Road Map

- Add F_Comptime support in [R_Lib](https://github.com/fuji-184/RLib)
- Figuring how to make using comptime output in other comptime possible

---
