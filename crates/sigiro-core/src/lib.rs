//! Logic shared by the native `sigiro` binary and the Workers lake: OTLP to
//! Arrow decoding, DRAIN log classification, and the read-only SQL firewall.

pub mod drain;
pub mod ids;
pub mod otlp;
pub mod sql;
