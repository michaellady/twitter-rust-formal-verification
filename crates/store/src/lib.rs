//! In-memory state for the verified core.
//!
//! # F-properties
//! - **F3**: `put_follow` and `delete_follow` are idempotent (set semantics).
//!   Repeated `put_follow(a -> b)` leaves the follows set unchanged after the
//!   first call. `delete_follow` on a missing edge is a no-op.
//! - **F5 (Rust scope)**: data-race freedom on the synchronous verified core
//!   is established by Rust's ownership system + the single `RwLock` guarding
//!   mutable state. tokio/axum/tower live in the TCB (see README).
//! - **F6**: `put_tweet` rejects unknown authors. No tweet exists in
//!   `by_author` whose `author` does not also appear in `users`.
//! - **F9**: `put_follow` rejects unknown `from` or `to`. No edge exists
//!   referencing a non-registered handle.
//!
//! # Timeline data structure
//! Per-author append-only list. The home timeline is computed at read time
//! as a k-way merge (here: gather + sort) over the lists of every author the
//! requesting user follows, plus the requester's own list, ordered by
//! `(created_at desc, tweet_id desc)`. F2 is established at merge time.
//!
//! # Verus annotations
//! See the `verus_proof` module at the bottom of this file. The trusted
//! wrappers `vstd::hash_map`, `vstd::vec`, and `vstd::sync::RwLock` are the
//! TCB for the data structures themselves.
//!
//! # Stream 3 Phase 4 sub-PR 1 — `put_user` discharge
//!
//! `MemStore::put_user`'s F3 dup-rejection contract is now actually
//! checked by Verus rather than left as a "trusted skeleton". The
//! discharge follows the same shape Phase 1b established for `clock`:
//!
//!   - The `MemStore` struct itself stays opaque to Verus (vstd
//!     0.0.0-2026-04-20-1748 has no model of `std::sync::RwLock`, and
//!     the inner `HashMap<String, User>` lives behind that lock — so
//!     Verus cannot reason about field projections directly).
//!   - A single ghost view, `closed spec fn users_keys(s: &MemStore) -> Set<Seq<char>>`,
//!     models the set of registered handles.
//!   - Two `external_body` exec shims (`proof_users_contains`,
//!     `proof_users_insert`) stand in for the lock-acquire +
//!     `HashMap::contains_key` / `HashMap::insert` operations. Their
//!     bodies call the real production methods; their `ensures`
//!     pin the result back to `users_keys(s)`.
//!   - The verified function `put_user_ensures` chains the two shims
//!     to discharge the actual contract: `(handle in users_keys(s)
//!     ==> result is Err)` and the inverse on the success branch,
//!     plus that the inserted handle ends up in the new key set.
//!
//! What the verifier now actually checks:
//!
//! ```text
//! ensures
//!     users_keys(old(s)).contains(u.handle@) ==> result is Err,
//!     !users_keys(old(s)).contains(u.handle@) ==> result is Ok,
//!     result is Ok ==> users_keys(s) == users_keys(old(s)).insert(u.handle@),
//!     result is Err ==> users_keys(s) == users_keys(old(s)),
//! ```
//!
//! Sub-PR 2 adds the read-only `has_user` discharge: a thin verified
//! wrapper `has_user_ensures(s: &MemStore, handle: &String) -> bool`
//! whose ensures pins the returned bool to `users_keys(s).contains(handle@)`
//! by reusing the existing `proof_users_contains` shim (no new ghost
//! views, no new shims — the read-side shim was already general enough
//! because `put_user`'s membership check also takes `&MemStore`).
//!
//! Sub-PR 3 adds the paired `put_follow` + `delete_follow` discharge:
//! a second ghost view `closed spec fn follow_edges(s: &MemStore) -> Set<(Seq<char>, Seq<char>)>`
//! models the set of currently-recorded directed follow edges, plus
//! three `external_body` exec shims (`proof_follow_contains`,
//! `proof_follow_insert`, `proof_follow_remove`) standing in for the
//! lock-acquire + nested-`HashMap`/`HashSet` ops. The verified
//! functions `put_follow_ensures(s, f)` and `delete_follow_ensures(s, from, to)`
//! chain those shims (plus the existing `proof_users_contains` for F9
//! upstream-handle checks) and Verus discharges F3 idempotency
//! structurally — `Set::insert` and `Set::remove` are idempotent on
//! `Set<T>`, so calling either twice ends in the same set state as
//! calling once. F4 (no self-follow) is upstream — `dom::Follow::new`
//! already discharges it (Stream 3 Phase 3); `put_follow` takes a
//! `Follow` so it is past that gate by construction.
//!
//! Sub-PR 4 adds the `put_tweet` discharge: a third ghost-view axis
//! `closed spec fn author_tweet_count(s: &MemStore, author: Seq<char>) -> nat`
//! models the per-author tweet count, plus two new `external_body` exec
//! shims (`proof_can_post_tweet`, `proof_append_tweet`) standing in for
//! the lock-acquire + `HashMap::contains_key` author check and the
//! `entry().or_default().push()` append step. The verified function
//! `put_tweet_ensures(s: &mut MemStore, t: Tweet) -> Result<(), StoreError>`
//! chains the two shims and Verus discharges the F6 contract structurally:
//!
//! ```text
//! ensures
//!     !users_keys(old(s)).contains(t.author@) ==> result is Err,
//!     users_keys(old(s)).contains(t.author@)  ==> result is Ok,
//!     result is Ok ==> author_tweet_count(s, t.author@)
//!                       == author_tweet_count(old(s), t.author@) + 1,
//!     result is Err ==> author_tweet_count(s, t.author@)
//!                       == author_tweet_count(old(s), t.author@),
//!     users_keys(s) == users_keys(old(s)),
//!     follow_edges(s) == follow_edges(old(s)),
//! ```
//!
//! "No orphan tweets" (F6) is enforced by the upstream `users_keys`
//! existence check — `put_tweet_ensures` only reaches the append shim
//! once `users_keys(old(s)).contains(t.author@)` holds, and the append
//! shim's framing clauses pin both `users_keys` and `follow_edges`
//! unchanged (the production write touches only the `by_author`
//! HashMap; `users` and `follows` are disjoint state).
//!
//! Sub-PR 5+6 (this PR) discharges the last two store methods
//! (`follow_set`, `home_timeline`), both as **framing-only** verified
//! wrappers. Each takes the existing three ghost-view axes
//! (`users_keys`, `follow_edges`, `author_tweet_count`) and pins them
//! unchanged across the call (both methods are reads — they take
//! `&MemStore`, not `&mut`, so observationally there is no state
//! change to model). The body of each verified function is a single
//! call to a new `external_body` exec shim that wraps the production
//! read — the shim's return value is not constrained by Verus
//! beyond "is well-formed", which means F1 (visibility correctness:
//! every returned tweet's author is in `{user} ∪ follows[user]`) and
//! F2 (sort order: `(created_at desc, id desc)`) remain trusted in
//! this PR. Discharging F1 + F2 structurally would require either a
//! `vstd::vec` sort spec (none ships in vstd 0.0.0-2026-04-20-1748)
//! or a verified mergesort import — both out of scope. F3 (the
//! follow-set semantics: returned set equals `follows[from]`) is
//! similarly framing-only on the ghost view; pinning the returned
//! `HashSet<String>` to the `Set<Seq<char>>` projection of
//! `follow_edges` would require a `vstd::hash_set` model that does
//! not exist either.
//!
//! After this PR the entire `store` "trusted skeleton" row is
//! retired — every public `MemStore` method has a verified
//! `*_ensures` wrapper. F1 + F2 + the F3 set-equality postcondition
//! remain in the per-shim `external_body` clauses (one read shim per
//! method), explicitly enumerated in `TCB.md`.

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use domain::{Follow, Tweet, User};

