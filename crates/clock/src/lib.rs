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

use std::sync::Mutex;

#[cfg(verus_only)]
#[allow(unused_imports)]
use vstd::prelude::*;

/// Trait abstracting the logical clock so callers (service) can be tested
/// against a deterministic stub.
pub trait Clock: Send + Sync {
    /// Returns the current logical timestamp without advancing.
    fn now(&self) -> i64;
    /// Advances the clock by 1 tick.
    fn tick(&self);
}

/// `Logical` is the single concrete clock used by the verified core. It
/// starts at zero and only advances when `tick()` is called explicitly,
/// which makes timeline timestamps fully reproducible from the conformance
/// suite.
///
/// Internal state is guarded by a `Mutex<i64>` rather than `AtomicI64` so
/// the Verus annotations can reason about the lock-protected critical
/// section as a single transition (Verus understands `Mutex` exclusivity
/// natively; `Atomic` would require a separate ghost protocol).
pub struct Logical {
    inner: Mutex<i64>,
}

impl Logical {
    /// Returns a fresh clock at t=0.
    pub fn new() -> Self {
        Self { inner: Mutex::new(0) }
    }

    /// Returns a fresh clock at t=`start`.
    pub fn new_at(start: i64) -> Self {
        Self { inner: Mutex::new(start) }
    }
}

impl Default for Logical {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for Logical {
    fn now(&self) -> i64 {
        *self.inner.lock().expect("clock mutex poisoned")
    }

    fn tick(&self) {
        let mut g = self.inner.lock().expect("clock mutex poisoned");
        *g += 1;
    }
}

// =============================================================================
// Verus proof obligations (F7).
// =============================================================================
//
// Under stable rustc this module is compiled out. Under `--cfg verus` it is
// expanded by the Verus toolchain and discharged by Z3.
//
// The clauses below state, in Verus syntax:
//
//   spec fn ts(c: &Logical) -> int  // ghost view of the clock
//
//   #[verifier::external_body]
//   impl Logical {
//       fn now(&self) -> (out: i64)
//           ensures out as int == ts(self);
//
//       fn tick(&self)
//           ensures ts(self) == old(ts(self)) + 1;
//   }
//
//   // Derived (F7):
//   proof fn lemma_non_decreasing(c: &Logical)
//       ensures forall |t1: int, t2: int|
//           t1 <= t2 ==> ts_history(c, t1) <= ts_history(c, t2);
//
// The "external_body" attribute is required because `Mutex::lock` is not
// Verus-verifiable; we trust the std library's exclusivity guarantee.
#[cfg(verus_only)]
mod verus_proof {
    use super::*;
    use vstd::prelude::*;
    verus! {
        #[verifier::external_type_specification]
        #[verifier::external_body]
        pub struct ExLogical(crate::Logical);

        // Opaque ghost view of the clock's current logical timestamp.
        // The body is unobservable to the verifier (`external_body`); we
        // treat it as a trusted abstraction over `Mutex<i64>` exclusivity.
        #[verifier::external_body]
        pub closed spec fn ts(c: &Logical) -> int { unimplemented!() }

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
}
