# comptime

FComptime is a Rust **compile time code evaluation and generation** library. It runs Rust code at test time, evaluating expressions, generating values or syntaxs, and producing files. Then includes those results at compile time in debug/release builds.

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
fcomptime = { path = "..." }

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
Initializes the internal mutex registry. Must be called once at crate root.

```rust
init_comptime!();
```

---

### `comptime!(blocks)` — Define evaluators and generators
Defines code evaluation and generation blocks. Each named block becomes a test function that runs during `cargo test --features=comptime`. It can be used to evaluate functions, generate structs, build strings, or produce any Rust expression. The output is saved to a file and included at compile time.

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

- Items (e.g. `use`, `fn`, `struct`) are shared across all blocks
- Named blocks `name { ... }` become `#[test]` functions
- Only runs when `cargo test --features=comptime`

---

### `comptime!("name")` — Include evaluated result
Includes the evaluated/generated file as an expression in non test builds. Returns `Default::default()` in test builds.

```rust
let x = comptime!("the_key_name");
```

---

### `comptime!(full, "name")` — Include generated item
Includes the generated file as a top level item (e.g. struct, impl). Use for declarations that cannot appear inside expressions.

```rust
comptime!(full, "my_struct");
```

Optionally accepts a default for test builds:
```rust
comptime!(full, "my_struct", struct MyStruct { x: i32 });
```

---

### `comptime_output!(tag, value, "name")` — Write output file
Writes an evaluated value to `./comptime/<name>`. Only runs during test.

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
Wraps code that should only run in non test builds.

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

## Scripts

### `comptime.sh` — Dev runner
Evaluates and generates files, then runs the project.

```bash
#!/bin/bash
cargo test --features=comptime -- --no-capture
clear
cargo run
```
