//! Verified business-logic layer.
//!
//! Composes `clock`, `ids`, `domain`, and `store`. The HTTP shim calls into
//! this crate and *only* this crate.
//!
//! # F-properties dispatched here
//! - **F1, F2** via `MemStore::home_timeline`
//! - **F4** via `domain::Follow::new`
//! - **F7** via `clock::Clock`
//! - **F8** via `ids::Generator`

use std::sync::Arc;

use clock::{Clock, Logical};
use domain::{DomainError, Follow, Tweet, User};
use ids::Generator;
use store::{MemStore, StoreError, StoreSnapshot};

// Re-export so admin handlers can construct/destructure without depending
// on `store` directly.
pub use store::StoreSnapshot as ServiceStoreSnapshot;

/// Errors raised by the service layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceError {
    EmptyHandle,
    EmptyText,
    SelfFollow,
    UnknownUser,
    DuplicateUser,
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::EmptyHandle => f.write_str("empty_handle"),
            ServiceError::EmptyText => f.write_str("empty_text"),
            ServiceError::SelfFollow => f.write_str("self_follow_forbidden"),
            ServiceError::UnknownUser => f.write_str("unknown_user"),
            ServiceError::DuplicateUser => f.write_str("duplicate_user"),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<DomainError> for ServiceError {
    fn from(e: DomainError) -> Self {
        match e {
            DomainError::SelfFollow => ServiceError::SelfFollow,
        }
    }
}

impl From<StoreError> for ServiceError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::UnknownUser => ServiceError::UnknownUser,
            StoreError::DuplicateUser => ServiceError::DuplicateUser,
        }
    }
}

/// The verified core. All exported methods are safe for concurrent use:
/// `MemStore` has its own `RwLock`, the `ids::Generator` instances have their
/// own mutexes, and the clock is a `dyn Clock + Send + Sync`.
pub struct Service {
    clk: Arc<dyn Clock>,
    tweet_ids: Generator,
    user_ids: Generator,
    st: MemStore,
}

impl Service {
    /// Build a new service with a fresh logical clock at t=0.
    pub fn new() -> Self {
        Self::new_with_clock(Arc::new(Logical::new()))
    }

    /// Build a service with the caller-provided clock. The conformance
    /// harness uses this to drive the clock by hand between steps.
    pub fn new_with_clock(clk: Arc<dyn Clock>) -> Self {
        Self {
            clk,
            tweet_ids: Generator::new(),
            user_ids: Generator::new(),
            st: MemStore::new(),
        }
    }

    /// Returns a clone of the clock handle so external drivers (the
    /// conformance test) can `tick()` without going through the service.
    pub fn clock(&self) -> Arc<dyn Clock> {
        self.clk.clone()
    }

    /// Advances the inner clock by one tick. Pure delegation to
    /// `Clock::tick`; the underlying F7 (timestamps non-decreasing,
    /// `tick` advances by exactly 1) is discharged in `crates/clock`
    /// (`tick_ensures`, Stream 3 Phase 1b).
    ///
    /// Stream 3 Phase 5 added this convenience method so the service
    /// surface has an explicit "advance time" entry point that mirrors
    /// the discharged clock primitive — previously callers reached
    /// through `service.clock().tick()`. Behavior is unchanged; this is
    /// shorthand for the same `Arc<dyn Clock>::tick` call.
    pub fn tick(&self) {
        self.clk.tick();
    }

    /// Registers a new user; rejects empty handles.
    pub fn create_user(&self, handle: &str) -> Result<User, ServiceError> {
        if handle.is_empty() {
            return Err(ServiceError::EmptyHandle);
        }
        let u = User { id: self.user_ids.next_id(), handle: handle.to_string() };
        self.st.put_user(u.clone())?;
        Ok(u)
    }

    /// Reports user existence.
    pub fn has_user(&self, handle: &str) -> bool {
        self.st.has_user(handle)
    }

    /// Records a follow edge. F4 rejects self-follow; F9 rejects unknown users.
    pub fn follow(&self, from: &str, to: &str) -> Result<(), ServiceError> {
        let f = Follow::new(from.to_string(), to.to_string())?;
        Ok(self.st.put_follow(f)?)
    }

    /// Removes a follow edge. Idempotent (F3): missing edges are 204 at the
    /// shim, but unknown users are still 400 (see `httpshim`).
    pub fn unfollow(&self, from: &str, to: &str) -> Result<(), ServiceError> {
        if !self.st.has_user(from) || !self.st.has_user(to) {
            return Err(ServiceError::UnknownUser);
        }
        self.st.delete_follow(from, to);
        Ok(())
    }

