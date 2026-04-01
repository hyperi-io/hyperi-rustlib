// Project:   hyperi-rustlib
// File:      src/worker/engine/intern.rs
// Purpose:   Concurrent field name interning for the batch processing engine
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Field name interning for the batch processing engine.
//!
//! Deduplicates field name strings across an entire batch. The first occurrence
//! of a field name allocates an `Arc<str>`; all subsequent occurrences get a
//! cheap `Arc::clone` (~2 ns). Thread-safe via `DashMap` — safe for concurrent
//! access from rayon worker threads.

use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use sonic_rs::JsonContainerTrait as _;

/// Concurrent field name interner.
///
/// Pre-populate with `with_known_fields` to amortise the cost of the first
/// `intern` call for the hot-path fields (e.g. `_table`, `_timestamp`).
///
/// # Thread safety
///
/// `DashMap` shards the hash map into multiple segments; concurrent
/// reads and writes from rayon worker threads are safe without external locking.
pub struct FieldInterner {
    table: DashMap<Arc<str>, ()>,
}

impl FieldInterner {
    /// Create a new, empty interner.
    pub fn new() -> Self {
        Self {
            table: DashMap::new(),
        }
    }

    /// Create an interner pre-populated with the given field names.
    ///
    /// Callers should pass the `known_fields` from [`super::config::BatchProcessingConfig`]
    /// so that common fields never hit the slow-path allocation during a batch.
    pub fn with_known_fields(fields: &[&str]) -> Self {
        let interner = Self::new();
        for f in fields {
            interner.intern(f);
        }
        interner
    }

    /// Intern a field name and return a shared `Arc<str>`.
    ///
    /// # Cost model
    ///
    /// - Fast path (already interned): one DashMap read-lock shard + `Arc::clone` → ~20 ns
    /// - Slow path (first occurrence): one write to DashMap + `Arc::from` allocation → ~100 ns
    ///
    /// The slow path is taken at most once per unique field name per `FieldInterner` instance.
    #[inline]
    pub fn intern(&self, name: &str) -> Arc<str> {
        // Fast path: field already interned — borrow the existing Arc.
        // Arc<str>: Borrow<str> is in std, so DashMap::get accepts &str directly.
        if let Some(entry) = self.table.get(name) {
            return Arc::clone(entry.key());
        }

        // Slow path: first occurrence — allocate and insert.
        let key: Arc<str> = Arc::from(name);
        self.table.entry(Arc::clone(&key)).or_insert(());

        // Re-read to handle the (rare) concurrent-insert race: two threads may
        // both miss the fast path and both try to insert. The one that wins
        // the DashMap shard lock stores its key; the loser's key is dropped.
        // We always return the canonical key that is present in the map.
        if let Some(entry) = self.table.get(name) {
            Arc::clone(entry.key())
        } else {
            // Extremely unlikely: the entry we just inserted is somehow gone
            // (shouldn't happen without external removal). Return our key.
            key
        }
    }

    /// Extract known (pre-interned) fields from a parsed `sonic_rs::Value`.
    ///
    /// Iterates the top-level object keys and returns only those that are
    /// already interned. O(known_fields × object_keys) — typically
    /// 6 known × 15 keys = 90 string comparisons per message.
    ///
    /// Returns an empty map if `value` is not a JSON object.
    pub fn extract_known(&self, value: &sonic_rs::Value) -> HashMap<Arc<str>, sonic_rs::Value> {
        let mut extracted = HashMap::new();
        if let Some(obj) = value.as_object() {
            for (key, val) in obj.iter() {
                if let Some(entry) = self.table.get(key) {
                    let v: sonic_rs::Value = val.clone();
                    extracted.insert(Arc::clone(entry.key()), v);
                }
            }
        }
        extracted
    }

    /// Return the number of interned field names.
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// Return `true` if no field names have been interned yet.
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

impl Default for FieldInterner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use sonic_rs::JsonValueTrait as _;

