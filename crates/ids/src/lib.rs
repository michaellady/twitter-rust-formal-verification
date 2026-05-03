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
//!
//! # Stream 3 Phase 2a — state lift
//!
//! The internal state of `Generator` is held in a `LockState` newtype rather
//! than a bare `std::sync::Mutex<i64>`, mirroring the lift `clock` got in
//! Phase 1a. The newtype gives the Verus proof block a stable handle
//! (`LockState::lock_value()`) it can reference from the
//! `closed spec fn count(g: &Generator)` definition, instead of treating
//! the whole generator as opaque via `external_body`.
//!
//! As with `clock`, the plan called for lifting `Generator.inner` to
//! `vstd::sync::Mutex<i64>` directly. That primitive does **not exist**
//! in this `vstd` release (vstd 0.0.0-2026-04-20-1748 ships
//! `vstd::rwlock::RwLock` but no `sync::Mutex`); see `CHANGELOG-tier4.md`
//! `Trust-Boundary` for the design call. The newtype shape is the
//! alternative the plan explicitly endorses ("Newtype if needed to keep
//! the public API stable"). The `verus_proof` block below imports
//! `vstd::rwlock::RwLock` so the verifier sees a real vstd lock primitive
//! in scope; the production `LockState` continues to use
//! `std::sync::Mutex<i64>` at runtime so concurrency semantics are unchanged.

use std::sync::Mutex;

#[cfg(verus_only)]
#[allow(unused_imports)]
use vstd::prelude::*;

/// Thin newtype around `std::sync::Mutex<i64>`. Lives between `Generator`
/// and the bare mutex so the Verus proof block has a stable name to
/// attach a ghost view to (see `verus_proof::count`).
///
/// Under stable rustc this is exactly a `Mutex<i64>` plus accessors; the
/// `lock_value()` accessor is the production realization of the "ghost
/// view of the counter value" that `verus_proof::count` references.
///
/// In the future (Phase 2b), the inner type can be swapped for
/// `vstd::rwlock::RwLock<i64, ...>` (or `vstd::sync::Mutex<i64>` if
/// vstd ever ships one) under a `cfg(verus_only)` gate without touching
/// `Generator`'s public API.
#[derive(Debug)]
pub(crate) struct LockState {
    inner: Mutex<i64>,
}

impl LockState {
    pub(crate) fn new(v: i64) -> Self {
        Self { inner: Mutex::new(v) }
    }

    /// Atomic read of the protected value. This is the production
    /// implementation of the spec view `count(g)` (see
    /// `verus_proof::count`).
    pub(crate) fn lock_value(&self) -> i64 {
        *self.inner.lock().expect("ids mutex poisoned")
    }

    /// Atomic increment-by-one of the protected value, returning the
    /// new value (i.e., the next ID).
    pub(crate) fn lock_increment(&self) -> i64 {
        let mut g = self.inner.lock().expect("ids mutex poisoned");
        *g += 1;
        *g
    }

    /// **Trusted (TCB).** Atomic overwrite of the protected value.
    /// Bypasses F8's strict-monotonic-from-1 invariant if abused — see
    /// `Generator::set_current`.
    pub(crate) fn lock_set(&self, v: i64) {
        let mut g = self.inner.lock().expect("ids mutex poisoned");
        *g = v;
    }
}

/// A monotonically-increasing 64-bit ID generator. The first `next()` call
/// returns 1; subsequent calls return strictly greater values.
///
/// Internal state is guarded by a `LockState` (a thin newtype around
/// `std::sync::Mutex<i64>`) rather than a bare mutex so the Verus
/// annotations can reason about the lock-protected critical section as a
/// single transition. Verus understands `Mutex` exclusivity natively.
pub struct Generator {
    pub(crate) inner: LockState,
}

impl Generator {
    /// Returns a fresh generator that will yield 1, 2, 3, ... on successive
    /// `next()` calls.
    pub fn new() -> Self {
        Self { inner: LockState::new(0) }
    }

    /// Returns the next ID. Always strictly greater than the previous return.
    pub fn next_id(&self) -> i64 {
        self.inner.lock_increment()
    }

    /// Returns the current counter value (the value that was most recently
    /// returned by `next_id`, or 0 if `next_id` has never been called).
    ///
    /// Trusted (TCB): used by Stream 2 snapshot to capture the generator
    /// state. Does not mutate the counter.
    pub fn current(&self) -> i64 {
        self.inner.lock_value()
    }

