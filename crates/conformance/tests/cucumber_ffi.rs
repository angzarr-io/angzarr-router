//! The counter suite (tests/cucumber.rs) through the router's C ABI: the
//! same feature and steps, the fixture registered and dispatched as a
//! binding does (tests/ffi; `ffi::through_ffi` is true in this target).

#[path = "cucumber.rs"]
mod suite;

fn main() {
    suite::main()
}
