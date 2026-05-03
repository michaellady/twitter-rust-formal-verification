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
//! The other five store methods (`put_follow`, `delete_follow`,
//! `put_tweet`, `follow_set`, `home_timeline`) remain in the trusted
//! skeleton and are scheduled for the follow-up sub-PRs (S3P4-3..7).

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
//     requires users.contains(f.from) && users.contains(f.to) && f.from != f.to
//     ensures  follows.contains(f.from -> f.to)             // F3 idempotent set
//
//   put_tweet:
//     requires users.contains(t.author)                      // F6
//     ensures  by_author[t.author].last() == t
//
//   delete_follow:
//     ensures  !follows.contains(from -> to)                // F3 idempotent
//
//   home_timeline:
//     ensures  forall t in result: t.author == user
//                              || follows.contains(user -> t.author)   // F1
//     ensures  forall i, j: i < j ==>
//                  result[i].created_at > result[j].created_at
//               || (result[i].created_at == result[j].created_at
//                  && result[i].id > result[j].id)                     // F2
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
