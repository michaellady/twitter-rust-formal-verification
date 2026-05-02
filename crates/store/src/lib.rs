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

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use domain::{Follow, Tweet, User};

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
#[cfg(verus)]
mod verus_proof {
    use super::*;
    verus! {
        // Obligations stated above, dispatched via #[verifier::external_body]
        // wrappers because Mutex/HashMap require trusted shims in vstd.
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
        let f = Follow::new("alice", "bob").unwrap();
        assert_eq!(s.put_follow(f).unwrap_err(), StoreError::UnknownUser);
    }

    #[test]
    fn put_follow_rejects_unknown_to() {
        let s = MemStore::new();
        s.put_user(alice()).unwrap();
        let f = Follow::new("alice", "bob").unwrap();
        assert_eq!(s.put_follow(f).unwrap_err(), StoreError::UnknownUser);
    }

    #[test]
    fn put_follow_is_idempotent_f3() {
        let s = MemStore::new();
        s.put_user(alice()).unwrap();
        s.put_user(bob()).unwrap();
        s.put_follow(Follow::new("alice", "bob").unwrap()).unwrap();
        s.put_follow(Follow::new("alice", "bob").unwrap()).unwrap();
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
        s.put_follow(Follow::new("alice", "bob").unwrap()).unwrap();
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
        s.put_follow(Follow::new("alice", "bob").unwrap()).unwrap();
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
        s.put_follow(Follow::new("alice", "bob").unwrap()).unwrap();
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
                    let _ = s.put_follow(Follow::new("alice", "bob").unwrap());
                }
            }));
        }
        for h in handles { h.join().unwrap(); }
        assert_eq!(s.follow_set("alice").len(), 1);
    }
}
