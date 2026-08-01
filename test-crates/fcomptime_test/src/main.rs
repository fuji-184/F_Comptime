#![allow(unexpected_cfgs)]
use fcomptime::prelude::*;

#[comptime]
pub fn gen() {
    let data = 5 * 5;
    let _ = &data;
    source! {
        output!(raw, data, "data");
        output!(str, "hello \"world\"", "greeting");
        output!(str, format!("multi {}", "part"), "multi_str");
    }
}

#[comptime]
pub fn use_str() {
    source! {
        call!(str in, "greeting", g {
            assert_eq!(g, "hello \"world\"");
        });
        call!(str in, "multi_str", m {
            assert_eq!(m, "multi part");
        });
        call!(str in, "missing_file", not_found {
            assert!(false, "body of missing str in should not run");
        });
    }
}

#[comptime]
pub fn use_raw_const_multi_type() {
    source! {
        call!(raw in, "data", const val: i32 {
            assert_eq!(val, 25);
        });
    }
}

#[comptime]
pub fn use_raw_let() {
    source! {
        call!(raw in, "data", let val {
            assert_eq!(val, 25);
        });
    }
}

pub fn not_annotated() {}

#[info]
pub fn callee<T>(_x: T) {}

#[info]
pub fn caller() {
    callee::<i32>(5);
}

#[info]
pub fn caller2() {
    not_annotated();
    callee::<i32>(5);
    callee::<&str>("uwu");
}

#[comptime]
pub fn check_info() {
    source! {
        let info_a = get!("callee");
        if let Some(i) = info_a {
            assert_eq!(i.name, "callee");
            assert_eq!(i.parameters.len(), 1);
            assert_eq!(i.callers.len(), 3);
            assert_eq!(i.generics.len(), 1);
        } else {
            panic!("get! on callee returned None");
        }

        let info_b = get!("not_annotated");
        if let Some(i) = info_b {
            assert_eq!(i.name, "not_annotated");
            assert!(i.line.is_none());
            assert!(i.return_type.is_none());
        } else {
            panic!("get! on not_annotated returned None");
        }

        let info_c = get!("totally_missing");
        assert!(info_c.is_none());
    }
}

#[comptime]
pub fn check_partial() {
    source! {
        output!(raw, "\"a\",\"b\",\"c\",\"d\",\"e\",\"f\",\"g\",\"h\",\"i\",\"j\",\"k\"", "parts12");
    }
}

pub fn partial_result() {
    call!(partial, "parts12", (#1, #10, #11));
}

pub fn bare_call_result() -> i32 {
    call!("data")
}

pub fn token_result() -> i32 {
    call!(token, "data")
}

#[comptime]
pub fn gen_full() {
    source! {
        output!(raw, "const DATA_CONST: i32 = 25;", "data_full");
    }
}

call!(full, "data_full", const DATA_CONST: i32 = 0;);

pub fn full_result() -> i32 {
    DATA_CONST
}

#[allow(unused_variables)]
#[comptime]
pub fn math(i: i32) {
    source! {
        let res = i * 2;
        output!(raw, res, "hasil");
    }
}

#[allow(unused_variables)]
#[comptime]
pub fn area(w: i32, h: i32) {
    source! {
        let res = w * h;
        output!(raw, res, "luas");
    }
}

#[allow(unused_variables, unused_assignments)]
#[comptime]
pub fn scale(i: i32) {
    source! {
        let mut out = 0;
        if i > 5 {
            out = i * 2;
            output!(raw, out, "big");
        } else {
            out = i * 3;
            output!(raw, out, "small");
        }
    }
}

#[comptime]
pub fn use_local() {
    source! {
        let base = 100;
        let tes = func!("math", base);
        output!(raw, tes, "local_data");
    }
}

pub fn local_result() -> i32 {
    call!("local_data", 0)
}

#[allow(unused_variables, unused_assignments)]
#[comptime]
pub fn gen_loop() {
    let mut x = 0;
    for i in 0..5 {
        x += i;
    }
    source! {
        output!(raw, x, "loop_data");
    }
}

pub fn loop_result() -> i32 {
    call!("loop_data", 0)
}

#[comptime]
pub fn async_gen() {
    async_source! {
        let val = async { 21 * 2 }.await;
        output!(raw, val, "async_data");
    }
}

#[comptime]
pub fn use_async_output() {
    source! {
        call!(str in, "async_data", v {
            let parsed = v.parse::<i32>().unwrap();
            assert_eq!(parsed, 42);
        });
    }
}

pub struct ImplGen;

fn impl_helper() -> i32 {
    30 + 12
}

#[comptime]
impl ImplGen {
    pub fn gen_value() {
        let local = impl_helper();
        source! {
            output!(raw, local, "impl_data");
        }
    }

    pub fn gen_str(&self) {
        source! {
            output!(str, format!("from impl {}", 7), "impl_str");
        }
    }
}

pub fn impl_data_result() -> i32 {
    call!("impl_data", 0)
}

pub fn impl_str_result() -> &'static str {
    call!("impl_str", "")
}

#[comptime]
fn main() {
    assert_eq!(bare_call_result(), 25);
    assert_eq!(token_result(), 25);
    assert_eq!(full_result(), 25);
    assert_eq!(loop_result(), 10);
    assert_eq!(local_result(), 200);
    assert_eq!(impl_data_result(), 42);
    assert_eq!(impl_str_result(), "from impl 7");
    let _ = partial_result();

    let tes = func!("math", 10);
    assert_eq!(tes, 20);
    println!("{}", tes);

    let v = 7;
    let tes2 = func!("math", v);
    assert_eq!(tes2, 14);

    assert_eq!(func!("area", 3, 4), 12);
    assert_eq!(func!("scale", 10), 20);
    assert_eq!(func!("scale", 2), 6);
    assert_eq!(func!("calc", 21), 43);

    call_scope! {
        let tes3 = func!("math", 100);
        assert_eq!(tes3, 200);
        println!("{}", tes3);
    }

    println!("all good");
}
