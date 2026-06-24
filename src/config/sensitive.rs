// Project:   scalo
// File:      src/config/sensitive.rs
// Purpose:   Re-export SensitiveString for backward compatibility
// Language:  Rust
//
// License:   Apache-2.0
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Re-exports [`SensitiveString`] from the crate root for backward compatibility.
//!
//! The canonical location is now [`crate::sensitive`], which is always available
//! regardless of feature gates. This module preserves the old import path
//! `scalo::config::sensitive::SensitiveString`.

pub use crate::sensitive::*;
