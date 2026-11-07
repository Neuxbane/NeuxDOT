//! NeuxDOT HTTP server library.
//!
//! The binary (`main.rs`) is a thin wrapper around this crate so the router can
//! be mounted in tests without spawning a real listener.

pub mod middleware;
pub mod routes;
pub mod state;
