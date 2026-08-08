//! PWCLI's four-layer application architecture.
//!
//! Dependency direction is `app -> runtime -> agent_core -> ai`.

pub mod agent_core;
pub mod ai;
pub mod app;
pub mod runtime;
