fn main() {
    assert_eq!(fcomptime_test::lib_value(), 50);
    assert_eq!(fcomptime_test::lib_const_value(), 50);
    assert_eq!(fcomptime_test::token_value(), 50);
    assert_eq!(fcomptime_test::lib_impl_value(), 50);
    println!("lib values good");
}
