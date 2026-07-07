//! Internal utility namespace shared across core modules.
//!
//! Contains helper modules used by multiple internal components.

pub mod hashing;

pub(crate) mod escaping;
pub(crate) use escaping::escape_control;
