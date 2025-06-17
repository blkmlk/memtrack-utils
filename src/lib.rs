//!
//! # memtrack-utils
//!
//! ## memtrack-utils
//!
//! A library with utils used for parsing heap tracing files
//!
//! > **Platform support**: Currently tested only on macOS (aarch64-apple-darwin)
//!
//! License: MIT

pub(crate) mod executor;
pub mod interpret;
mod output;
pub mod parser;
pub mod pipe_io;
pub mod common;
mod resolver;
