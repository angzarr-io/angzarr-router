//! `Send` where there are threads: the kernel-facing dispatch traits' one
//! bound that differs by target (owner ruling 2026-10-30, after the
//! 2026-10-27 ruling for the store ports; the same pattern as
//! angzarr-ports' `send`).
//!
//! On every target but `wasm32`, [`MaybeSend`] is `Send` and [`MaybeSync`]
//! is `Sync`: a component's dispatch futures are `Send`, so a server spawns
//! them on a multi-threaded runtime. On `wasm32` they bound nothing, so a
//! browser coordinator's components (whose handles are not `Send`)
//! implement the same traits. There is one trait set; only these two
//! bounds change.

use std::future::Future;
use std::pin::Pin;

/// `Send`, except on `wasm32`.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + ?Sized> MaybeSend for T {}

/// `Send`, except on `wasm32`.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}

/// `Sync`, except on `wasm32`.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSync: Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Sync + ?Sized> MaybeSync for T {}

/// `Sync`, except on `wasm32`.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSync {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSync for T {}

/// A boxed future: `Send` on every target but `wasm32`. What the
/// object-safe twins of the dispatch traits return.
#[cfg(not(target_arch = "wasm32"))]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// A boxed future: `Send` on every target but `wasm32`. What the
/// object-safe twins of the dispatch traits return.
#[cfg(target_arch = "wasm32")]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;
