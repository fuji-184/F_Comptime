# F_Comptime

F_Comptime is a Rust **compile time code evaluation and generation** library. It runs Rust code at test time, evaluating expressions, generating values or syntaxs, and producing files. Then includes those results at compile time in debug/release builds

---

## How It Works

```
cargo test --features=comptime  →  evaluates code, generates ./comptime/* files
cargo build                    →  includes those files via include!()
```

The key idea: anything that would be expensive or complex to compute at runtime can be **evaluated once during test**, saved to a file, and **included as a literal at compile time**. Easier than writing macro. No proc macro to speed up compile speed.

---

## Setup

`Cargo.toml`:
```toml
[dependencies]
fcomptime = { git = "https://github.com/fuji-184/F_Comptime.git" }

# important, add the feature name `comptime` so that comptime can run independently from other test code
[features]
comptime = []
```

`main.rs` / `lib.rs`:
```rust
use fcomptime::*;
init_comptime!();
```

---

## Macros

### `init_comptime!()`
Initializes the internal mutex registry. Must be called once at crate root

```rust
init_comptime!();
```

---

### `comptime!(blocks)` — Define evaluators and generators
Defines code evaluation and generation blocks. Each named block becomes a test function that runs during `cargo test --features=comptime`. It can be used to evaluate functions, generate structs, build strings, or produce any Rust expression. The output is saved to a file and included at compile time

Example :

```rust
comptime!(
    use std::fmt::Write;

    // generate a struct definition
    any_name {
        let mut out = String::from("struct Test {");
        write!(out, "x: i32,").unwrap();
        out.push_str("}");
        comptime_output!(raw, out, "the_key_name");
    }

    // evaluate a function and save the result
    any_name {
        let result = some_fn(42);
        comptime_output!(raw, result, "the_key_name2");
    }
);
```

---

### `comptime!("name")` — Include the result. Returns `Default::default()` in test builds

```rust
let x = comptime!("the_key_name");
```

---

### `comptime!(full, "name")` — Include the result as a top level item (eg struct, impl). Use for declarations that cannot appear inside expressions

```rust
comptime!(full, "the_key_name");
```

Optionally accepts a default for test builds:
```rust
comptime!(full, "my_struct", struct MyStruct { x: i32 });
```

---

### `comptime_output!(tag, value, "name")` — write the result to `./comptime/<name>`. Only runs during test

| Tag | Behavior |
|-----|----------|
| `raw` | Writes value as is | use raw for number and syntax
| `str` | Wraps value in quotes `"..."` | use str for str/string only

```rust
comptime_output!(raw, value, "my_value");  // writes: 42
comptime_output!(str, value, "my_value");  // writes: "hello"
```

- Panics if the same name is written twice (duplicate detection)

---

### `comptime_scope!(...)` — Release only block
Wraps code that should only run in non test builds

```rust
comptime_scope!(
    let val = comptime!("my_value");
    println!("{}", val);
);
```

---

## Full Example

```rust
use comptime::*;
init_comptime!();

// includes generated struct at compile time
comptime!(full, "my_struct",);

fn compute(n: i32) -> i32 {
    n * n
}

fn main() {
    comptime_scope!(
        let result = comptime!("computed");  // includes evaluated result
        let s = MyStruct { x: result };
        println!("{}", s.x);
    );
}

// define what to generate and evaluate
comptime!(
    use std::fmt::Write;

    // generate a struct from code
    my_struct {
        let mut out = String::from("struct MyStruct {");
        out.push_str("x: i32,");
        out.push_str("}");
        comptime_output!(raw, out, "my_struct");
    }

    // evaluate a function at test time, save result
    computed {
        let result = compute(9);  // evaluates to 81
        comptime_output!(raw, result, "computed");
    }
);
```

**Evaluate and generate:**
```bash
cargo test --features=comptime
```

**Build with results included:**
```bash
cargo build
```

---

## Optional Error Types

| Type | Active when |
|------|-------------|
| `TraceError` | `cfg(test)` or `feature = "trace"` |
| `Box<dyn Error>` | release builds |

`Res<T>` is an alias for `Result<T, Error>`.

```rust
fn my_block() -> Res {
    Ok(())
}
```

`TraceError` includes file, line, column, thread id, and backtrace for easier debugging.

---

## Run Cargo-Comptime

Easier comptime compile pipeline with cargo-comptime

### Install cargo-comptime

```bash
cargo install --git https://github.com/fuji-184/F_Comptime.git cargo-comptime
```

## How To Use Cargo-Comptime To Compile The Comptime

### Compile comptime and run cargo check

```bash
cargo comptime check
```

### Compile comptime and run cargo run

```bash
cargo comptime run
```

### Compile comptime and run cargo build (debug build)

```bash
cargo comptime build
```

### Compile comptime and run cargo build --release

```bash
cargo comptime build --release
```

### Generate custom command config file

```bash
cargo comptime init config
```

### Compile comptime and run custom command

```bash
cargo comptime path_to_the_config

# for example

cargo comptime comptime.config
```