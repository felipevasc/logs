//! Bounded CPU-only online learning. Scores describe surprise, never causality.
//! Local message prototypes are deliberately absent from the sharing protocol.
mod change;
mod model;
mod seen;
mod shared;
pub use change::*;
pub use model::*;
pub use seen::*;
pub use shared::*;

use sha2::{Digest, Sha256};
pub const SCHEMA: u32 = 1;
pub const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
