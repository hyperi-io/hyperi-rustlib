// Project:   hyperi-rustlib
// File:      src/config/sensitive.rs
// Purpose:   Re-export SensitiveString for backward compatibility
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Re-exports [`SensitiveString`] from the crate root for backward compatibility.
//!
//! The canonical location is now [`crate::sensitive`], which is always available
//! regardless of feature gates. This module preserves the old import path
//! `hyperi_rustlib::config::sensitive::SensitiveString`.

pub use crate::sensitive::*;
