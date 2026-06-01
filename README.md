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
| **Shareable** | **Can share the comptime logic cross project/crate** So that it can be reused with different input. | **Fixed, can't share the compile time logic** Can only share the output. |
| **Nested** | **Support nested comptime** Can use comptime output inside other comptime. | **Can't use the compile time evaluation output inside other Crabtime macro** |
| **Impl And Trait Support** | **Support Impl and Trait** Can use comptime inside method that takes `self` parameter. | **Limited Support** Can only be used in assosiated function |
| **Parameter Types And Values Info** | **Currently can do some of them** Support for info about normal fn parameter types, normal fn parameter value, generic fn types, and generic fn values. It doesn't support method inside impl block without potential duplicate names and Trait yet. | **Can't know types and values info of fn and generic parameter** |

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
### `call!(str in, "name", any_name { // use any_name })`
### `call!(raw in, "name", let any_name { // use any_name })`

Embed the output in the current line.

- `call!("name")` — embed as expression (must be called inside call_scope)
- `call!(full, "name")` — embed as non expression (top level declaration).
- `call!(full, "name", default_expr)` — embed as non expression (top level declaration) with fallback item.
- `call!(partial, "name", code)` — embed partial token.
- `call!(str in, "name", any_name { // use any_name })` — call the result of other comptime inside other comptime (only support str/string, if want to make it to number use the build in method `.parse::<type>()`. can't embed raw token/syntax)
- `call!(raw in, "name", let any_name { // use any_name })` — call the result of other comptime inside other comptime as raw syntax as expression. Support `let name`, `let mut name`, `const name: type`

```rust
let result = call!("result");

call!(full, "result");

call!(full, "result", const A: i32 = 10;);

call!(partial, "result", 
    struct #1 {
      result: #2
    }
);

call!(str in, "name", any_name {
    // parse to number type if need to be number
    let int = any_name.parse::<i32>.unwrap();
});

call!(raw in, "name", let any_name {
    // use any_name
});
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

## How to use the comptime in Impl and Trait

Places the `#[comptime]` above the declaration, then writing the comptime macro normally

```rust
struct Data;

#[comptime]
impl Data {
  fn a() {
    ...
    source!{
      
    }
    ...
  }
  
  fn b() {
    ...
    source!{
      
    }
    ...
  }
}

#[comptime]
trait Trait {
  fn a() {
    ...
    source!{
      
    }
    ...
  }
  
  fn b() {
    ...
    source!{
      
    }
    ...
  }
}
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

## How to use comptime output inside other comptime

### By sharing the logic

Many callers can use the same logic, equivalent to many callers use the same output. 

```rust
fn shared(a: i32) -> i32 {
  a * 2
}

// user 1
#[comptime]
fn a1() {
  source! {
    let a = shared(10);
    ...
  }
}

// user 2
#[comptime]
fn a2() {
  source! {
    let a = shared(100);
    ...
  }
}
```

---

### By calling the output

Using `call!(raw in, "name", let var { })`. It can contains `let`, `let mut`, or `const: type`. This one can only be compiled with `cargo comptime run nested raw` because it has different step. Supports `run`, `check`, and `build`.

```rust
#[comptime]
fn a1() {
  source! {
    let a = 2 * 2;
    output!(raw, a, "a");
  }
}

#[comptime]
fn a2() {
  source! {
    call!(raw in, "a", const val: i32 {
        // use the val of a here
    });
  }
}

```

---

## Knowing Parameter Types And Values Info with `#[info]` and `get!()`

### `#[info`

Inspect the types and values of parameters (both `normal and generic`). `Currently only support pure function`. `It can be used inside impl and trait but the method name must be unique globally`. It must be placed above function that is the function declaration itself, and other function that call the declaration function

```rust
#[info]
fn a<T>(b: T) {
  
}

#[info]
fn b() {
  // b calls a
  a::<i32>(val);
}
```

---

### `get!()`

Gets the info of specific function. It returns `Option<Info>`.

```rust
#[info]
fn a<T>(b: T) {
  
}

#[info]
fn b() {
  // b calls a
  a::<i32>(val);
}

#[comptime]
fn main() {
  source! {
    let info_a = get!("a");
    // use the info
  }
}
```

Example of the info content :

```json
{
  "name": "my_function2",
  "line": 12,
  "generics": ["T", "U"],
  "where": [{"generic": "T", "bounds": ": Sync + std::fmt::Debug"}, {"generic": "U", "bounds": ": Send"}],
  "parameters": [{"name": "a", "type": "T"}, {"name": "b", "type": "U"}],
  "callers": [
    {
      "generics": ["i32", "&'static str"],
      "values": ["1000", "\"uwu\""],
      "line": 28
    },
    {
      "generics": ["i32", "&'static str"],
      "values": ["1000", "\"uwu\""],
      "line": 30
    }
  ]
}
```

---

## How to do println debugging in the source macro

Don't use any kind of println because it will not stop the step and print the message. But uses `panic!()`, it will stop the process and print the message

---

## Only active when `cfg(all(test, feature = "comptime"))`.

The feature `comptime` makes sure the comptime code and pure test code can run be run independently

---

## `cargo-comptime` Subcommand

Install once:

```bash
cargo install --git https://github.com/fuji-184/F_Comptime cargo-comptime
```

Then use:

```bash
cargo comptime check
cargo comptime build
cargo comptime build --release
cargo comptime run
cargo comptime init config
cargo comptime path/to/comptime.config
cargo comptime check nested raw
cargo comptime run nested raw
cargo comptime build nested raw --release
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
fcomptime = { git = "https://github.com/fuji-184/F_Comptime.git" }

[features]
# important to toggle comptime mode and normal mode
comptime = []
```

### Call `init_comptime!()` 1 time in the crate root

```rust
use fcomptime::prelude::*;

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

If calling comptime output in other comptime by calling the output not shared logic method, using manual run is complex. Uses `Cargo-Comptime` instead that automates nested raw compilation

### Auto Run With Cargo-Comptime

```bash
cargo comptime run
```

Or using cargo check in both methods

```bash
cargo comptime check
```

Nested raw

```bash
cargo comptime run nested raw
```

---

## Add `/comptime/` folder to `.gitignore` to exclude it from the commit

```
/comptime/
```

---

## Known Limitation
same limitation in Crabtime about these ones
- Can not use comptime output inside other comptime as non expression raw token/syntax yet
- Can not know the parameter types and values
- Can not know generic types and values

---

## Road Map

- Add F_Comptime support in [R_Lib](https://github.com/fuji-184/RLib)
- Figuring how to make using comptime output inside other comptime as non expression raw token/syntax possible
- Figuring how to make knowing parameter types and values possible
- Figuring how to make knowing generic types and values possible

---
