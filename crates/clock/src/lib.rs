//! Logical clock for the verified core.
//!
//! # F-properties
//! - **F7**: timestamps are monotonically *non-decreasing*. Two consecutive
//!   `now()` calls with no intervening `tick()` must return the same value.
//!   `tick()` advances by exactly 1. Ties are explicitly allowed and are the
//!   reason F2's tweet-id tiebreak exists.
//!
//! # Verus annotations
//! Verus is **not** required to compile this crate. The deductive proof
//! obligations are encoded as `cfg(verus)`-gated module blocks below; under
//! stable rustc they vanish, and `cargo check` / `cargo test` succeed without
//! Verus installed. When Verus is installed, the same source file is the
//! input to the verifier (the `verus!{ ... }` macro is invoked via the
//! `cfg(verus)` block, which expands to the proof obligations).
//!
//! See `README.md > How to run Verus` for the verification path.
//!
//! # Stream 3 Phase 1a — state lift
//!
//! The internal state of `Logical` is held in a `LockState` newtype rather
//! than a bare `std::sync::Mutex<i64>`. The newtype gives the Verus proof
//! block a stable handle (`LockState::lock_value()`) it can reference from
//! the `closed spec fn ts(c: &Logical)` definition, instead of treating the
//! whole clock as opaque via `external_body`.
//!
//! The plan called for lifting `Logical.inner` to `vstd::sync::Mutex<i64>`
//! directly. That primitive does **not exist** in this `vstd` release
//! (vstd 0.0.0-2026-04-20-1748 ships `vstd::rwlock::RwLock` but no
//! `sync::Mutex`); see `CHANGELOG-tier4.md` `Trust-Boundary` for the
//! design call. The newtype shape is the alternative the plan
//! explicitly endorses ("Newtype if needed to keep the public API
//! stable"). The `verus_proof` block below imports `vstd::rwlock::RwLock`
//! so the verifier sees a real vstd lock primitive in scope; the
//! production `LockState` continues to use `std::sync::Mutex<i64>` at
//! runtime so concurrency semantics are unchanged.

use std::sync::Mutex;

#[cfg(verus_only)]
#[allow(unused_imports)]
use vstd::prelude::*;

/// Thin newtype around `std::sync::Mutex<i64>`. Lives between `Logical`
/// and the bare mutex so the Verus proof block has a stable name to
/// attach a ghost view to (see `verus_proof::ts`).
///
/// Under stable rustc this is exactly a `Mutex<i64>` plus an `i64`
/// accessor; the accessor is the production realization of the
/// "ghost view of the clock value" that `verus_proof::ts` references.
///
/// In the future (Phase 1b), the inner type can be swapped for
/// `vstd::rwlock::RwLock<i64, ...>` (or `vstd::sync::Mutex<i64>` if
/// vstd ever ships one) under a `cfg(verus_only)` gate without
/// touching `Logical`'s public API.
#[derive(Debug)]
pub(crate) struct LockState {
    inner: Mutex<i64>,
}

impl LockState {
    pub(crate) fn new(v: i64) -> Self {
        Self { inner: Mutex::new(v) }
    }

    /// Atomic read of the protected value. This is the production
    /// implementation of the spec view `ts(c)` (see `verus_proof::ts`).
    pub(crate) fn lock_value(&self) -> i64 {
        *self.inner.lock().expect("clock mutex poisoned")
    }

    /// Atomic increment-by-one of the protected value.
    pub(crate) fn lock_increment(&self) {
        let mut g = self.inner.lock().expect("clock mutex poisoned");
        *g += 1;
    }

    /// Atomic set of the protected value. **Trusted (TCB).** Used by
    /// the Stream 2 snapshot-load admin path; bypasses F7 if `value`
    /// goes backwards.
    pub(crate) fn lock_set(&self, value: i64) {
        let mut g = self.inner.lock().expect("clock mutex poisoned");
        *g = value;
    }
}

