use fcomptime::prelude::*;

init_comptime!();

const BASE: i32 = 10;

#[comptime]
pub fn gen() {
    let v = BASE * 5;
    let _ = &v;
    source! {
        output!(raw, v, "lib_data");
        output!(raw, "const LIB_DATA: i32 = 50;", "lib_data_full");
    }
}

call!(full, "lib_data_full", const LIB_DATA: i32 = 0;);

#[comptime]
pub fn check_lib() {
    source! {
        call!(raw in, "lib_data", let v {
            assert_eq!(v, 50);
        });
    }
}

pub fn lib_value() -> i32 {
    call!("lib_data", 0)
}

pub fn lib_const_value() -> i32 {
    LIB_DATA
}

pub fn token_value() -> i32 {
    call!(token, "lib_data", 0)
}

pub struct LibGen;

fn lib_impl_helper() -> i32 {
    25 * 2
}

#[comptime]
impl LibGen {
    pub fn gen_impl() {
        let v = lib_impl_helper();
        source! {
            output!(raw, v, "lib_impl_data");
        }
    }
}

pub fn lib_impl_value() -> i32 {
    call!("lib_impl_data", 0)
}

#[comptime]
pub fn long_gen() {
    let mut acc = 0;
    let mut i = 0;
    while i < 100 {
        acc += i;
        i += 1;
    }
    let a = acc * 2;
    let b = a + 1;
    let c = b - 3;
    let d = c % 7;
    let e = d * 11;
    let f = e + 5;
    let g = f / 2;
    let h = g * 3;
    let j = h - 8;
    let k = j + 4;
    let m = k * 6;
    let n = m - 2;
    let o = n + 9;
    let p = o * 13;
    let q = p % 17;
    let r = q * 5;
    let s = r - 11;
    let t = s + 2;
    let u = t * 7;
    let v = u / 3;
    let w = v + 14;
    let y = w * 2;
    let z = y - 6;
    let _ = &z;
    source! {
        output!(raw, z, "long_data");
    }
}

pub fn long_result() -> i32 {
    call!("long_data", 0)
}

pub trait Calc {
    fn calc(x: i32) -> i32;
}

#[comptime]
impl Calc for i32 {
    fn calc(x: i32) -> i32 {
        let v = x * 2 + 1;
        source! {
            output!(raw, v, "trait_calc");
        }
        v
    }
}

#[comptime]
pub fn trait_use() {
    source! {
        let got = func!("calc", 21);
        output!(raw, got, "trait_use_data");
    }
}

pub fn trait_use_result() -> i32 {
    call!("trait_use_data", 0)
}
