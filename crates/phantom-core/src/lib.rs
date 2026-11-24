//! Headless core extracted from Terax's `src-tauri` for Phantom.
//!
//! The rule this crate exists to enforce: **no UI-framework dependency**.
//! Nothing here may depend on `tauri`, a webview, or a windowing toolkit, so
//! the same logic serves both the desktop app and the headless server. Output
//! and lifecycle events leave through [`pty::PtySink`] rather than being
//! emitted directly to a frontend.

pub mod agent;
pub mod ai_sessions;
pub mod fs;
pub mod git;
pub mod history;
pub mod net;
pub mod path;
pub mod proc;
pub mod pty;
pub mod secrets;
pub mod shell;
pub mod workspace;
