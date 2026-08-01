fn main() {
    assert_eq!(fcomptime_test::lib_value(), 50);
    assert_eq!(fcomptime_test::lib_const_value(), 50);
    assert_eq!(fcomptime_test::token_value(), 50);
    assert_eq!(fcomptime_test::lib_impl_value(), 50);
    assert_eq!(fcomptime_test::long_result(), 190);
    assert_eq!(fcomptime_test::trait_use_result(), 43);
    println!("lib values good");
}