    /// Posts a tweet. F6 rejects unknown authors; F7 stamps with the clock;
    /// F8 issues a fresh strictly-monotonic ID.
    pub fn post_tweet(&self, author: &str, text: &str) -> Result<Tweet, ServiceError> {
        if text.is_empty() {
            return Err(ServiceError::EmptyText);
        }
        if !self.st.has_user(author) {
            return Err(ServiceError::UnknownUser);
        }
        let t = Tweet {
            id: self.tweet_ids.next_id(),
            author: author.to_string(),
            text: text.to_string(),
            created_at: self.clk.now(),
        };
        self.st.put_tweet(t.clone())?;
        Ok(t)
    }

    /// Returns the home timeline for `user`. F1 + F2 enforced by the store.
    pub fn home_timeline(&self, user: &str, limit: usize) -> Vec<Tweet> {
        self.st.home_timeline(user, limit)
    }

    /// Captures the full inner state of the verified core: clock value,
    /// both id-generator counters, and the store snapshot. Used by the
    /// Stream 2 admin snapshot endpoint.
    ///
    /// Trusted (TCB): exposes internals so they can leave the process.
    pub fn snapshot_state(&self) -> ServiceState {
        ServiceState {
            clock_now: self.clk.now(),
            id_counter_users: self.user_ids.current(),
            id_counter_tweets: self.tweet_ids.current(),
            store: self.st.snapshot(),
        }
    }

    /// **Trusted (TCB).** Atomically replaces the inner state. Bypasses
    /// every verified admission check; the producer of the snapshot is
    /// the source of truth. Used by the Stream 2 admin load-snapshot
    /// endpoint.
    pub fn load_state(&self, s: ServiceState) {
        self.st.replace(s.store);
        self.user_ids.set_current(s.id_counter_users);
        self.tweet_ids.set_current(s.id_counter_tweets);
        self.clk.set_now(s.clock_now);
    }
}

/// Typed inner state of the verified core. The HTTP/admin layer marshals
/// this to/from JSON; the service layer stays JSON-free.
///
/// Trusted (TCB): every field is normally hidden behind a verified
/// constructor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceState {
    pub clock_now: i64,
    pub id_counter_users: i64,
    pub id_counter_tweets: i64,
    pub store: StoreSnapshot,
}

