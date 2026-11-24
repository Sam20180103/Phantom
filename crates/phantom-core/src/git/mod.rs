//! Git plumbing, shelling out to the `git` binary.
//!
//! Tauri-free by construction so both the desktop app and `phantom-server` can
//! drive it; the command wrappers that bind it to an IPC surface live in each
//! of those crates instead.

pub mod errors;
pub mod operations;
pub mod parser;
mod process;
pub mod types;
pub mod utils;
mod untracked;
