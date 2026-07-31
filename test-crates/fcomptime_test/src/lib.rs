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
