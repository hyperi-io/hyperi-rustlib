// Project:   hyperi-rustlib
// File:      src/parse_guard.rs
// Purpose:   Stack-safe nesting-depth guard for the JSON/MsgPack parse paths
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Parse-path depth guard against stack-exhaustion DoS.
//!
//! JSON (`sonic_rs`) and MsgPack (`rmpv`) recurse per nesting level; a
//! deeply-nested payload can blow the worker stack. We reject above
//! [`MAX_PARSE_DEPTH`] -- JSON via the iterative [`json_depth_within`] (run
//! before the recursive parser), MsgPack via `read_value_with_max_depth`.

/// Max accepted JSON/MsgPack nesting depth. DFE payloads are shallow; 64 is
/// well above real data and under a stack hazard.
pub(crate) const MAX_PARSE_DEPTH: usize = 64;

/// `true` if the JSON payload nests no deeper than `max`.
///
/// Single forward pass counting `{`/`[`, skipping string contents (with `\`
/// escapes). Not a validator -- a pre-filter ahead of `sonic_rs`. Iterative, so
/// stack-safe at any depth; returns on the first breach, so `O(max)` not
/// `O(len)`.
pub(crate) fn json_depth_within(payload: &[u8], max: usize) -> bool {
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escaped = false;
    for &b in payload {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                if depth > max {
                    return false;
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_and_shallow_pass() {
        assert!(json_depth_within(br"{}", 64));
        assert!(json_depth_within(br#"{"a":1,"b":[1,2,3]}"#, 64));
        assert!(json_depth_within(br#"{"a":{"b":{"c":1}}}"#, 64));
    }

    #[test]
    fn exactly_at_bound_passes_one_over_fails() {
        // depth 3 with max 3 is OK; depth 4 with max 3 is rejected.
        assert!(json_depth_within(br"[[[1]]]", 3));
        assert!(!json_depth_within(br"[[[[1]]]]", 3));
    }

    #[test]
    fn braces_inside_strings_do_not_count() {
        // The string value contains many braces but real depth is 1.
        assert!(json_depth_within(br#"{"k":"{{{{{{{{[[[[["}"#, 2));
    }

    #[test]
    fn escaped_quote_keeps_string_open() {
        // The escaped quote does not close the string, so the trailing braces
        // stay inside it and do not add depth.
        assert!(json_depth_within(br#"{"k":"a\"{{{{{"}"#, 2));
    }

    #[test]
    fn pathological_depth_is_rejected_cheaply() {
        // 5000 nested arrays -> rejected well before the end (O(max), no panic,
        // no recursion).
        let mut deep = vec![b'['; 5000];
        deep.extend_from_slice(b"1");
        deep.extend(std::iter::repeat_n(b']', 5000));
        assert!(!json_depth_within(&deep, MAX_PARSE_DEPTH));
    }
}
