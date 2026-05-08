# Rustime

Rustime is a Rust **compile time code evaluation and generation** library. It runs Rust code at test time, evaluating expressions, generating values or syntaxs, and producing files. Then includes those results at compile time in debug/release builds.

---

## How It Works

```
cargo test --features=rustime  →  evaluates code, generates ./rustime/* files
cargo build                    →  includes those files via include!()
```

The key idea: anything that would be expensive or complex to compute at runtime can be **evaluated once during test**, saved to a file, and **included as a literal at compile time**. Easier than writing macro. No proc macro to speed up compile speed.

---

## Setup

`Cargo.toml`:
```toml
[dependencies]
rustime = { path = "..." }

[features]
rustime = []
```

`main.rs` / `lib.rs`:
```rust
use rustime::*;
init_rustime!();
```

---

## Macros

### `init_rustime!()`
Initializes the internal mutex registry. Must be called once at crate root.

```rust
init_rustime!();
```

---

### `rustime!(blocks)` — Define evaluators and generators
Defines code evaluation and generation blocks. Each named block becomes a test function that runs during `cargo test --features=rustime`. It can be used to evaluate functions, generate structs, build strings, or produce any Rust expression. The output is saved to a file and included at compile time.

```rust
rustime!(
    use std::fmt::Write;

    // generate a struct definition
    any_name {
        let mut out = String::from("struct Test {");
        write!(out, "x: i32,").unwrap();
        out.push_str("}");
        rustime_output!(raw, out, "the_key_name");
    }

    // evaluate a function and save the result
    any_name {
        let result = some_fn(42);
        rustime_output!(raw, result, "the_key_name2");
    }
);
```

- Items (e.g. `use`, `fn`, `struct`) are shared across all blocks
- Named blocks `name { ... }` become `#[test]` functions
- Only runs when `cargo test --features=rustime`

---

### `rustime!("name")` — Include evaluated result
Includes the evaluated/generated file as an expression in non test builds. Returns `Default::default()` in test builds.

```rust
let x = rustime!("the_key_name");
```

---

### `rustime!(full, "name")` — Include generated item
Includes the generated file as a top level item (e.g. struct, impl). Use for declarations that cannot appear inside expressions.

```rust
rustime!(full, "my_struct");
```

Optionally accepts a default for test builds:
```rust
rustime!(full, "my_struct", struct MyStruct { x: i32 });
```

---

### `rustime_output!(tag, value, "name")` — Write output file
Writes an evaluated value to `./rustime/<name>`. Only runs during test.

| Tag | Behavior |
|-----|----------|
| `raw` | Writes value as is | use raw for number and syntax
| `str` | Wraps value in quotes `"..."` | use str for str/string only

```rust
rustime_output!(raw, value, "my_value");  // writes: 42
rustime_output!(str, value, "my_value");  // writes: "hello"
```

- Panics if the same name is written twice (duplicate detection)

---

### `rustime_scope!(...)` — Release only block
Wraps code that should only run in non test builds.

```rust
rustime_scope!(
    let val = rustime!("my_value");
    println!("{}", val);
);
```

---

## Full Example

```rust
use rustime::*;
init_rustime!();

// includes generated struct at compile time
rustime!(full, "my_struct",);

fn compute(n: i32) -> i32 {
    n * n
}

fn main() {
    rustime_scope!(
        let result = rustime!("computed");  // includes evaluated result
        let s = MyStruct { x: result };
        println!("{}", s.x);
    );
}

// define what to generate and evaluate
rustime!(
    use std::fmt::Write;

    // generate a struct from code
    my_struct {
        let mut out = String::from("struct MyStruct {");
        out.push_str("x: i32,");
        out.push_str("}");
        rustime_output!(raw, out, "my_struct");
    }

    // evaluate a function at test time, save result
    computed {
        let result = compute(9);  // evaluates to 81
        rustime_output!(raw, result, "computed");
    }
);
```

**Evaluate and generate:**
```bash
cargo test --features=rustime
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

### `rustime.sh` — Dev runner
Evaluates and generates files, then runs the project.

```bash
#!/bin/bash
cargo test --features=rustime -- --no-capture
clear
cargo run
```

### `rustime-release.sh` — Release builder
Evaluates and generates files, then builds release binary.

```bash
#!/bin/bash
cargo test --features=rustime -- --no-capture
clear
cargo build --release
```

### Custom pipeline
You can define your own pipeline by chaining with `&&` to stop on failure:

```bash
#!/bin/bash
cargo test --features=rustime -- --no-capture && clear && cargo run
```

Or with error handling:

```bash
#!/bin/bash
set -e  # stop on any error
cargo test --features=rustime -- --no-capture
clear
cargo run
```

**Make executable:**
```bash
chmod +x rustime.sh rustime-release.sh
```

**Run:**
```bash
./rustime.sh
./rustime-release.sh
```