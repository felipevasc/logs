//! Focused Windows filesystem regressions using the exact production modules.
//! The complete library test suite remains a separate, unchanged target.
#![cfg(windows)]

#[path = "../src/atomic_metadata.rs"]
mod atomic_metadata;