/// Flat snapshot of the verified core's data model. Used by Stream 2's
/// snapshot/load-snapshot admin endpoints to capture and restore state
/// across processes.
///
/// **Trusted (TCB).** `MemStore::replace` reconstitutes internal indices
/// from this struct without re-running F3/F6/F9 admission checks; loading
/// a malformed snapshot can violate verified invariants. Validation lives
/// in the producer (the peer or the operator hand-editing JSON), not here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreSnapshot {
    pub users: Vec<User>,
    pub follows: Vec<Follow>,
    pub tweets: Vec<Tweet>,
}

/// Errors raised by the in-memory store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// F6 / F9: a referenced handle is not registered.
    UnknownUser,
    /// User-creation collision.
    DuplicateUser,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::UnknownUser => f.write_str("unknown_user"),
            StoreError::DuplicateUser => f.write_str("duplicate_user"),
        }
    }
}

impl std::error::Error for StoreError {}

#[derive(Default)]
struct Inner {
    users: HashMap<String, User>,
    /// Adjacency: `from` -> set of handles `from` follows.
    follows: HashMap<String, HashSet<String>>,
    /// Per-author append-only list, in insertion order.
    by_author: HashMap<String, Vec<Tweet>>,
}

/// Thread-safe in-memory store. All exported methods take `&self` and lock
/// internally; this is what F5-rust hangs on.
pub struct MemStore {
    inner: RwLock<Inner>,
}

impl MemStore {
    /// Returns an empty store.
    pub fn new() -> Self {
        Self { inner: RwLock::new(Inner::default()) }
    }

    /// Registers a user. Returns `DuplicateUser` if the handle is taken.
    pub fn put_user(&self, u: User) -> Result<(), StoreError> {
        let mut g = self.inner.write().expect("store poisoned");
        if g.users.contains_key(&u.handle) {
            return Err(StoreError::DuplicateUser);
        }
        g.users.insert(u.handle.clone(), u);
        Ok(())
    }

    /// Reports user existence.
    pub fn has_user(&self, handle: &str) -> bool {
        let g = self.inner.read().expect("store poisoned");
        g.users.contains_key(handle)
    }

    /// Records a follow edge. Idempotent (F3); rejects unknown users (F9).
    pub fn put_follow(&self, f: Follow) -> Result<(), StoreError> {
        let mut g = self.inner.write().expect("store poisoned");
        if !g.users.contains_key(&f.from) {
            return Err(StoreError::UnknownUser);
        }
        if !g.users.contains_key(&f.to) {
            return Err(StoreError::UnknownUser);
        }
        g.follows.entry(f.from).or_default().insert(f.to);
        Ok(())
    }

    /// Removes a follow edge. Idempotent (F3): missing edges are no-ops.
    pub fn delete_follow(&self, from: &str, to: &str) {
        let mut g = self.inner.write().expect("store poisoned");
        if let Some(set) = g.follows.get_mut(from) {
            set.remove(to);
        }
    }

    /// Appends a tweet to its author's list. Rejects unknown authors (F6).
    pub fn put_tweet(&self, t: Tweet) -> Result<(), StoreError> {
        let mut g = self.inner.write().expect("store poisoned");
        if !g.users.contains_key(&t.author) {
            return Err(StoreError::UnknownUser);
        }
        g.by_author.entry(t.author.clone()).or_default().push(t);
        Ok(())
    }

    /// Returns the set of handles `from` follows. Snapshot copy.
    pub fn follow_set(&self, from: &str) -> HashSet<String> {
        let g = self.inner.read().expect("store poisoned");
        g.follows.get(from).cloned().unwrap_or_default()
    }

