//! IEM Cast host core.
//!
//! One crate with internal modules by responsibility. Task 1 freezes the shared contract in
//! [`ids`] and [`contract`] and creates a stub `mod.rs` for every future lane folder so lanes
//! never edit this file during parallel execution.

pub mod audio;
pub mod capture;
pub mod config;
pub mod contract;
pub mod control;
pub mod diagnostics;
pub mod encoder;
pub mod fixtures;
pub mod ids;
pub mod pipeline;
pub mod runtime;
pub mod server;
pub mod transport;