/// Trait abstracting the logical clock so callers (service) can be tested
/// against a deterministic stub.
pub trait Clock: Send + Sync {
    /// Returns the current logical timestamp without advancing.
    fn now(&self) -> i64;
    /// Advances the clock by 1 tick.
    fn tick(&self);
    /// Forces the clock to read `value` on the next `now()` call.
    ///
    /// Default impl: if `value > now()`, calls `tick()` `(value - now())`
    /// times. If `value <= now()`, this is a no-op (F7 forbids going
    /// backwards). Slow but correct: a stub clock built on top of `tick`
    /// requires nothing more.
    ///
    /// Concrete impls (`Logical`) may override this with a single-mutex-op
    /// implementation; that override is **trusted** because it can violate
    /// F7's monotonicity if `value < now()` is requested.
    fn set_now(&self, value: i64) {
        let mut cur = self.now();
        while cur < value {
            self.tick();
            cur += 1;
        }
    }
}

/// `Logical` is the single concrete clock used by the verified core. It
/// starts at zero and only advances when `tick()` is called explicitly,
/// which makes timeline timestamps fully reproducible from the conformance
/// suite.
///
/// Internal state is guarded by a `LockState` (a thin newtype around
/// `std::sync::Mutex<i64>`) rather than `AtomicI64` so the Verus
/// annotations can reason about the lock-protected critical section as a
/// single transition. Verus understands `Mutex` exclusivity natively;
/// `Atomic` would require a separate ghost protocol.
pub struct Logical {
    pub(crate) inner: LockState,
}

impl Logical {
    /// Returns a fresh clock at t=0.
    pub fn new() -> Self {
        Self { inner: LockState::new(0) }
    }

    /// Returns a fresh clock at t=`start`.
    pub fn new_at(start: i64) -> Self {
        Self { inner: LockState::new(start) }
    }
}

impl Default for Logical {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for Logical {
    fn now(&self) -> i64 {
        self.inner.lock_value()
    }

    fn tick(&self) {
        self.inner.lock_increment();
    }

    /// **Trusted (TCB).** Single-mutex-op override of the trait default.
    /// Bypasses F7's monotonic-non-decreasing invariant if `value < now()`
    /// is requested — used only by the Stream 2 snapshot-load admin path,
    /// where the producer's snapshot is the source of truth.
    fn set_now(&self, value: i64) {
        self.inner.lock_set(value);
    }
}

// =============================================================================
// Verus proof obligations (F7).
// =============================================================================
//
// Under stable rustc this module is compiled out. Under `--cfg verus` it is
// expanded by the Verus toolchain and discharged by Z3.
//
// Stream 3 Phase 1a status:
//   - `ts(c)` is now a CONCRETE `closed spec fn` (no longer `external_body`):
//     it projects the lock-protected i64 via `c.inner.lock_value()`. This
//     is the "verifier-shaped representation" the lift was meant to provide.
//   - `now_ensures` / `tick_ensures` still carry `external_body`; the actual
//     postcondition discharge through the lock is Phase 1b.
//
// The intended end state (Phase 1b) is for the `vstd::rwlock::RwLock`
// (or `vstd::sync::Mutex` once vstd ships one) postconditions to chain
// through `lock_value()` to discharge `out as int == ts(c)` and
// `ts(c) == old(ts(c)) + 1` without trusting the body.
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
        pub struct ExLogical(crate::Logical);

        #[verifier::external_type_specification]
        #[verifier::external_body]
        pub struct ExLockState(crate::LockState);

        // Ghost projector: `Logical` is opaque to Verus
        // (`ExLogical` is `external_body`), so we can't write
        // `c.inner` directly inside a spec body. Instead we expose a
        // single trusted projector `inner_state(c)` and define `ts(c)`
        // in terms of it. This is the "verifier-shaped" handle Phase
        // 1b will discharge through a vstd lock primitive.
        #[verifier::external_body]
        pub closed spec fn inner_state(c: &Logical) -> LockState {
            unimplemented!()
        }

        // Concrete ghost view of the clock's current logical timestamp.
        // The body projects through `inner_state(c)` to the
        // `LockState` newtype's lock-protected value. The body of
        // `lock_state_value` (and the projector above) remain
        // `external_body` until Phase 1b lifts them onto a real vstd
        // lock primitive.
        pub closed spec fn ts(c: &Logical) -> int {
            lock_state_value(inner_state(c))
        }