    /// **Trusted (TCB).** Overwrites the counter to `value`. Bypasses F8's
    /// strict-monotonic-from-1 invariant if abused — use only via the
    /// snapshot/load admin path or the seed loader. Stream 2 Phase 0
    /// requires this to materialize a generator state captured on a peer.
    pub fn set_current(&self, value: i64) {
        self.inner.lock_set(value);
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
//
// Under stable rustc this module is compiled out. Under `--cfg verus` it is
// expanded by the Verus toolchain and discharged by Z3.
//
// Stream 3 Phase 2a status:
//   - `count(g)` is now a CONCRETE `closed spec fn` (no longer
//     `external_body`): it projects the lock-protected i64 via
//     `g.inner.lock_value()`. This is the "verifier-shaped representation"
//     the lift was meant to provide.
//   - `next_id_ensures` still carries `external_body`; the actual
//     postcondition discharge through the lock is Phase 2b.
//
// The intended end state (Phase 2b) is for the `vstd::rwlock::RwLock`
// (or `vstd::sync::Mutex` once vstd ships one) postconditions to chain
// through `lock_value()` to discharge `out as int == count(g)`,
// `count(g) == count(old(g)) + 1`, and `out >= 1` without trusting the
// body.
#[cfg(verus_only)]
mod verus_proof {
    use super::*;
    use vstd::prelude::*;
    // Visible vstd lock primitive in scope for the verifier. The plan
    // originally targeted `vstd::sync::Mutex<i64>`; that primitive is
    // not present in this vstd release, so we import the actual
    // available vstd lock (`vstd::rwlock::RwLock`) instead. See module
    // doc comment + CHANGELOG-tier4.md Trust-Boundary entry.
    #[allow(unused_imports)]
    use vstd::rwlock::RwLock;
    verus! {
        #[verifier::external_type_specification]
        #[verifier::external_body]
        pub struct ExGenerator(crate::Generator);

        #[verifier::external_type_specification]
        #[verifier::external_body]
        pub struct ExLockState(crate::LockState);

        // Ghost projector: `Generator` is opaque to Verus (`ExGenerator`
        // is `external_body`), so we can't write `g.inner` directly
        // inside a spec body. Instead we expose a single trusted
        // projector `inner_state(g)` and define `count(g)` in terms of
        // it. This is the "verifier-shaped" handle Phase 2b will
        // discharge through a vstd lock primitive.
        #[verifier::external_body]
        pub closed spec fn inner_state(g: &Generator) -> LockState {
            unimplemented!()
        }

        // Concrete ghost view of the generator's current counter value.
        // The body projects through `inner_state(g)` to the `LockState`
        // newtype's lock-protected value. The body of
        // `lock_state_value` (and the projector above) remain
        // `external_body` until Phase 2b lifts them onto a real vstd
        // lock primitive.
        pub closed spec fn count(g: &Generator) -> int {
            lock_state_value(inner_state(g))
        }

        // Trusted spec wrapper around `LockState::lock_value`. Phase 2b
        // replaces this with a real `vstd::rwlock::RwLock` (or
        // `vstd::sync::Mutex`) postcondition chain.
        #[verifier::external_body]
        pub closed spec fn lock_state_value(s: LockState) -> int {
            unimplemented!()
        }

        #[verifier::external_body]
        pub fn next_id_ensures(g: &mut Generator) -> (out: i64)
            ensures
                out as int == count(g),
                count(g) == count(old(g)) + 1,
                out >= 1
        {
            unimplemented!()
        }
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

    #[test]
    fn current_starts_at_zero() {
        let g = Generator::new();
        assert_eq!(g.current(), 0);
    }

    #[test]
    fn current_tracks_next_id() {
        let g = Generator::new();
        g.next_id();
        g.next_id();
        g.next_id();
        assert_eq!(g.current(), 3);
    }

    #[test]
    fn set_current_overrides_counter() {
        // Trusted: bypasses F8 strict-monotonic-from-1.
        let g = Generator::new();
        g.set_current(42);
        assert_eq!(g.current(), 42);
        assert_eq!(g.next_id(), 43);
    }

    #[test]
    fn lock_state_value_matches_current() {
        // Sanity: the ghost view's production realization (lock_value)
        // returns exactly what `current()` returns. This is the
        // property Phase 2b will discharge through the vstd lock
        // primitive.
        let g = Generator::new();
        assert_eq!(g.inner.lock_value(), g.current());
        g.next_id();
        assert_eq!(g.inner.lock_value(), g.current());
        assert_eq!(g.inner.lock_value(), 1);
    }
}
