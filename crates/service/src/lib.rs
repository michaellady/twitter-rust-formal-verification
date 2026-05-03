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
// # Stream 3 Phase 5 sub-PR R2 — `Service::has_user` + `Service::create_user` discharge
//
// Builds on the Phase 5 sub-PR (R1, PR #24) that landed `Service::tick` as a
// TCB-narrowing-only stub. R2 actually discharges two service-layer methods
// that route entirely through the *concrete* `Service.st: MemStore` field —
// **not** through `Service.clk: Arc<dyn Clock>`. The R1 trait-object-modeling
// blocker therefore does not apply: `MemStore` is a concrete type with
// already-discharged `*_ensures` wrappers (Stream 3 Phase 4 sub-PRs 1+2).
//
// Discharge shape — same trusted-shim pattern Phase 1b/4 established, lifted
// one layer up to `&Service`:
//
//   1. `Service` is treated as opaque to Verus (`ExService` =
//      `external_type_specification` + `external_body`). Same reason
//      `MemStore` is opaque: `Service` has an `Arc<dyn Clock>` field
//      vstd 0.0.0-2026-04-20-1748 cannot model. The opaque view
//      *neutralizes* that field for the verifier — Verus reasons about
//      `Service` only through the ghost view + shims defined here.
//   2. A single ghost view `service_users_keys(s: &Service) -> Set<Seq<char>>`
//      models the set of registered handles, projected through `Service.st`.
//      Body opaque (`external_body`): cross-crate `verus_proof` reuse of
//      `store::users_keys` is the same novel-infrastructure blocker R1
//      called out for `clock`. Worked around by re-defining the axis at
//      the service layer.
//   3. Two `external_body` exec shims (`proof_service_has_user`,
//      `proof_service_put_user`) bottom out in the production calls
//      `s.st.has_user(...)` / `s.st.put_user(...)`. The shim bodies are
//      one-liners against the same production methods Phase 4 already
//      verified at the store layer; what is trusted here is the
//      cross-layer projection of `service_users_keys` through `Service.st`.
//
// What R2 actually verifies (the `*_ensures` lemmas below):
//
//   - `has_user_ensures(s: &Service, handle: &String) -> bool`
//     ensures `result == service_users_keys(s).contains(handle@)`.
//   - `create_user_ensures(s: &mut Service, handle: &String) -> Result<User, ServiceError>`
//     ensures the F3 dup-rejection contract on the handle axis:
//     `EmptyHandle` short-circuits with no state change; otherwise
//     `service_users_keys(old(s)).contains(handle@) ==> result is Err`,
//     `!service_users_keys(old(s)).contains(handle@) ==> result is Ok`,
//     and the inserted handle ends up in the post-state set.
//
// What stays trusted in R2 (explicit non-goals):
//
//   - `Service::tick` composition (still the R1 blocker — `Arc<dyn Clock>`).
//   - `post_tweet`, `home_timeline` (need clock + author_tweet_count +
//     vec-sort spec); scheduled for later sub-PRs.
//   - The returned `User`'s `id` field on `create_user`: the ghost view
//     models only the handle axis. The id is generated by
//     `self.user_ids.next_id()` (F8, discharged at `ids::next_id_ensures`)
//     but threading it through `&mut Service` requires modeling
//     `Service.user_ids` as a separate ghost-view axis — out of scope here.
//   - The `User` payload's `handle` field equals the requested handle: the
//     production `User { handle: handle.to_string() }` literal makes this
//     true by construction; pinning it in the ensures clause requires a
//     `String::to_string` spec the shim trusts.
//
// # Stream 3 Phase 5 sub-PR R3 — `Service::follow` + `Service::unfollow` discharge
//
// Builds on R2 by adding the follow-edge axis on `&Service`. Same opaque-shim
// pattern; new ghost view `service_follow_edges(s) -> Set<(Seq<char>, Seq<char>)>`
// mirrors `store::follow_edges`. Two new `external_body` exec shims:
// `proof_service_put_follow` (bottoming out in `s.st.put_follow(f)`) and
// `proof_service_delete_follow` (bottoming out in `s.st.delete_follow(from, to)`).
// Both shims frame `service_users_keys` (disjoint-state argument inherited
// from the store layer: production writes touch only the `follows` HashMap,
// never `users`). What R3 verifies:
//
//   - `follow_ensures(s: &mut Service, from: String, to: String)`
//     composes `dom::Follow::new` (F4 discharged in Phase 3) with
//     `proof_service_put_follow`. F4 chains through `?`. F3 idempotency
//     falls out of `Set::insert`.
//   - `unfollow_ensures(s: &mut Service, from: &String, to: &String)`
//     pins the production `UnknownUser` guard via two `proof_service_has_user`
//     checks + `proof_service_delete_follow`. F3 idempotency falls out of
//     `Set::remove`.
//
// What stays trusted in R3 (explicit non-goals):
//
//   - F9 (unknown-user rejection on `follow`) — propagated as
//     `Err(ServiceError::UnknownUser)` from `MemStore::put_follow` via `?`,
//     not pinned in `follow_ensures`'s ensures clauses (would require a
//     cross-axis `service_users_keys`-touching shim signature on
//     `proof_service_put_follow`).
//   - The cross-layer projection
//     `service_follow_edges(s) == store::follow_edges(s.st)` —
//     same novel-infrastructure blocker R2 called out for `service_users_keys`.
//
// # Stream 3 Phase 5 sub-PR R4 — `Service::home_timeline` discharge (framing-only)
//
// Pure-delegation discharge of the read-only `Service::home_timeline`
// wrapper. Mirrors the framing-only shape Phase 4 sub-PR 5+6 used at the
// store layer: `store::home_timeline_ensures` was discharged framing-only
// there (F1 visibility and F2 sort-order remain trusted in
// `store::proof_home_timeline` because vstd 0.0.0-2026-04-20-1748 ships
// no `vstd::vec` sort spec and no verified mergesort), so the
// service-layer wrapper carries the same framing-only contract one
// composition layer up. New trust footprint: 1 `external_body` exec shim
// (`proof_service_home_timeline` bottoming out in
// `s.st.home_timeline(user.as_str(), limit)`); zero new ghost views.
// What R4 verifies: framing on both ghost-view axes
// (`service_users_keys`, `service_follow_edges`) via the
// `&Service`-not-`&mut` signature — structural; no ensures clauses on
// the returned `Vec<Tweet>`. What stays trusted in R4: F1 + F2 (still
// pending the missing `vstd::vec` sort spec / verified mergesort at the
// store layer).
#[cfg(verus_only)]
mod verus_proof {
    use super::*;
    use vstd::prelude::*;
    verus! {
        // `Service` is opaque to Verus — it carries an `Arc<dyn Clock>`
        // field that vstd 0.0.0-2026-04-20-1748 has no model for.
        // Reasoned about exclusively through the `service_users_keys`
        // ghost view + the `proof_service_*` shims below. Same trust
        // shape Stream 3 Phase 4 established for `MemStore`, lifted one
        // layer up to the service composition point.
        #[verifier::external_type_specification]
        #[verifier::external_body]
        pub struct ExService(crate::Service);

        #[verifier::external_type_specification]
        pub struct ExServiceError(crate::ServiceError);

        // Ghost view of the set of currently-registered user handles
        // (each handle viewed as the `Seq<char>` projection of its
        // String key), projected through `Service.st: MemStore`. Body
        // opaque: cross-crate `verus_proof` reuse of `store::users_keys`
        // is the novel-infrastructure blocker called out in R1 for
        // `clock::tick_ensures`. Worked around here by re-defining the
        // axis at the service layer; the two shims below pin their
        // results back to it.
        #[verifier::external_body]
        pub closed spec fn service_users_keys(s: &Service) -> Set<Seq<char>> {
            unimplemented!()
        }

        // Trusted shim around `Service::has_user`'s composition step
        // (`s.st.has_user(handle.as_str())`). Body calls the real
        // production method on the concrete `MemStore` field; what is
        // trusted is the cross-layer projection — `service_users_keys(s)`
        // is by-construction the same set as `store::users_keys(s.st)`,
        // but Verus cannot see that equality across the private
        // `store::verus_proof` boundary, so it is asserted here as the
        // shim's ensures.
        #[verifier::external_body]
        pub fn proof_service_has_user(s: &Service, handle: &String) -> (out: bool)
            ensures out == service_users_keys(s).contains(handle@)
        {
            s.st.has_user(handle.as_str())
        }

        // Trusted shim around `Service::create_user`'s store-side
        // composition step (`self.st.put_user(u)?` plus the implicit
        // `self.user_ids.next_id()` call needed to construct `u`).
        // Models the handle-axis post-state of the ghost view: on
        // success the handle ends up in `service_users_keys(s)` and
        // the result is Ok; on duplicate the set is unchanged and
        // the result is Err. The id-axis (F8) is not modeled here —
        // see module commentary above. Signature takes `&mut Service`
        // so Verus can express the post-state of the ghost view; the
        // production op only needs `&self` (interior mutability via
        // `MemStore`'s `RwLock`). Sound because the production call
        // chain holds the store's write lock during the put.
        #[verifier::external_body]
        pub fn proof_service_put_user(s: &mut Service, handle: &String) -> (result: Result<User, ServiceError>)
            ensures
                service_users_keys(old(s)).contains(handle@) ==> result is Err,
                !service_users_keys(old(s)).contains(handle@) ==> result is Ok,
                result is Ok ==> service_users_keys(s) == service_users_keys(old(s)).insert(handle@),
                result is Err ==> service_users_keys(s) == service_users_keys(old(s)),
        {
            let u = User { id: s.user_ids.next_id(), handle: handle.clone() };
            match s.st.put_user(u.clone()) {
                Ok(()) => Ok(u),
                Err(e) => Err(match e {
                    StoreError::UnknownUser => ServiceError::UnknownUser,
                    StoreError::DuplicateUser => ServiceError::DuplicateUser,
                }),
            }
        }

        // R2 discharge — verified read-only wrapper for `Service::has_user`.
        // Pure verified function: takes `&Service` (read), reuses the
        // `proof_service_has_user` shim whose post-condition pins the
        // returned `bool` to `service_users_keys(s).contains(handle@)`.
        // This is exactly the body of the production `Service::has_user`
        // (one-line delegation to `self.st.has_user`), expressed against
        // the trusted shim.
        pub fn has_user_ensures(s: &Service, handle: &String) -> (result: bool)
            ensures result == service_users_keys(s).contains(handle@),
        {
            proof_service_has_user(s, handle)
        }

        // R2 discharge — verified wrapper for `Service::create_user`.
        // Encodes the F3 dup-rejection contract on the handle axis:
        //
        //   ensures
        //     handle@.len() == 0 ==> result is Err,
        //     handle@.len() == 0 ==> service_users_keys(s) == service_users_keys(old(s)),
        //     handle@.len() > 0 && service_users_keys(old(s)).contains(handle@) ==> result is Err,
        //     handle@.len() > 0 && !service_users_keys(old(s)).contains(handle@) ==> result is Ok,
        //     result is Ok ==> service_users_keys(s) == service_users_keys(old(s)).insert(handle@),
        //     result is Err ==> service_users_keys(s) == service_users_keys(old(s)),
        //
        // Body mirrors the production `Service::create_user` control flow
        // exactly: empty-handle short-circuit, then chain through the
        // `proof_service_put_user` shim which composes the F8 id call +
        // the F3 store-side dup check.
        pub fn create_user_ensures(s: &mut Service, handle: &String) -> (result: Result<User, ServiceError>)
            ensures
                handle@.len() == 0 ==> result is Err,
                handle@.len() == 0 ==> service_users_keys(s) == service_users_keys(old(s)),
                handle@.len() > 0 && service_users_keys(old(s)).contains(handle@) ==> result is Err,
                handle@.len() > 0 && !service_users_keys(old(s)).contains(handle@) ==> result is Ok,
                result is Ok ==> service_users_keys(s) == service_users_keys(old(s)).insert(handle@),
                result is Err ==> service_users_keys(s) == service_users_keys(old(s)),
        {
            // `String::as_str().is_empty()` chains through the vstd
            // assume_specifications for `String::as_str` (`res@ == s@`)
            // and `str::is_empty` (`res == (s@.len() == 0)`), giving
            // Verus the bridge from the exec branch to the `handle@.len() == 0`
            // spec clauses above.
            if handle.as_str().is_empty() {
                return Err(ServiceError::EmptyHandle);
            }
            proof_service_put_user(s, handle)
        }

        // -----------------------------------------------------------------
        // Stream 3 Phase 5 sub-PR R3 — `Service::follow` + `Service::unfollow`
        // discharge.
        //
        // Adds a second ghost-view axis on `&Service` — the set of currently
        // recorded directed follow edges, projected through `Service.st`. Same
        // disjoint-state argument as `store::follow_edges` vs `users_keys`:
        // edges live in a different field of the underlying `MemStore`, so
        // the two axes are independent and the put/delete shims frame
        // `service_users_keys` while updating `service_follow_edges`.
        //
        // What R3 verifies (the two `*_ensures` lemmas below):
        //
        //   - `follow_ensures(s, from, to)` — composes `dom::Follow::new` (F4
        //     discharged upstream in Phase 3) with `proof_service_put_follow`.
        //     F4 (`from@ == to@ ==> Err`) chains structurally from
        //     `Follow::new`'s ensures into the early-return branch. F9
        //     (unknown-user rejection) is **not** modeled here — it would
        //     require a cross-axis chain into `service_users_keys`, but the
        //     shim's exclusive-write semantics are along the `service_follow_edges`
        //     axis only. Production behavior matches Phase 4 sub-PR 3:
        //     `MemStore::put_follow` returns `UnknownUser` on missing
        //     endpoints; the service layer just propagates it via `?`.
        //   - `unfollow_ensures(s, from, to)` — F3 idempotency falls out
        //     structurally from `Set::remove`, which is idempotent. The
        //     production guard (two `has_user` checks → `UnknownUser`) is
        //     reflected in the ensures clauses: when both endpoints are
        //     absent from `service_users_keys(old(s))` the result is `Err`
        //     and `service_follow_edges` is framed; otherwise the edge is
        //     removed from `service_follow_edges` and the result is `Ok`.
        //
        // What stays trusted in R3:
        //
        //   - The cross-layer projection `service_follow_edges(s)` ==
        //     `store::follow_edges(s.st)` — same novel-infrastructure
        //     blocker R1/R2 called out for cross-crate `verus_proof` reuse.
        //     Worked around by re-defining the axis at the service layer;
        //     the two new shims pin their results back to it.
        //   - The `Follow` payload's `from`/`to` projection invariants are
        //     carried through `dom::Follow::new`'s already-discharged ensures
        //     clauses (Phase 3) — no new trust on the F4 axis.
        // -----------------------------------------------------------------

        // Ghost view of the set of currently-recorded directed follow edges
        // on `&Service`. Models `Set<(Seq<char>, Seq<char>)>` exactly mirroring
        // `store::follow_edges`. Body opaque for the same reason
        // `service_users_keys` is opaque (cross-crate `verus_proof` reuse
        // blocker).
        #[verifier::external_body]
        pub closed spec fn service_follow_edges(s: &Service) -> Set<(Seq<char>, Seq<char>)> {
            unimplemented!()
        }

        // Trusted shim around `Service::follow`'s store-side composition step
        // (`self.st.put_follow(f)`). Models the post-state along both ghost-view
        // axes:
        //
        //   - On Ok the targeted edge ends up in `service_follow_edges`
        //     (exactly the `Set::insert` axiom);
        //   - On Err the set is unchanged (framing);
        //   - `service_users_keys` is unaffected on either branch (the
        //     production write touches only the `follows` HashMap inside
        //     `MemStore`, never the `users` HashMap; the two axes project
        //     disjoint state).
        //
        // F3 idempotency falls out structurally because `Set::insert` is
        // idempotent: inserting an already-present element returns the same
        // set. The Err branch covers the production `UnknownUser` path
        // (`MemStore::put_follow` returns `StoreError::UnknownUser` when
        // either endpoint is missing); it is **not** further refined into
        // F9 here — see module commentary above.
        //
        // Signature is `&mut Service` for the same reason `proof_service_put_user`
        // is — sound because the production call holds the store's write lock.
        #[verifier::external_body]
        pub fn proof_service_put_follow(s: &mut Service, f: Follow) -> (result: Result<(), ServiceError>)
            ensures
                result is Ok ==>
                    service_follow_edges(s) == service_follow_edges(old(s)).insert((f.from@, f.to@)),
                result is Err ==> service_follow_edges(s) == service_follow_edges(old(s)),
                service_users_keys(s) == service_users_keys(old(s)),
        {
            match s.st.put_follow(f) {
                Ok(()) => Ok(()),
                Err(e) => Err(match e {
                    StoreError::UnknownUser => ServiceError::UnknownUser,
                    StoreError::DuplicateUser => ServiceError::DuplicateUser,
                }),
            }
        }

        // Trusted shim around `Service::unfollow`'s store-side composition
        // step (`self.st.delete_follow(from, to)`). Always succeeds at the
        // store layer (production `delete_follow` returns `()`); the service
        // wrapper is what produces `UnknownUser` on missing endpoints.
        // Models the post-state along both ghost-view axes:
        //
        //   - The targeted edge is removed from `service_follow_edges`
        //     (exactly the `Set::remove` axiom);
        //   - `service_users_keys` is unaffected (same disjoint-state
        //     argument).
        //
        // F3 idempotency falls out structurally because `Set::remove` is
        // idempotent: removing an absent element returns the same set.
        #[verifier::external_body]
        pub fn proof_service_delete_follow(s: &mut Service, from: &String, to: &String)
            ensures
                service_follow_edges(s) == service_follow_edges(old(s)).remove((from@, to@)),
                service_users_keys(s) == service_users_keys(old(s)),
        {
            s.st.delete_follow(from.as_str(), to.as_str());
        }

        // R3 discharge — verified wrapper for `Service::follow`. Composes
        // `dom::Follow::new` (F4 discharged in Phase 3) with
        // `proof_service_put_follow` (handle-axis store-side step).
        //
        // F4 (`from@ == to@ ==> Err`) chains structurally from `Follow::new`'s
        // ensures clauses through the `?` operator: when `from@ == to@`,
        // `Follow::new` returns `Err(DomainError::SelfFollow)` which the
        // `?` converts to `Err(ServiceError::SelfFollow)`. F3 (idempotent)
        // is structural: `proof_service_put_follow`'s post-state is
        // `old.insert((from@, to@))` and `Set::insert` is idempotent.
        //
        // Body mirrors the production `Service::follow` exactly: build the
        // `Follow` (which gates F4) and pass it to the store-side step.
        // `from.clone()` / `to.clone()` mirrors the production call site
        // (`from.to_string()`, `to.to_string()`) — `Follow::new` consumes
        // its arguments by value.
        pub fn follow_ensures(s: &mut Service, from: String, to: String) -> (result: Result<(), ServiceError>)
            ensures
                from@ == to@ ==> result is Err,
                result is Ok ==>
                    service_follow_edges(s) == service_follow_edges(old(s)).insert((from@, to@)),
                result is Err ==> service_follow_edges(s) == service_follow_edges(old(s)),
                service_users_keys(s) == service_users_keys(old(s)),
        {
            let f = match Follow::new(from, to) {
                Ok(f) => f,
                Err(e) => return Err(match e {
                    DomainError::SelfFollow => ServiceError::SelfFollow,
                }),
            };
            proof_service_put_follow(s, f)
        }

        // R3 discharge — verified wrapper for `Service::unfollow`. Encodes
        // the production guard: both endpoints must be registered in
        // `service_users_keys(old(s))`, otherwise `UnknownUser`. F3
        // idempotency falls out structurally from `Set::remove`.
        //
        // Body mirrors the production `Service::unfollow` exactly: two
        // `has_user`-shaped checks (via `proof_service_has_user`) gating
        // the store-side delete. The first check pins
        // `service_users_keys(s).contains(from@)`; the second pins
        // `service_users_keys(s).contains(to@)`. After both pass, the
        // `proof_service_delete_follow` shim performs the removal and
        // frames `service_users_keys`.
        pub fn unfollow_ensures(s: &mut Service, from: &String, to: &String) -> (result: Result<(), ServiceError>)
            ensures
                !service_users_keys(old(s)).contains(from@) ==> result is Err,
                !service_users_keys(old(s)).contains(to@)   ==> result is Err,
                (service_users_keys(old(s)).contains(from@)
                    && service_users_keys(old(s)).contains(to@))
                    ==> result is Ok,
                result is Ok ==>
                    service_follow_edges(s) == service_follow_edges(old(s)).remove((from@, to@)),
                result is Err ==> service_follow_edges(s) == service_follow_edges(old(s)),
                service_users_keys(s) == service_users_keys(old(s)),
        {
            if !proof_service_has_user(s, from) {
                return Err(ServiceError::UnknownUser);
            }
            if !proof_service_has_user(s, to) {
                return Err(ServiceError::UnknownUser);
            }
            proof_service_delete_follow(s, from, to);
            Ok(())
        }

        // -----------------------------------------------------------------
        // Stream 3 Phase 5 sub-PR R4 — `Service::home_timeline` discharge
        // (framing-only).
        //
        // Pure-delegation discharge of the read-only `Service::home_timeline`
        // wrapper. Mirrors the framing-only shape Phase 4 sub-PR 5+6 used at
        // the store layer: the underlying `store::home_timeline_ensures` was
        // discharged framing-only there (F1 visibility and F2 sort-order
        // remain trusted in `store::proof_home_timeline` because vstd
        // 0.0.0-2026-04-20-1748 ships no `vstd::vec` sort spec and no
        // verified mergesort), so the service-layer wrapper carries the same
        // framing-only contract one composition layer up.
        //
        // What R4 verifies (the `home_timeline_ensures` lemma below):
        //
        //   - Framing on both ghost-view axes via the `&Service`-not-`&mut`
        //     signature: the type system pins `service_users_keys` and
        //     `service_follow_edges` unchanged across the read. No ensures
        //     clauses are needed — framing is structural.
        //
        // What stays trusted in R4 (explicit non-goals):
        //
        //   - F1 (visibility): every returned tweet's author is in
        //     `{user} ∪ follow_set(user)` — propagated as a trust property
        //     of `store::proof_home_timeline`, not refined into a service-
        //     layer ensures clause here.
        //   - F2 (sort order): the returned `Vec<Tweet>` is sorted by
        //     `(created_at desc, id desc)` — same propagation; needs the
        //     missing `vstd::vec` sort spec to chain through.
        //   - The author_tweet_count axis on `&Service` (would let the
        //     wrapper express that the returned `Vec<Tweet>`'s length is
        //     bounded by the per-author counts) — out of scope; deferred
        //     until `Service::post_tweet` lands the F6 axis at the service
        //     layer.
        // -----------------------------------------------------------------

        // Trusted shim around `Service::home_timeline`'s store-side
        // composition step (`s.st.home_timeline(user.as_str(), limit)`).
        // Body calls the real production method on the concrete `MemStore`
        // field; framing on both ghost-view axes is structural via the
        // `&Service` signature (no `&mut`). No ensures clauses on the
        // returned `Vec<Tweet>` — F1 and F2 stay trusted at the
        // `store::proof_home_timeline` layer (vstd 0.0.0-2026-04-20-1748
        // ships no sort spec to chain through).
        #[verifier::external_body]
        pub fn proof_service_home_timeline(s: &Service, user: &String, limit: usize) -> (out: Vec<Tweet>)
        {
            s.st.home_timeline(user.as_str(), limit)
        }

        // R4 discharge — verified read-only wrapper for
        // `Service::home_timeline`. Pure delegation to
        // `proof_service_home_timeline`. Framing on both ghost-view axes
        // (`service_users_keys`, `service_follow_edges`) is structural via
        // the `&Service` signature. F1 and F2 remain trusted at the
        // `store::proof_home_timeline` layer — see module commentary above.
        pub fn home_timeline_ensures(s: &Service, user: &String, limit: usize) -> (result: Vec<Tweet>)
        {
            proof_service_home_timeline(s, user, limit)
        }

        // `Service::tick` composition (R1 status; unchanged by R2/R3/R4).
        // Verified `tick_ensures(s: &mut Service)` lemma is **not** shipped
        // here — `Service.clk: Arc<dyn Clock>` has no Verus model in
        // vstd 0.0.0-2026-04-20-1748, and `clock::verus_proof` is a private
        // module so cross-crate reuse of `clock::tick_ensures` requires
        // novel infrastructure. F7 itself remains discharged in
        // `crates/clock` (Phase 1b). When either blocker resolves the
        // lemma can land as a follow-up sub-PR.
        //
        // `post_tweet` discharge composes additional axes (clock for F7,
        // author_tweet_count for F6) and is scheduled for a later sub-PR.
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