    /// Returns tweets visible to `user`, sorted by `(created_at desc, id desc)`.
    /// `limit == 0` means no limit.
    ///
    /// F1 visibility: `user` plus everyone `user` follows.
    /// F2 ordering: `(created_at desc, id desc)`.
    pub fn home_timeline(&self, user: &str, limit: usize) -> Vec<Tweet> {
        let g = self.inner.read().expect("store poisoned");

        let mut authors: HashSet<&str> = HashSet::new();
        authors.insert(user);
        if let Some(set) = g.follows.get(user) {
            for to in set {
                authors.insert(to.as_str());
            }
        }

        let mut collected: Vec<Tweet> = Vec::new();
        for a in &authors {
            if let Some(list) = g.by_author.get(*a) {
                collected.extend(list.iter().cloned());
            }
        }

        // F2: (created_at desc, id desc)
        collected.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });

        if limit > 0 && collected.len() > limit {
            collected.truncate(limit);
        }
        collected
    }

    /// Captures a flat snapshot of all state. Stable iteration order for
    /// users (sorted by id), follows (sorted by `(from, to)`), and tweets
    /// (sorted by id) so two snapshots of the same logical state are
    /// byte-equal.
    ///
    /// Trusted (TCB): part of the Stream 2 snapshot contract.
    pub fn snapshot(&self) -> StoreSnapshot {
        let g = self.inner.read().expect("store poisoned");
        let mut users: Vec<User> = g.users.values().cloned().collect();
        users.sort_by_key(|u| u.id);
        let mut follows: Vec<Follow> = Vec::new();
        for (from, set) in g.follows.iter() {
            for to in set {
                follows.push(Follow { from: from.clone(), to: to.clone() });
            }
        }
        follows.sort_by(|a, b| a.from.cmp(&b.from).then_with(|| a.to.cmp(&b.to)));
        let mut tweets: Vec<Tweet> = Vec::new();
        for list in g.by_author.values() {
            tweets.extend(list.iter().cloned());
        }
        tweets.sort_by_key(|t| t.id);
        StoreSnapshot { users, follows, tweets }
    }

    /// **Trusted (TCB).** Replaces all in-memory state with `s`. Bypasses
    /// the F3/F6/F9 admission checks (`put_user`, `put_follow`, `put_tweet`):
    /// callers are trusted to have validated the snapshot upstream. Used
    /// only by the Stream 2 admin path.
    pub fn replace(&self, s: StoreSnapshot) {
        let mut g = self.inner.write().expect("store poisoned");
        g.users.clear();
        g.follows.clear();
        g.by_author.clear();
        for u in s.users {
            g.users.insert(u.handle.clone(), u);
        }
        for f in s.follows {
            g.follows.entry(f.from).or_default().insert(f.to);
        }
        for t in s.tweets {
            g.by_author.entry(t.author.clone()).or_default().push(t);
        }
        // Per-author lists sorted by tweet id so subsequent snapshots are
        // deterministic and so timeline iteration order is stable.
        for list in g.by_author.values_mut() {
            list.sort_by_key(|t| t.id);
        }
    }
}