    use super::*;

    #[test]
    fn intern_returns_same_arc_for_same_string() {
        let interner = FieldInterner::new();
        let a = interner.intern("_table");
        let b = interner.intern("_table");
        assert!(Arc::ptr_eq(&a, &b), "expected same Arc for '_table'");
    }

    #[test]
    fn intern_returns_different_arcs_for_different_strings() {
        let interner = FieldInterner::new();
        let a = interner.intern("_table");
        let b = interner.intern("_timestamp");
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(a.as_ref(), "_table");
        assert_eq!(b.as_ref(), "_timestamp");
    }

    #[test]
    fn with_known_fields_prepopulates_table() {
        let fields = ["_table", "_timestamp", "host"];
        let interner = FieldInterner::with_known_fields(&fields);
        assert_eq!(interner.len(), 3);

        // Subsequent intern calls return the same Arc (pointer equality).
        let a = interner.intern("_table");
        let b = interner.intern("_table");
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn extract_known_extracts_matching_fields() {
        let interner = FieldInterner::with_known_fields(&["_table", "host"]);
        let value: sonic_rs::Value =
            sonic_rs::from_str(r#"{"_table": "events", "host": "web1", "unknown": 42}"#).unwrap();

        let extracted = interner.extract_known(&value);

        assert_eq!(extracted.len(), 2);

        // Verify extracted values.
        let table_key: Arc<str> = interner.intern("_table");
        let host_key: Arc<str> = interner.intern("host");
        assert_eq!(
            extracted.get(&table_key).and_then(|v| v.as_str()),
            Some("events")
        );
        assert_eq!(
            extracted.get(&host_key).and_then(|v| v.as_str()),
            Some("web1")
        );
        // Unknown field was not extracted.
        let unknown_key: Arc<str> = Arc::from("unknown");
        assert!(extracted.get(&unknown_key).is_none());
    }

    #[test]
    fn extract_known_ignores_unknown_fields() {
        let interner = FieldInterner::with_known_fields(&["_table"]);
        let value: sonic_rs::Value = sonic_rs::from_str(r#"{"foo": 1, "bar": 2}"#).unwrap();

        let extracted = interner.extract_known(&value);
        assert!(extracted.is_empty(), "no known fields should be extracted");
    }

    #[test]
    fn extract_known_on_non_object_returns_empty() {
        let interner = FieldInterner::with_known_fields(&["_table"]);
        let value: sonic_rs::Value = sonic_rs::from_str(r#"[1, 2, 3]"#).unwrap();
        let extracted = interner.extract_known(&value);
        assert!(extracted.is_empty());
    }

    #[test]
    fn concurrent_interning_deduplicates_correctly() {
        use std::sync::Arc as StdArc;

        let interner = StdArc::new(FieldInterner::new());
        let field = "_table";
        let num_threads = 8;

        let handles: Vec<_> = (0..num_threads)
            .map(|_| {
                let interner = StdArc::clone(&interner);
                thread::spawn(move || interner.intern(field))
            })
            .collect();

        let arcs: Vec<Arc<str>> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All threads must have received the same Arc (pointer equality).
        let first = &arcs[0];
        for arc in &arcs[1..] {
            assert!(
                Arc::ptr_eq(first, arc),
                "concurrent interning produced different Arc instances"
            );
        }

        // Only one entry should be in the table.
        assert_eq!(interner.len(), 1);
    }

    #[test]
    fn len_and_is_empty() {
        let interner = FieldInterner::new();
        assert!(interner.is_empty());
        interner.intern("a");
        assert_eq!(interner.len(), 1);
        assert!(!interner.is_empty());
        interner.intern("b");
        assert_eq!(interner.len(), 2);
        // Repeated intern of existing key does not grow the table.
        interner.intern("a");
        assert_eq!(interner.len(), 2);
    }
}