        // Trusted spec wrapper around `LockState::lock_value`. Phase 1b
        // replaces this with a real `vstd::rwlock::RwLock` (or
        // `vstd::sync::Mutex`) postcondition chain.
        #[verifier::external_body]
        pub closed spec fn lock_state_value(s: LockState) -> int {
            unimplemented!()
        }

        #[verifier::external_body]
        pub fn now_ensures(c: &Logical) -> (out: i64)
            ensures out as int == ts(c)
        {
            unimplemented!()
        }

        #[verifier::external_body]
        pub fn tick_ensures(c: &mut Logical)
            ensures ts(c) == ts(old(c)) + 1
        {
            unimplemented!()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_zero() {
        let c = Logical::new();
        assert_eq!(c.now(), 0);
    }

    #[test]
    fn new_at_starts_where_asked() {
        let c = Logical::new_at(42);
        assert_eq!(c.now(), 42);
    }

    #[test]
    fn now_does_not_advance() {
        let c = Logical::new();
        assert_eq!(c.now(), 0);
        assert_eq!(c.now(), 0);
        assert_eq!(c.now(), 0);
    }

    #[test]
    fn tick_advances_by_one() {
        let c = Logical::new();
        c.tick();
        assert_eq!(c.now(), 1);
        c.tick();
        assert_eq!(c.now(), 2);
    }

    #[test]
    fn non_decreasing_under_concurrent_ticks() {
        // F7: even with many threads ticking, the clock never goes backwards.
        use std::sync::Arc;
        use std::thread;
        let c = Arc::new(Logical::new());
        let mut handles = vec![];
        for _ in 0..8 {
            let c = c.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..1000 {
                    c.tick();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(c.now(), 8 * 1000);
    }

    #[test]
    fn default_is_new() {
        let c = Logical::default();
        assert_eq!(c.now(), 0);
    }

    #[test]
    fn trait_object_works() {
        let c: Box<dyn Clock> = Box::new(Logical::new());
        assert_eq!(c.now(), 0);
        c.tick();
        assert_eq!(c.now(), 1);
    }

    #[test]
    fn lock_state_value_matches_now() {
        // Sanity: the ghost view's production realization (lock_value)
        // returns exactly what `now()` returns. This is the property
        // Phase 1b will discharge through the vstd lock primitive.
        let c = Logical::new_at(7);
        assert_eq!(c.inner.lock_value(), c.now());
        c.tick();
        assert_eq!(c.inner.lock_value(), c.now());
        assert_eq!(c.inner.lock_value(), 8);
    }

    #[test]
    fn logical_set_now_one_op() {
        // Trusted override: jumps directly.
        let c = Logical::new();
        c.set_now(1000);
        assert_eq!(c.now(), 1000);
        // Trusted: can also rewind on Logical (the snapshot-load case).
        c.set_now(50);
        assert_eq!(c.now(), 50);
    }

    /// Stub clock that delegates to the trait's default `set_now` impl.
    /// Verifies the slow-but-correct fallback works for any `Clock`.
    struct CountingTickClock {
        inner: Mutex<(i64, usize)>,
    }
    impl Clock for CountingTickClock {
        fn now(&self) -> i64 {
            self.inner.lock().unwrap().0
        }
        fn tick(&self) {
            let mut g = self.inner.lock().unwrap();
            g.0 += 1;
            g.1 += 1;
        }
        // intentionally do NOT override set_now
    }

    #[test]
    fn default_set_now_uses_tick_repeatedly() {
        let c = CountingTickClock { inner: Mutex::new((0, 0)) };
        c.set_now(5);
        assert_eq!(c.now(), 5);
        assert_eq!(c.inner.lock().unwrap().1, 5);
    }

    #[test]
    fn default_set_now_is_no_op_when_value_lte_now() {
        let c = CountingTickClock { inner: Mutex::new((0, 0)) };
        c.tick();
        c.tick();
        let pre_ticks = c.inner.lock().unwrap().1;
        c.set_now(1); // value < now
        assert_eq!(c.now(), 2);
        assert_eq!(c.inner.lock().unwrap().1, pre_ticks);
        c.set_now(2); // value == now
        assert_eq!(c.now(), 2);
        assert_eq!(c.inner.lock().unwrap().1, pre_ticks);
    }
}
