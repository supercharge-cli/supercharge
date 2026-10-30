//! Built-in REA investigation tools (native Rust; no Node).
//!
//! Active when `[rea] enabled` is on. Contracts and Evidence/limitations style
//! are adapted from morluto/rea (MIT License); see the bundled skill LICENSE.

mod apk;
mod asar;
mod binary;
mod bounds;
mod disasm;
mod evidence;
mod external;
mod hashes;
mod identify;
mod js;
mod read_bytes;
mod strings;
mod tools;

pub use tools::*;