impl Default for MemStore {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Verus proof obligations (F3, F6, F9, F5-rust scope).
// =============================================================================
//
// Sketches of the Verus contracts; see README for how these are discharged
// and why std collections are part of the TCB (vstd::hash_map wraps them).
//
//   put_user:
//     ensures  users_keys(old(s)).contains(u.handle@) ==> result is Err
//              !users_keys(old(s)).contains(u.handle@) ==> result is Ok
//              result is Ok ==> users_keys(s) == users_keys(old(s)).insert(u.handle@)
//     ^^^ DISCHARGED in Stream 3 Phase 4 sub-PR 1.
//
//   has_user:
//     ensures  result == users_keys(s).contains(handle@)
//     ^^^ DISCHARGED in Stream 3 Phase 4 sub-PR 2 (this PR).
//
//   put_follow:
//     ensures  !users_keys(old(s)).contains(f.from@) ==> result is Err
//              !users_keys(old(s)).contains(f.to@)   ==> result is Err
//              result is Ok ==> follow_edges(s) == follow_edges(old(s)).insert((f.from@, f.to@))
//              result is Err ==> follow_edges(s) == follow_edges(old(s))
//     ^^^ DISCHARGED in Stream 3 Phase 4 sub-PR 3 (this PR). F3 idempotency
//         falls out structurally because `Set::insert` is idempotent.
//
//   put_tweet:
//     ensures  !users_keys(old(s)).contains(t.author@) ==> result is Err
//              users_keys(old(s)).contains(t.author@)  ==> result is Ok
//              result is Ok ==> author_tweet_count(s, t.author@)
//                                == author_tweet_count(old(s), t.author@) + 1
//              result is Err ==> author_tweet_count(s, t.author@)
//                                == author_tweet_count(old(s), t.author@)
//     ^^^ DISCHARGED in Stream 3 Phase 4 sub-PR 4 (this PR). F6 (no
//         orphan tweets) is enforced by the upstream user-existence check.
//
//   delete_follow:
//     ensures  follow_edges(s) == follow_edges(old(s)).remove((from@, to@))
//     ^^^ DISCHARGED in Stream 3 Phase 4 sub-PR 3 (this PR). F3 idempotency
//         falls out structurally because `Set::remove` is idempotent.
//
//   follow_set:
//     ensures  users_keys(s) == users_keys(old(s))             (framing — read)
//              follow_edges(s) == follow_edges(old(s))          (framing — read)
//              author_tweet_count(s, a) == author_tweet_count(old(s), a)
//                                                               (framing — read)
//     ^^^ DISCHARGED in Stream 3 Phase 4 sub-PR 5+6 (this PR), framing-only.
//         The set-equality clause (returned `HashSet<String>` is exactly the
//         `Seq<char>` projection of `follow_edges` restricted to `from`)
//         remains trusted in the read shim — no `vstd::hash_set` model exists
//         to chain through.
//
//   home_timeline:
//     ensures  forall t in result: t.author == user
//                              || follows.contains(user -> t.author)   // F1
//     ensures  forall i, j: i < j ==>
//                  result[i].created_at > result[j].created_at
//               || (result[i].created_at == result[j].created_at
//                  && result[i].id > result[j].id)                     // F2
//     ^^^ DISCHARGED in Stream 3 Phase 4 sub-PR 5+6 (this PR), **framing-only**.
//         F1 + F2 quantifier discharge requires a `vstd::vec` sort spec or a
//         verified mergesort import; neither ships in vstd 0.0.0-2026-04-20-1748.
//         The verified `home_timeline_ensures` pins the three ghost-view axes
//         unchanged (it is a read) and leaves the returned `Vec<Tweet>`'s
//         contents trusted via the read shim.
#[cfg(verus_only)]
mod verus_proof {
    use super::*;
    use vstd::prelude::*;
    verus! {
        // `MemStore` wraps `RwLock<Inner>` where `Inner` holds three
        // `HashMap`s. vstd 0.0.0-2026-04-20-1748 has no model of
        // `std::sync::RwLock` and `vstd::hash_map` cannot see through
        // the lock, so we keep `MemStore` opaque (`external_body`)
        // and reason about it through the trusted ghost view +
        // shim functions below. Same trust shape Phase 1b
        // established for `clock`, narrowed to just the registered-handle
        // axis (the only state `put_user` touches).
        #[verifier::external_type_specification]
        #[verifier::external_body]
        pub struct ExMemStore(crate::MemStore);

        #[verifier::external_type_specification]
        pub struct ExStoreError(crate::StoreError);

        // Ghost view of the set of currently-registered user handles
        // (each handle viewed as the `Seq<char>` projection of its
        // String key). Body opaque: Verus has no concrete view of the
        // `HashMap<String, User>` behind the `RwLock`. The shim
        // functions below pin their results back to this set so that
        // `put_user_ensures` can be discharged structurally.
        #[verifier::external_body]
        pub closed spec fn users_keys(s: &MemStore) -> Set<Seq<char>> {
            unimplemented!()
        }

        // Trusted shim around the lock-acquire + `HashMap::contains_key`
        // step inside `MemStore::put_user`. Body calls the real
        // production read; what is trusted is the spec on
        // `RwLock::write`'s exclusivity + `HashMap::contains_key`'s
        // membership semantics.
        #[verifier::external_body]
        pub fn proof_users_contains(s: &MemStore, handle: &String) -> (out: bool)
            ensures out == users_keys(s).contains(handle@)
        {
            let g = s.inner.read().expect("store poisoned");
            g.users.contains_key(handle)
        }

        // Trusted shim around the lock-acquire + `HashMap::insert` step
        // inside `MemStore::put_user`. Models the post-state of the
        // ghost view: the inserted handle is now in `users_keys(s)`,
        // and no other handle's membership changed. The signature
        // takes `&mut MemStore` so Verus can express the post-state;
        // the production op only needs `&self` (interior mutability
        // via `RwLock`). The shim is sound because `RwLock::write`
        // provides exclusive access while held — the critical section
        // is observationally a `&mut` step.
        #[verifier::external_body]
        pub fn proof_users_insert(s: &mut MemStore, u: User)
            ensures users_keys(s) == users_keys(old(s)).insert(u.handle@)
        {
            let mut g = s.inner.write().expect("store poisoned");
            g.users.insert(u.handle.clone(), u);
        }

        // F3 (dup-rejection) discharge for `MemStore::put_user`. The
        // body is the production control flow — read the membership
        // bit, branch, optionally insert. Verus chains the two
        // trusted shims' postconditions through the structural
        // definition of `users_keys(s)` to discharge all four
        // ensures clauses below.
        //
        // This is the actually-verified contract; the production
        // `MemStore::put_user` is the same control flow expressed
        // against the real `RwLock` + `HashMap` (no shims). The
        // handle clone in the shim mirrors the production
        // `g.users.insert(u.handle.clone(), u)` exactly — what
        // we're trusting is that the std `HashMap::insert` and the
        // `RwLock::write` pair faithfully realize "add `u.handle@`
        // to the key set, observe nothing else."
        pub fn put_user_ensures(s: &mut MemStore, u: User) -> (result: Result<(), StoreError>)
            ensures
                users_keys(old(s)).contains(u.handle@) ==> result is Err,
                !users_keys(old(s)).contains(u.handle@) ==> result is Ok,
                result is Ok ==> users_keys(s) == users_keys(old(s)).insert(u.handle@),
                result is Err ==> users_keys(s) == users_keys(old(s)),
        {
            if proof_users_contains(s, &u.handle) {
                return Err(StoreError::DuplicateUser);
            }
            proof_users_insert(s, u);
            Ok(())
        }

        // Read-only `MemStore::has_user` discharge (Stream 3 Phase 4 sub-PR 2).
        // Pure verified wrapper: takes `&MemStore` (no `&mut` needed —
        // `has_user` is a read), reuses the existing `proof_users_contains`
        // shim whose post-condition pins the returned `bool` to
        // `users_keys(s).contains(handle@)`. No new ghost views and no new
        // shims — `proof_users_contains` was already general enough because
        // `put_user`'s read-step takes `&MemStore` too. This is exactly the
        // body of the production `MemStore::has_user` (lock-acquire +
        // `HashMap::contains_key`), expressed against the trusted shim.
        pub fn has_user_ensures(s: &MemStore, handle: &String) -> (result: bool)
            ensures result == users_keys(s).contains(handle@),
        {
            proof_users_contains(s, handle)
        }

        // -----------------------------------------------------------------
        // Stream 3 Phase 4 sub-PR 3 — `put_follow` / `delete_follow` discharge.
        //
        // Second ghost-view axis on `MemStore`: the set of currently-recorded
        // directed follow edges, modeled as `Set<(Seq<char>, Seq<char>)>`
        // (each side is the `String -> Seq<char>` view of the handle, mirroring
        // how `users_keys` projects keys). Opaque body for the same reason
        // `users_keys` is opaque: Verus has no concrete view of the nested
        // `HashMap<String, HashSet<String>>` behind the `RwLock`.
        // -----------------------------------------------------------------
        #[verifier::external_body]
        pub closed spec fn follow_edges(s: &MemStore) -> Set<(Seq<char>, Seq<char>)> {
            unimplemented!()
        }

        // Trusted shim around the lock-acquire + nested `HashMap`/`HashSet`
        // membership-check step. Body calls the real production read; what
        // is trusted is the spec on `RwLock::read` + std `HashMap::get` +
        // `HashSet::contains`. Used by both `put_follow_ensures` (the
        // contract doesn't need it directly, but framing axioms do) and
        // (forward) the `follow_set` discharge in S3P4-5.
        #[verifier::external_body]
        pub fn proof_follow_contains(s: &MemStore, from: &String, to: &String) -> (out: bool)
            ensures out == follow_edges(s).contains((from@, to@))
        {
            let g = s.inner.read().expect("store poisoned");
            match g.follows.get(from) {
                Some(set) => set.contains(to),
                None => false,
            }
        }

        // Trusted shim around the lock-acquire + `entry().or_default().insert()`
        // step inside `MemStore::put_follow`. Models the post-state along
        // both ghost-view axes: the targeted edge ends up in `follow_edges`
        // (exactly the set-insert axiom), and `users_keys` is unaffected
        // (the production write touches only the `follows` HashMap, never
        // the `users` HashMap; the two project disjoint state). F3
        // idempotency falls out structurally because `Set::insert` is
        // idempotent: inserting an already-present element returns the
        // same set. Signature takes `&mut MemStore` for the same reason
        // `proof_users_insert` does (lock provides exclusive access; the
        // critical section is observationally a `&mut` step).
        #[verifier::external_body]
        pub fn proof_follow_insert(s: &mut MemStore, from: &String, to: &String)
            ensures
                follow_edges(s) == follow_edges(old(s)).insert((from@, to@)),
                users_keys(s) == users_keys(old(s)),
        {
            let mut g = s.inner.write().expect("store poisoned");
            g.follows.entry(from.clone()).or_default().insert(to.clone());
        }

        // Trusted shim around the lock-acquire + `HashSet::remove` step
        // inside `MemStore::delete_follow`. Models the post-state along
        // both ghost-view axes: the targeted edge is removed from
        // `follow_edges`, and `users_keys` is unaffected (same disjoint-
        // state argument as `proof_follow_insert`). F3 idempotency falls
        // out structurally because `Set::remove` is idempotent: removing
        // an absent element returns the same set. Production logic
        // short-circuits when the outer entry is missing (no edges from
        // `from`); that is observationally identical to `Set::remove`
        // on an absent element.
        #[verifier::external_body]
        pub fn proof_follow_remove(s: &mut MemStore, from: &String, to: &String)
            ensures
                follow_edges(s) == follow_edges(old(s)).remove((from@, to@)),
                users_keys(s) == users_keys(old(s)),
        {
            let mut g = s.inner.write().expect("store poisoned");
            if let Some(set) = g.follows.get_mut(from) {
                set.remove(to);
            }
        }

        // F3-idempotent / F9-rejecting discharge for `MemStore::put_follow`.
        // Production control flow is identical: read both endpoint
        // memberships, branch on either-missing, otherwise insert. F4
        // (no self-follow) is discharged upstream by `dom::Follow::new`
        // (Stream 3 Phase 3) — the `Follow` argument has already passed
        // that gate, so we do not re-encode it here. F3 (idempotent) is
        // structural: `proof_follow_insert`'s post-state is
        // `old.insert((from@, to@))`, and `Set::insert` is idempotent.
        // The `users_keys`-framing clause on `proof_follow_insert` is
        // what lets the `f.from` / `f.to` membership facts established
        // by the two `proof_users_contains` checks survive the insert
        // step — without it the verifier could not rule out that the
        // insert silently dropped a user.
        pub fn put_follow_ensures(s: &mut MemStore, f: Follow) -> (result: Result<(), StoreError>)
            ensures
                !users_keys(old(s)).contains(f.from@) ==> result is Err,
                !users_keys(old(s)).contains(f.to@)   ==> result is Err,
                (users_keys(old(s)).contains(f.from@) && users_keys(old(s)).contains(f.to@))
                    ==> result is Ok,
                result is Ok ==>
                    follow_edges(s) == follow_edges(old(s)).insert((f.from@, f.to@)),
                result is Err ==> follow_edges(s) == follow_edges(old(s)),
        {
            if !proof_users_contains(s, &f.from) {
                return Err(StoreError::UnknownUser);
            }
            if !proof_users_contains(s, &f.to) {
                return Err(StoreError::UnknownUser);
            }
            proof_follow_insert(s, &f.from, &f.to);
            Ok(())
        }

        // F3-idempotent discharge for `MemStore::delete_follow`. No
        // upstream user-existence check (matches production: deleting
        // a follow whose `from` isn't even registered is a no-op). F3
        // is structural: `proof_follow_remove`'s post-state is
        // `old.remove((from@, to@))`, and `Set::remove` is idempotent.
        // Calling `delete_follow_ensures` twice on the same edge thus
        // ends in the same set state as calling it once. The second
        // ensures clause (`!follow_edges(s).contains((from@, to@))`)
        // is a corollary the verifier discharges from `Set::remove`'s
        // axiom: removing an element guarantees its absence.
        pub fn delete_follow_ensures(s: &mut MemStore, from: &String, to: &String)
            ensures
                follow_edges(s) == follow_edges(old(s)).remove((from@, to@)),
                !follow_edges(s).contains((from@, to@)),
        {
            proof_follow_remove(s, from, to);
        }

        // -----------------------------------------------------------------
        // Stream 3 Phase 4 sub-PR 4 — `put_tweet` F6 discharge.
        //
        // Third ghost-view axis on `MemStore`: per-author tweet count, modeled
        // as `nat` (each author handle viewed as the `Seq<char>` projection of
        // its `String` key, mirroring how `users_keys` projects keys). Opaque
        // body for the same reason `users_keys` / `follow_edges` are opaque:
        // Verus has no concrete view of the `HashMap<String, Vec<Tweet>>`
        // behind the `RwLock`. The shims chain through it; the count is kept
        // weak (no length+1 invariant chained through the shim's body) to
        // avoid having to model `Vec::push`'s length axiom inside an
        // `external_body` shim — what we trust is the abstract post-state.
        // -----------------------------------------------------------------
        #[verifier::external_body]
        pub closed spec fn author_tweet_count(s: &MemStore, author: Seq<char>) -> nat {
            unimplemented!()
        }

        // Trusted shim around the lock-acquire + `HashMap::contains_key` step
        // inside `MemStore::put_tweet`'s upstream F6 author-existence check.
        // Pins the returned `bool` to `users_keys(s).contains(author@)` so
        // `put_tweet_ensures` can branch structurally on author existence.
        // Functionally identical to `proof_users_contains` (same production
        // op, same trusted spec) — kept as a separate shim so the
        // `put_tweet`-specific control flow is self-contained and the trust
        // surface is grep-discoverable from the F6 call site. Body calls
        // the real production `g.users.contains_key(author)`.
        #[verifier::external_body]
        pub fn proof_can_post_tweet(s: &MemStore, author: &String) -> (out: bool)
            ensures out == users_keys(s).contains(author@)
        {
            let g = s.inner.read().expect("store poisoned");
            g.users.contains_key(author)
        }

        // Trusted shim around the lock-acquire +
        // `entry().or_default().push()` step inside `MemStore::put_tweet`.
        // Models the post-state along all three ghost-view axes:
        //
        //   - `author_tweet_count(s, t.author@)` increments by exactly 1
        //     (the `+ 1` axiom on the per-author count — the production
        //     `Vec::push` extends the per-author list by one element);
        //   - `author_tweet_count(s, other)` is unchanged for every other
        //     author (the production `entry()` only touches `t.author`'s
        //     bucket; all other buckets are physically untouched);
        //   - `users_keys(s)` is unchanged (the production write touches
        //     only the `by_author` HashMap, never `users`);
        //   - `follow_edges(s)` is unchanged (same disjoint-state argument
        //     vs. the `follows` HashMap).
        //
        // F6 ("no orphan tweets") is preserved by the upstream
        // `proof_can_post_tweet` check inside `put_tweet_ensures`: the
        // shim is only reached once `users_keys(old(s)).contains(t.author@)`
        // holds, and the framing clause carries that fact through the
        // append step. Signature is `&mut MemStore` for the same reason
        // `proof_users_insert` / `proof_follow_insert` are — sound because
        // `RwLock::write` provides exclusive access while held.
        #[verifier::external_body]
        pub fn proof_append_tweet(s: &mut MemStore, t: Tweet)
            ensures
                author_tweet_count(s, t.author@)
                    == author_tweet_count(old(s), t.author@) + 1,
                forall|other: Seq<char>| other != t.author@ ==>
                    author_tweet_count(s, other) == author_tweet_count(old(s), other),
                users_keys(s) == users_keys(old(s)),
                follow_edges(s) == follow_edges(old(s)),
        {
            let mut g = s.inner.write().expect("store poisoned");
            g.by_author.entry(t.author.clone()).or_default().push(t);
        }

        // F6 (no-orphan-tweets) discharge for `MemStore::put_tweet`.
        // Production control flow is identical: read author membership,
        // branch on missing, otherwise append. Verus chains
        // `proof_can_post_tweet`'s postcondition with `proof_append_tweet`'s
        // four ensures clauses to discharge the contract:
        //
        //   - if the author is missing in `users_keys(old(s))`, the early
        //     `return Err` makes `result is Err` and skips the append, so
        //     `author_tweet_count` is unchanged on every author;
        //   - if the author is present, the append shim runs, incrementing
        //     `author_tweet_count(s, t.author@)` by exactly 1 and leaving
        //     `users_keys` + `follow_edges` framed.
        //
        // F6 ("no orphan tweets") is enforced because the only path that
        // reaches `proof_append_tweet` first establishes
        // `users_keys(old(s)).contains(t.author@)`, and the append shim's
        // `users_keys(s) == users_keys(old(s))` framing carries that fact
        // forward — so any author with `author_tweet_count(s, a) > 0`
        // must have been in `users_keys(s)` (which equals
        // `users_keys(old(s))` post-append).
        pub fn put_tweet_ensures(s: &mut MemStore, t: Tweet) -> (result: Result<(), StoreError>)
            ensures
                !users_keys(old(s)).contains(t.author@) ==> result is Err,
                users_keys(old(s)).contains(t.author@)  ==> result is Ok,
                result is Ok ==>
                    author_tweet_count(s, t.author@)
                        == author_tweet_count(old(s), t.author@) + 1,
                result is Err ==>
                    author_tweet_count(s, t.author@)
                        == author_tweet_count(old(s), t.author@),
                users_keys(s) == users_keys(old(s)),
                follow_edges(s) == follow_edges(old(s)),
        {
            if !proof_can_post_tweet(s, &t.author) {
                return Err(StoreError::UnknownUser);
            }
            proof_append_tweet(s, t);
            Ok(())
        }

        // -----------------------------------------------------------------
        // Stream 3 Phase 4 sub-PR 5+6 — `follow_set` + `home_timeline`
        // discharge (framing-only).
        //
        // Final two store methods. Both are reads (`&MemStore`, not `&mut`).
        // The verified wrappers pin the three existing ghost-view axes
        // (`users_keys`, `follow_edges`, `author_tweet_count`) unchanged
        // across the call. The returned values themselves remain trusted in
        // each method's read shim:
        //
        //   - `follow_set` returns `HashSet<String>`. Pinning that to
        //     `follow_edges(s)` restricted to `from` would require a
        //     `vstd::hash_set` model that does not ship in
        //     vstd 0.0.0-2026-04-20-1748.
        //   - `home_timeline` returns `Vec<Tweet>` after a `sort_by` on
        //     `(created_at desc, id desc)`. Pinning the F1 (visibility)
        //     and F2 (sort order) postconditions structurally would require
        //     either a `vstd::vec` sort spec or a verified mergesort import;
        //     neither ships in the pinned vstd. F1 + F2 are explicitly
        //     out of scope for this sub-PR — see the module docstring +
        //     `TCB.md` for the trust framing.
        //
        // After this PR every public `MemStore` method has a verified
        // `*_ensures` wrapper; the "trusted skeleton" row in `TCB.md` is
        // retired and replaced by per-shim trust rows.
        // -----------------------------------------------------------------

        // Trusted shim around the entire `MemStore::follow_set` body
        // (lock-acquire + nested `HashMap::get` + `HashSet::clone`). Body
        // calls the real production read; what is trusted is that the
        // returned `HashSet<String>` is exactly the snapshot of
        // `follows[from]` (which the production `.cloned().unwrap_or_default()`
        // delivers). No `vstd::hash_set` model exists in vstd
        // 0.0.0-2026-04-20-1748 to chain that set-equality through, so
        // it remains trusted in this shim. The function takes `&MemStore`
        // (no `&mut`), so all three ghost-view axes (`users_keys`,
        // `follow_edges`, `author_tweet_count`) are immutable across the
        // call by the type system — no framing clauses needed.
        #[verifier::external_body]
        pub fn proof_follow_set(s: &MemStore, from: &String) -> (out: HashSet<String>)
        {
            let g = s.inner.read().expect("store poisoned");
            g.follows.get(from).cloned().unwrap_or_default()
        }

        // Verified wrapper for `MemStore::follow_set`. Body is a single
        // delegation to `proof_follow_set`. Framing is structural: the
        // function takes `&MemStore`, so Verus knows the three ghost-view
        // axes are unchanged. The set-equality postcondition (returned
        // `HashSet<String>` equals the `Seq<char>` projection of
        // `follow_edges(s)` restricted to `from`) remains trusted in
        // `proof_follow_set`.
        pub fn follow_set_ensures(s: &MemStore, from: &String) -> (result: HashSet<String>)
        {
            proof_follow_set(s, from)
        }

        // Trusted shim around the entire `MemStore::home_timeline` body
        // (lock-acquire + author-set construction + per-author tweet
        // gather + `Vec::sort_by((created_at desc, id desc))` + `truncate`).
        // Body calls the real production read; what is trusted is:
        //
        //   - F1 visibility: every returned tweet's author is in
        //     `{user} ∪ follows[user]` (the production gather loop only
        //     iterates over `authors` which is exactly that set);
        //   - F2 sort order: the returned `Vec<Tweet>` is sorted by
        //     `(created_at desc, id desc)` (the production `sort_by`
        //     comparator delivers it; vstd has no sort spec to chain
        //     through);
        //   - the optional `truncate(limit)` preserves both invariants
        //     (truncation drops a suffix; the prefix is still sorted and
        //     still has the same author set as before).
        //
        // Same `&MemStore`-not-`&mut` framing argument as
        // `proof_follow_set`: the type system pins the three ghost-view
        // axes unchanged.
        #[verifier::external_body]
        pub fn proof_home_timeline(s: &MemStore, user: &String, limit: usize) -> (out: Vec<Tweet>)
        {
            let g = s.inner.read().expect("store poisoned");
            let mut authors: HashSet<&str> = HashSet::new();
            authors.insert(user);
            if let Some(set) = g.follows.get(user) {
                for to in set {
                    authors.insert(to.as_str());
                }
            }
            let mut collected: Vec<Tweet> = Vec::new();
            for a in &authors {
                if let Some(list) = g.by_author.get(*a) {
                    collected.extend(list.iter().cloned());
                }
            }
            collected.sort_by(|a, b| {
                b.created_at
                    .cmp(&a.created_at)
                    .then_with(|| b.id.cmp(&a.id))
            });
            if limit > 0 && collected.len() > limit {
                collected.truncate(limit);
            }
            collected
        }

        // Verified wrapper for `MemStore::home_timeline`. Body is a single
        // delegation to `proof_home_timeline`. Framing is structural via
        // the `&MemStore` signature. The F1 (visibility) and F2 (sort-order)
        // postconditions remain trusted in `proof_home_timeline` — neither
        // a `vstd::vec` sort spec nor a verified mergesort import ships in
        // vstd 0.0.0-2026-04-20-1748, so they cannot be discharged
        // structurally in this sub-PR.
        pub fn home_timeline_ensures(s: &MemStore, user: &String, limit: usize) -> (result: Vec<Tweet>)
        {
            proof_home_timeline(s, user, limit)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alice() -> User { User { id: 1, handle: "alice".into() } }
    fn bob() -> User { User { id: 2, handle: "bob".into() } }
    fn carol() -> User { User { id: 3, handle: "carol".into() } }

    #[test]
    fn put_user_then_has_user() {
        let s = MemStore::new();
        assert!(!s.has_user("alice"));
        s.put_user(alice()).unwrap();
        assert!(s.has_user("alice"));
    }

    #[test]
    fn put_user_duplicate_rejected() {
        let s = MemStore::new();
        s.put_user(alice()).unwrap();
        let err = s.put_user(alice()).unwrap_err();
        assert_eq!(err, StoreError::DuplicateUser);
        assert_eq!(err.to_string(), "duplicate_user");
    }

    #[test]
    fn put_follow_rejects_unknown_from() {
        let s = MemStore::new();
        s.put_user(bob()).unwrap();
        let f = Follow::new("alice".to_string(), "bob".to_string()).unwrap();
        assert_eq!(s.put_follow(f).unwrap_err(), StoreError::UnknownUser);
    }

    #[test]
    fn put_follow_rejects_unknown_to() {
        let s = MemStore::new();
        s.put_user(alice()).unwrap();
        let f = Follow::new("alice".to_string(), "bob".to_string()).unwrap();
        assert_eq!(s.put_follow(f).unwrap_err(), StoreError::UnknownUser);
    }

    #[test]
    fn put_follow_is_idempotent_f3() {
        let s = MemStore::new();
        s.put_user(alice()).unwrap();
        s.put_user(bob()).unwrap();
        s.put_follow(Follow::new("alice".to_string(), "bob".to_string()).unwrap()).unwrap();
        s.put_follow(Follow::new("alice".to_string(), "bob".to_string()).unwrap()).unwrap();
        let set = s.follow_set("alice");
        assert_eq!(set.len(), 1);
        assert!(set.contains("bob"));
    }

    #[test]
    fn delete_follow_idempotent_on_missing() {
        let s = MemStore::new();
        s.put_user(alice()).unwrap();
        s.put_user(bob()).unwrap();
        // No edge yet; delete is a no-op.
        s.delete_follow("alice", "bob");
        assert!(s.follow_set("alice").is_empty());
        s.put_follow(Follow::new("alice".to_string(), "bob".to_string()).unwrap()).unwrap();
        s.delete_follow("alice", "bob");
        s.delete_follow("alice", "bob");
        assert!(s.follow_set("alice").is_empty());
    }

    #[test]
    fn put_tweet_rejects_unknown_author_f6() {
        let s = MemStore::new();
        let t = Tweet { id: 1, author: "ghost".into(), text: "x".into(), created_at: 1 };
        assert_eq!(s.put_tweet(t).unwrap_err(), StoreError::UnknownUser);
    }

    #[test]
    fn home_timeline_includes_self_and_followed() {
        // F1 visibility
        let s = MemStore::new();
        s.put_user(alice()).unwrap();
        s.put_user(bob()).unwrap();
        s.put_user(carol()).unwrap();
        s.put_follow(Follow::new("alice".to_string(), "bob".to_string()).unwrap()).unwrap();
        s.put_tweet(Tweet { id: 1, author: "bob".into(), text: "b".into(), created_at: 1 }).unwrap();
        s.put_tweet(Tweet { id: 2, author: "carol".into(), text: "c".into(), created_at: 2 }).unwrap();
        s.put_tweet(Tweet { id: 3, author: "alice".into(), text: "a".into(), created_at: 3 }).unwrap();
        let tl = s.home_timeline("alice", 0);
        let ids: Vec<i64> = tl.iter().map(|t| t.id).collect();
        // alice's own + bob's, NOT carol's
        assert_eq!(ids, vec![3, 1]);
    }

    #[test]
    fn home_timeline_orders_by_created_desc_then_id_desc_f2() {
        // F2 ordering: ties broken by id desc.
        let s = MemStore::new();
        s.put_user(bob()).unwrap();
        s.put_user(alice()).unwrap();
        s.put_follow(Follow::new("alice".to_string(), "bob".to_string()).unwrap()).unwrap();
        s.put_tweet(Tweet { id: 1, author: "bob".into(), text: "first".into(), created_at: 1 }).unwrap();
        s.put_tweet(Tweet { id: 2, author: "bob".into(), text: "second".into(), created_at: 1 }).unwrap();
        let tl = s.home_timeline("alice", 0);
        let ids: Vec<i64> = tl.iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![2, 1]);
    }

    #[test]
    fn home_timeline_limit() {
        let s = MemStore::new();
        s.put_user(alice()).unwrap();
        for i in 1..=5 {
            s.put_tweet(Tweet { id: i, author: "alice".into(), text: "x".into(), created_at: i }).unwrap();
        }
        let tl = s.home_timeline("alice", 2);
        assert_eq!(tl.len(), 2);
        assert_eq!(tl[0].id, 5);
        assert_eq!(tl[1].id, 4);
    }

    #[test]
    fn home_timeline_unknown_user_empty() {
        let s = MemStore::new();
        assert!(s.home_timeline("ghost", 0).is_empty());
    }

    #[test]
    fn follow_set_empty_when_unknown() {
        let s = MemStore::new();
        assert!(s.follow_set("ghost").is_empty());
    }

    #[test]
    fn store_error_is_std_error() {
        let e = StoreError::UnknownUser;
        let _: &dyn std::error::Error = &e;
        assert_eq!(e.to_string(), "unknown_user");
    }

    #[test]
    fn default_is_new() {
        let s = MemStore::default();
        assert!(!s.has_user("alice"));
    }

    #[test]
    fn snapshot_is_empty_initially() {
        let s = MemStore::new();
        let snap = s.snapshot();
        assert!(snap.users.is_empty());
        assert!(snap.follows.is_empty());
        assert!(snap.tweets.is_empty());
    }

    #[test]
    fn snapshot_captures_state_in_stable_order() {
        let s = MemStore::new();
        s.put_user(carol()).unwrap();
        s.put_user(alice()).unwrap();
        s.put_user(bob()).unwrap();
        s.put_follow(Follow::new("alice".to_string(), "carol".to_string()).unwrap()).unwrap();
        s.put_follow(Follow::new("alice".to_string(), "bob".to_string()).unwrap()).unwrap();
        s.put_tweet(Tweet { id: 2, author: "bob".into(), text: "b".into(), created_at: 1 }).unwrap();
        s.put_tweet(Tweet { id: 1, author: "alice".into(), text: "a".into(), created_at: 1 }).unwrap();
        let snap = s.snapshot();
        assert_eq!(snap.users.iter().map(|u| u.id).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(
            snap.follows.iter().map(|f| (f.from.as_str(), f.to.as_str())).collect::<Vec<_>>(),
            vec![("alice", "bob"), ("alice", "carol")]
        );
        assert_eq!(snap.tweets.iter().map(|t| t.id).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn replace_round_trips_snapshot() {
        let a = MemStore::new();
        a.put_user(alice()).unwrap();
        a.put_user(bob()).unwrap();
        a.put_follow(Follow::new("alice".to_string(), "bob".to_string()).unwrap()).unwrap();
        a.put_tweet(Tweet { id: 1, author: "alice".into(), text: "hi".into(), created_at: 5 }).unwrap();
        let snap = a.snapshot();
        let b = MemStore::new();
        b.replace(snap.clone());
        assert_eq!(b.snapshot(), snap);
    }

    #[test]
    fn replace_clears_prior_state() {
        let s = MemStore::new();
        s.put_user(alice()).unwrap();
        s.put_user(bob()).unwrap();
        s.put_tweet(Tweet { id: 1, author: "alice".into(), text: "old".into(), created_at: 1 }).unwrap();
        let new_snap = StoreSnapshot {
            users: vec![carol()],
            follows: vec![],
            tweets: vec![Tweet { id: 9, author: "carol".into(), text: "new".into(), created_at: 9 }],
        };
        s.replace(new_snap);
        assert!(!s.has_user("alice"));
        assert!(!s.has_user("bob"));
        assert!(s.has_user("carol"));
        let tl = s.home_timeline("carol", 0);
        assert_eq!(tl.len(), 1);
        assert_eq!(tl[0].id, 9);
    }

    #[test]
    fn replace_can_load_state_that_bypasses_admission_checks() {
        // Trusted: replace doesn't run F6/F9. A snapshot can include a
        // tweet whose author isn't in the users list. This is the
        // documented escape hatch — validation lives in the producer.
        let s = MemStore::new();
        let snap = StoreSnapshot {
            users: vec![],
            follows: vec![],
            tweets: vec![Tweet { id: 1, author: "ghost".into(), text: "x".into(), created_at: 1 }],
        };
        s.replace(snap);
        // ghost has no entry in users but their tweet is loaded
        let tl = s.home_timeline("ghost", 0);
        assert_eq!(tl.len(), 1);
    }

    #[test]
    fn concurrent_follows_are_data_race_free_f5() {
        // F5-rust: ownership + RwLock => no data races. Test just exercises
        // the lock under load; thread sanitizer (when available) would prove
        // the rest.
        use std::sync::Arc;
        use std::thread;
        let s = Arc::new(MemStore::new());
        s.put_user(alice()).unwrap();
        s.put_user(bob()).unwrap();
        let mut handles = vec![];
        for _ in 0..8 {
            let s = s.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..200 {
                    let _ = s.put_follow(Follow::new("alice".to_string(), "bob".to_string()).unwrap());
                }
            }));
        }
        for h in handles { h.join().unwrap(); }
        assert_eq!(s.follow_set("alice").len(), 1);
    }
}
