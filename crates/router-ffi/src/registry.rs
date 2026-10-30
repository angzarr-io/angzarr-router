//! ComponentSpec → the core's one component: pure table population. Every
//! closure built here only marshals a role's Call across the callback and
//! its Return back; the semantics stay in angzarr-router.

use angzarr_router::error::CodedError;
use angzarr_router::router::Router;

use crate::abi::AngzarrCb;

/// The one component behind an opaque router handle (owner ruling: one
/// coordinator, one component, one business process), held as the core's
/// [`Router`].
pub struct FfiRouter {
    router: Option<Router>,
}

impl FfiRouter {
    pub fn new() -> Self {
        FfiRouter { router: None }
    }

    /// Decodes a ComponentSpec and hosts its component, each role a
    /// callback-marshaling closure over `cb` (abi.proto, ComponentSpec).
    pub fn register(&mut self, spec: &[u8], cb: AngzarrCb) -> Result<(), CodedError> {
        let _ = (spec, cb, &self.router);
        todo!("FfiRouter::register")
    }

    /// Decodes a DispatchRequest, runs it through the hosted component, and
    /// answers the serialized DispatchResponse.
    pub fn dispatch(&self, request: &[u8]) -> Result<Vec<u8>, CodedError> {
        let _ = (request, &self.router);
        todo!("FfiRouter::dispatch")
    }
}