impl Default for Service {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Verus proof obligations (F1, F2, F4 dispatched here; F6/F7/F8 by composition).
// =============================================================================
//
// # Stream 3 Phase 5 status — `Service::tick` composition (TCB-narrowing only)
//
// `Service::tick` is the smallest service-side composition: it is pure
// delegation to `Clock::tick`, whose F7 obligations
// (`now_ensures` / `tick_ensures`) are already discharged in `crates/clock`
// (Stream 3 Phase 1b — see `clock::verus_proof`).
//
// Phase 5 ships `Service::tick` as a thin wrapper but does **not** ship a
// verified `tick_ensures(s: &mut Service)` lemma in this block. The
// structural blockers:
//
//   1. `Service.clk` is `Arc<dyn Clock>` — a trait object behind an `Arc`.
//      Verus has no model of `Arc` or `dyn Trait` dispatch in
//      vstd 0.0.0-2026-04-20-1748 — chaining `clock::tick_ensures`
//      through the trait-object call would require either a vstd `Arc`
//      model or a custom trait-resolution shim, both out of scope here.
//   2. `clock::verus_proof` is a private module; its `ts(c)` ghost view
//      and `tick_ensures` lemma are not currently re-exported across
//      crate boundaries. Cross-crate `verus_proof` reuse is novel
//      infrastructure — no other crate in the workspace does it today,
//      and bootstrapping it for one delegation method is much larger
//      than the per-method sub-PR cadence the brief contemplates.
//
// Phase 5's deliverable is therefore TCB-narrowing only: the public
// `Service::tick` method exists (so future verified composition has a
// stable target), and the trusted-skeleton row in `TCB.md` is rewritten
// to call out the two blockers above explicitly. When `vstd` ships an
// `Arc<dyn Trait>` model (or when `Service` swaps `Arc<dyn Clock>` for a
// concrete `Arc<Logical>` field — a public-API change punted to a later
// sub-PR), this lemma can be discharged by chaining `clock::tick_ensures`
// through it.
//
// `post_tweet`, `follow`, `home_timeline` obligations remain documented
// above. Their full discharge requires the same trait-object-modeling work
// plus lifting Mutex/HashMap/Vec through vstd shims, scheduled for sub-PRs
// after the corresponding store methods land.
#[cfg(verus_only)]
mod verus_proof {
    use super::*;
    use vstd::prelude::*;
    verus! {
        // Trusted skeleton: see module-level commentary above for the
        // Phase 5 `Service::tick` composition status (TCB-narrowing
        // only; verified delegation blocked on `Arc<dyn Clock>`
        // modeling + cross-crate `verus_proof` reuse infrastructure).
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_user_rejects_empty() {
        let s = Service::new();
        assert_eq!(s.create_user("").unwrap_err(), ServiceError::EmptyHandle);
    }

    #[test]
    fn create_user_assigns_monotonic_ids() {
        let s = Service::new();
        let a = s.create_user("alice").unwrap();
        let b = s.create_user("bob").unwrap();
        let c = s.create_user("carol").unwrap();
        assert_eq!(a.id, 1);
        assert_eq!(b.id, 2);
        assert_eq!(c.id, 3);
    }

    #[test]
    fn create_user_duplicate() {
        let s = Service::new();
        s.create_user("alice").unwrap();
        assert_eq!(s.create_user("alice").unwrap_err(), ServiceError::DuplicateUser);
    }

    #[test]
    fn has_user() {
        let s = Service::new();
        assert!(!s.has_user("alice"));
        s.create_user("alice").unwrap();
        assert!(s.has_user("alice"));
    }

    #[test]
    fn follow_rejects_self_f4() {
        let s = Service::new();
        s.create_user("alice").unwrap();
        assert_eq!(s.follow("alice", "alice").unwrap_err(), ServiceError::SelfFollow);
    }

    #[test]
    fn follow_rejects_unknown() {
        let s = Service::new();
        s.create_user("alice").unwrap();
        assert_eq!(s.follow("alice", "ghost").unwrap_err(), ServiceError::UnknownUser);
        assert_eq!(s.follow("ghost", "alice").unwrap_err(), ServiceError::UnknownUser);
    }

    #[test]
    fn follow_idempotent_f3() {
        let s = Service::new();
        s.create_user("alice").unwrap();
        s.create_user("bob").unwrap();
        s.follow("alice", "bob").unwrap();
        s.follow("alice", "bob").unwrap();
        // No error; second call is a no-op.
    }

    #[test]
    fn unfollow_idempotent_on_missing() {
        let s = Service::new();
        s.create_user("alice").unwrap();
        s.create_user("bob").unwrap();
        s.unfollow("alice", "bob").unwrap();
        s.follow("alice", "bob").unwrap();
        s.unfollow("alice", "bob").unwrap();
        s.unfollow("alice", "bob").unwrap();
    }

    #[test]
    fn unfollow_rejects_unknown() {
        let s = Service::new();
        s.create_user("alice").unwrap();
        assert_eq!(s.unfollow("alice", "ghost").unwrap_err(), ServiceError::UnknownUser);
        assert_eq!(s.unfollow("ghost", "alice").unwrap_err(), ServiceError::UnknownUser);
    }

    #[test]
    fn post_tweet_rejects_empty_text() {
        let s = Service::new();
        s.create_user("alice").unwrap();
        assert_eq!(s.post_tweet("alice", "").unwrap_err(), ServiceError::EmptyText);
    }

    #[test]
    fn post_tweet_rejects_unknown_author_f6() {
        let s = Service::new();
        assert_eq!(s.post_tweet("ghost", "hi").unwrap_err(), ServiceError::UnknownUser);
    }

    #[test]
    fn post_tweet_uses_clock_and_ids() {
        let clk = Arc::new(Logical::new());
        let s = Service::new_with_clock(clk.clone());
        s.create_user("alice").unwrap();
        clk.tick(); // ts=1
        let t1 = s.post_tweet("alice", "first").unwrap();
        let t2 = s.post_tweet("alice", "second").unwrap();
        assert_eq!(t1.id, 1);
        assert_eq!(t2.id, 2);
        assert_eq!(t1.created_at, 1);
        assert_eq!(t2.created_at, 1); // tie allowed (F7)
    }

    #[test]
    fn home_timeline_visibility_f1_and_order_f2() {
        let clk = Arc::new(Logical::new());
        let s = Service::new_with_clock(clk.clone());
        s.create_user("alice").unwrap();
        s.create_user("bob").unwrap();
        s.create_user("carol").unwrap();
        s.follow("alice", "bob").unwrap();
        clk.tick(); // ts=1
        s.post_tweet("bob", "first").unwrap();        // id=1
        s.post_tweet("bob", "second").unwrap();       // id=2 same ts
        s.post_tweet("carol", "invisible").unwrap();  // id=3
        let tl = s.home_timeline("alice", 0);
        let ids: Vec<i64> = tl.iter().map(|t| t.id).collect();
        // F2 (id desc on tie), F1 (no carol)
        assert_eq!(ids, vec![2, 1]);
    }

    #[test]
    fn clock_handle_is_shared() {
        let clk = Arc::new(Logical::new());
        let s = Service::new_with_clock(clk.clone());
        let h = s.clock();
        h.tick();
        assert_eq!(clk.now(), 1);
    }

    #[test]
    fn service_tick_advances_inner_clock() {
        // Stream 3 Phase 5: `Service::tick` is pure delegation to the
        // shared clock handle. F7 (advance-by-1, non-decreasing) is
        // discharged in `clock::tick_ensures`.
        let clk = Arc::new(Logical::new());
        let s = Service::new_with_clock(clk.clone());
        assert_eq!(clk.now(), 0);
        s.tick();
        assert_eq!(clk.now(), 1);
        s.tick();
        s.tick();
        assert_eq!(clk.now(), 3);
    }

    #[test]
    fn service_error_display() {
        assert_eq!(ServiceError::EmptyHandle.to_string(), "empty_handle");
        assert_eq!(ServiceError::EmptyText.to_string(), "empty_text");
        assert_eq!(ServiceError::SelfFollow.to_string(), "self_follow_forbidden");
        assert_eq!(ServiceError::UnknownUser.to_string(), "unknown_user");
        assert_eq!(ServiceError::DuplicateUser.to_string(), "duplicate_user");
    }

    #[test]
    fn from_domain_error() {
        let e: ServiceError = DomainError::SelfFollow.into();
        assert_eq!(e, ServiceError::SelfFollow);
    }

    #[test]
    fn from_store_error() {
        let e: ServiceError = StoreError::UnknownUser.into();
        assert_eq!(e, ServiceError::UnknownUser);
        let e: ServiceError = StoreError::DuplicateUser.into();
        assert_eq!(e, ServiceError::DuplicateUser);
    }

    #[test]
    fn default_is_new() {
        let _ = Service::default();
    }

    #[test]
    fn home_timeline_unknown_user_returns_empty() {
        let s = Service::new();
        assert!(s.home_timeline("ghost", 0).is_empty());
    }

    #[test]
    fn service_error_std_error() {
        let e = ServiceError::EmptyText;
        let _: &dyn std::error::Error = &e;
    }

    #[test]
    fn snapshot_state_captures_full_state() {
        let clk = Arc::new(Logical::new());
        let s = Service::new_with_clock(clk.clone());
        s.create_user("alice").unwrap();
        s.create_user("bob").unwrap();
        s.follow("alice", "bob").unwrap();
        clk.tick();
        clk.tick();
        s.post_tweet("alice", "hi").unwrap();

        let snap = s.snapshot_state();
        assert_eq!(snap.clock_now, 2);
        assert_eq!(snap.id_counter_users, 2);
        assert_eq!(snap.id_counter_tweets, 1);
        assert_eq!(snap.store.users.len(), 2);
        assert_eq!(snap.store.follows.len(), 1);
        assert_eq!(snap.store.tweets.len(), 1);
    }

    #[test]
    fn load_state_round_trips_through_a_fresh_service() {
        let clk_a = Arc::new(Logical::new());
        let a = Service::new_with_clock(clk_a.clone());
        a.create_user("alice").unwrap();
        a.create_user("bob").unwrap();
        a.follow("alice", "bob").unwrap();
        clk_a.tick();
        a.post_tweet("alice", "hi").unwrap();
        a.post_tweet("bob", "yo").unwrap();
        let snap = a.snapshot_state();

        let clk_b = Arc::new(Logical::new());
        let b = Service::new_with_clock(clk_b.clone());
        b.load_state(snap.clone());

        assert_eq!(b.snapshot_state(), snap);
        // ids continue from where the snapshot left off — F8 still holds
        // for new IDs after a load.
        let new_user = b.create_user("carol").unwrap();
        assert_eq!(new_user.id, 3);
    }

    #[test]
    fn load_state_replaces_existing_state() {
        let s = Service::new();
        s.create_user("ghost").unwrap();
        let new_state = ServiceState {
            clock_now: 7,
            id_counter_users: 3,
            id_counter_tweets: 0,
            store: store::StoreSnapshot {
                users: vec![
                    User { id: 1, handle: "alice".into() },
                    User { id: 2, handle: "bob".into() },
                    User { id: 3, handle: "carol".into() },
                ],
                follows: vec![],
                tweets: vec![],
            },
        };
        s.load_state(new_state);
        assert!(!s.has_user("ghost"));
        assert!(s.has_user("alice"));
        assert!(s.has_user("bob"));
        assert!(s.has_user("carol"));
    }
}
