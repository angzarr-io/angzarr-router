//! HostCtxGuard: the thread-local host_ctx installed for one dispatch is
//! restored when the dispatch (or a nested one) ends.

use std::ffi::c_void;

use super::{HostCtxGuard, CURRENT_HOST_CTX};

fn current() -> *mut c_void {
    CURRENT_HOST_CTX.with(|c| c.get())
}

#[test]
fn guard_installs_and_restores_nested_contexts() {
    let outer = 0x10 as *mut c_void;
    let inner = 0x20 as *mut c_void;
    assert!(current().is_null());
    {
        let _outer = HostCtxGuard::set(outer);
        assert_eq!(current(), outer);
        {
            let _inner = HostCtxGuard::set(inner);
            assert_eq!(current(), inner);
        }
        assert_eq!(
            current(),
            outer,
            "a nested dispatch restores its caller's ctx"
        );
    }
    assert!(current().is_null(), "no ctx outlives its dispatch");
}
