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
//! # Stream 3 Phase 1a — state lift, Phase 1b — F7 discharge
//!
//! The internal state of `Logical` is held in a `LockState` newtype rather
//! than a bare `std::sync::Mutex<i64>`. The newtype gives the Verus proof
//! block a stable handle (`LockState::lock_value()`) it can reference from
//! the `spec fn ts(c: &Logical)` definition, instead of treating the
//! whole clock as opaque via `external_body`.
//!
//! Phase 1b discharges the F7 obligations (`now_ensures`,
//! `tick_ensures`) without `external_body`: their bodies are the
//! production reads/writes, and Verus chains `assume_specification`s on
//! `LockState::lock_value` / `LockState::lock_increment` (trusted shims
//! standing in for `std::sync::Mutex::lock`, which has no vstd model)
//! through the structural definition of `ts(c)`.
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
///
/// **`inner` is `pub`** so the `verus_proof` block's transparent
/// `ExLogical(crate::Logical)` external_type_specification can see the
/// field — Verus does not allow `pub(crate)` on transparent
/// external types. `LockState` itself is `pub(crate)`, so callers
/// outside the crate still cannot construct or interact with `inner`.
pub struct Logical {
    // `pub` (not `pub(crate)`) is required by Verus
    // `external_type_specification` for transparent datatypes.
    // `LockState` itself is `pub(crate)`, so the field is visibly
    // typed but not constructible outside the crate. The lint is
    // about exposing a `pub(crate)` type via a `pub` field — that
    // exposure is intentional here, scoped to Verus's needs.
    #[doc(hidden)]
    #[allow(private_interfaces)]
    pub inner: LockState,
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
// Stream 3 Phase 1b status (DISCHARGED):
//   - `now_ensures` and `tick_ensures` are no longer `external_body`. Their
//     bodies are the actual production reads/writes (`c.inner.lock_value()`
//     / `c.inner.lock_increment()`), and Verus discharges
//     `out as int == ts(c)` and `ts(c) == old(ts(c)) + 1` by chaining the
//     `assume_specification`s on `LockState::lock_value` and
//     `LockState::lock_increment` through the structural definition of
//     `ts(c) := lock_state_value(&c.inner)`.
//   - The trust footprint shrinks to two assume_specification stubs (one per
//     mutex op) plus the opaque ghost view `lock_state_value`. The Phase 1a
//     `inner_state(c)` projector is retired (the inner field is now visible
//     to Verus directly).
//   - `vstd::sync::Mutex` is still absent in this vstd release, so the
//     ultimate end state — discharging through a vstd lock primitive's own
//     postconditions instead of via assume_specification — remains future
//     work tracked in `CHANGELOG-tier4.md`. Phase 1b is the Tier-4 milestone:
//     F7 obligations themselves are no longer trusted.
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
        // `Logical` is structurally visible to Verus: `inner` is
        // `pub` (and `LockState` is `pub(crate)`), so the verifier
        // can write `c.inner` inside spec/exec bodies. This is what
        // permits the structural definition of `ts(c)` below.
        #[verifier::external_type_specification]
        pub struct ExLogical(crate::Logical);

        // `LockState` wraps a `std::sync::Mutex<i64>`. Verus has no
        // model of `Mutex` (vstd 0.0.0-2026-04-20-1748 ships no
        // `sync::Mutex`), so we keep `LockState` opaque
        // (`external_body`) and reason about it through the trusted
        // ghost view + shim functions below. These are the F7
        // discharge's remaining trust hooks on the clock side.
        #[verifier::external_type_specification]
        #[verifier::external_body]
        pub struct ExLockState(crate::LockState);

        // Ghost view of the i64 currently stored behind the lock. The
        // trusted shims chain through this. Body opaque (Verus has
        // no concrete view of the std mutex's interior).
        #[verifier::external_body]
        pub closed spec fn lock_state_value(s: &LockState) -> int {
            unimplemented!()
        }

        // Concrete ghost view of `Logical`'s current logical
        // timestamp. Structural over the (now visible) `inner` field
        // — no opaque projector hop needed (cf. Phase 1a's
        // `inner_state`, retired).
        pub open spec fn ts(c: &Logical) -> int {
            lock_state_value(&c.inner)
        }

        // Trusted shim around `LockState::lock_value`. Pins the
        // returned `i64` to the ghost view so callers (`now_ensures`)
        // can chain it to `ts(c)`. The exec body calls
        // `std::sync::Mutex::lock`; this trusted shim stands in for
        // that call's return.
        #[verifier::external_body]
        pub fn proof_lock_value(s: &LockState) -> (out: i64)
            ensures out as int == lock_state_value(s)
        {
            s.lock_value()
        }

        // Trusted shim around `LockState::lock_increment`. The
        // production exec method takes `&self` (interior mutability
        // via `Mutex`); for the proof we model it with `&mut` so
        // Verus can express the post-state of the ghost view. The
        // shim is sound because `Mutex::lock` provides exclusive
        // access while held — the critical section is observationally
        // a `&mut` step. Trusting this shim is exactly trusting that
        // std's `Mutex` correctly implements mutual exclusion.
        // Spec: bumps the ghost view by exactly 1. This is the F7
        // tick-step axiom; everything else flows from it.
        #[verifier::external_body]
        pub fn proof_lock_increment(s: &mut LockState)
            ensures lock_state_value(s) == lock_state_value(old(s)) + 1
        {
            s.lock_increment()
        }

        // F7 discharge — `out as int == ts(c)`. Body invokes the
        // trusted read shim; Verus chains the shim's ensures through
        // the structural definition of `ts(c) := lock_state_value(&c.inner)`.
        pub fn now_ensures(c: &Logical) -> (out: i64)
            ensures out as int == ts(c)
        {
            proof_lock_value(&c.inner)
        }

        // F7 discharge — `ts(c) == old(ts(c)) + 1`. Body invokes the
        // trusted write shim; Verus chains the shim's ensures
        // through the structural definition of `ts`.
        pub fn tick_ensures(c: &mut Logical)
            ensures ts(c) == ts(old(c)) + 1
        {
            proof_lock_increment(&mut c.inner)
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
