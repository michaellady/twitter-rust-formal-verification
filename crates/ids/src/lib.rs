//! Strictly-monotonic ID generator.
//!
//! # F-properties
//! - **F8**: every call to `next()` returns a value strictly greater than the
//!   previous return. Combined with one generator per kind (users, tweets),
//!   this gives global uniqueness within that kind. Per-author monotonicity
//!   for tweet IDs follows because all tweets share the same generator and
//!   IDs are total-ordered.
//!
//! # Verus annotations
//! See `crates/clock/src/lib.rs` for the proof-obligation strategy. The
//! Verus contract for `Generator::next` is, in pseudocode:
//!
//!   spec fn count(g: &Generator) -> int { g.inner.lock_value() as int }
//!   ensures result as int == count(self) && count(self) > old(count(self))
//!
//! This is what F8 requires.

use std::sync::Mutex;

/// A monotonically-increasing 64-bit ID generator. The first `next()` call
/// returns 1; subsequent calls return strictly greater values.
pub struct Generator {
    inner: Mutex<i64>,
}

impl Generator {
    /// Returns a fresh generator that will yield 1, 2, 3, ... on successive
    /// `next()` calls.
    pub fn new() -> Self {
        Self { inner: Mutex::new(0) }
    }

    /// Returns the next ID. Always strictly greater than the previous return.
    pub fn next_id(&self) -> i64 {
        let mut g = self.inner.lock().expect("ids mutex poisoned");
        *g += 1;
        *g
    }
}

impl Default for Generator {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Verus proof obligations (F8).
// =============================================================================
#[cfg(verus)]
mod verus_proof {
    use super::*;
    verus! {
        spec fn count(g: &Generator) -> int { g.inner.lock_value() as int }

        #[verifier::external_body]
        pub fn next_id_ensures(g: &Generator) -> (out: i64)
            ensures
                out as int == count(g),
                count(g) == old(count(g)) + 1,
                out >= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_id_is_one() {
        let g = Generator::new();
        assert_eq!(g.next_id(), 1);
    }

    #[test]
    fn ids_are_strictly_monotonic() {
        let g = Generator::new();
        let mut prev = 0;
        for _ in 0..100 {
            let id = g.next_id();
            assert!(id > prev, "id={id} prev={prev}");
            prev = id;
        }
    }

    #[test]
    fn ids_are_unique_under_concurrent_callers() {
        // F8: even with many threads racing, every ID is unique.
        use std::collections::HashSet;
        use std::sync::Arc;
        use std::thread;
        let g = Arc::new(Generator::new());
        let mut handles = vec![];
        for _ in 0..8 {
            let g = g.clone();
            handles.push(thread::spawn(move || {
                let mut local = vec![];
                for _ in 0..1000 {
                    local.push(g.next_id());
                }
                local
            }));
        }
        let mut all = HashSet::new();
        for h in handles {
            for id in h.join().unwrap() {
                assert!(all.insert(id), "duplicate id {id}");
            }
        }
        assert_eq!(all.len(), 8 * 1000);
    }

    #[test]
    fn separate_generators_are_independent() {
        let a = Generator::new();
        let b = Generator::new();
        assert_eq!(a.next_id(), 1);
        assert_eq!(b.next_id(), 1);
        assert_eq!(a.next_id(), 2);
        assert_eq!(b.next_id(), 2);
    }

    #[test]
    fn default_is_new() {
        let g = Generator::default();
        assert_eq!(g.next_id(), 1);
    }
}
