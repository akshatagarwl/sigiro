//! Logic shared by the native `sigiro` binary and wasm builds: DRAIN log
//! classification and the read-only SQL firewall.

pub mod drain;
pub mod sql;
