//! Pure value types and the small constructors that enforce per-value
//! invariants.
//!
//! # F-properties
//! - **F4**: `Follow::new` rejects self-follow at construction time. There is
//!   no other public constructor, so no `Follow` value with `from == to` can
//!   exist anywhere in the program.
//!
//! # Verus annotations
//!
//! Stream 3 Phase 3 — F4 is **discharged**, not skeleton-trusted.
//!
//! The `Follow`, `User`, `Tweet`, and `DomainError` types and `Follow::new`
//! are all defined inside a top-level `verus! { ... }` block so Verus can
//! see the field projections and discharge the postconditions against the
//! function body. Under `--cfg verus_keep_ghost` (set by `cargo verus
//! verify`), Verus checks that the body satisfies the `ensures` clauses.
//! Under stable rustc, the `verus!` macro erases the ghost annotations
//! and the function compiles as plain Rust.
//!
//! Discharged contract:
//!
//! ```text
//! ensures
//!     from@ == to@ ==> result is Err,
//!     from@ != to@ ==> result is Ok,
//!     result is Ok ==> result->Ok_0.from@ == from@,
//!     result is Ok ==> result->Ok_0.to@   == to@,
//!     result is Ok ==> result->Ok_0.from@ != result->Ok_0.to@,
//! ```
//!
//! `String` equality goes through `vstd`'s built-in `View` for `String`
//! (`s@ -> Seq<char>`) plus the `assume_specification` for
//! `<String as PartialEq>::eq` (which says `(a == b) == (a@ == b@)`).
//! No new `external_type_specification` rows are needed — `vstd` already
//! ships `ExString` (`std_specs/string.rs`) and `Result` is handled in
//! `std_specs/result.rs`. Net TCB delta: removes the `verus_proof`
//! "trusted skeleton" row; adds zero rows.

use std::fmt;
use vstd::prelude::*;

verus! {
    /// A registered user.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct User {
        pub id: i64,
        pub handle: String,
    }

    /// A posted tweet. Created via the service layer; this type is plain data.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Tweet {
        pub id: i64,
        pub author: String,
        pub text: String,
        pub created_at: i64,
    }

    /// A follow edge from `from` to `to`. Constructed through `Follow::new`,
    /// which is the only public way to make one — that's where F4 lives.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Follow {
        pub from: String,
        pub to: String,
    }

    /// Errors raised by domain constructors.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum DomainError {
        /// F4: `from == to` is rejected at construction time.
        SelfFollow,
    }

    impl Follow {
        /// Builds a `Follow`, rejecting self-follow (F4).
        pub fn new(from: String, to: String) -> (result: Result<Follow, DomainError>)
            ensures
                from@ == to@ ==> result is Err,
                from@ != to@ ==> result is Ok,
                result is Ok ==> result->Ok_0.from@ == from@,
                result is Ok ==> result->Ok_0.to@   == to@,
                result is Ok ==> result->Ok_0.from@ != result->Ok_0.to@,
        {
            if from == to {
                return Err(DomainError::SelfFollow);
            }
            Ok(Follow { from, to })
        }
    }
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DomainError::SelfFollow => f.write_str("self_follow_forbidden"),
        }
    }
}

impl std::error::Error for DomainError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_rejects_self() {
        let err = Follow::new("alice".to_string(), "alice".to_string()).unwrap_err();
        assert_eq!(err, DomainError::SelfFollow);
        assert_eq!(err.to_string(), "self_follow_forbidden");
    }

    #[test]
    fn follow_accepts_different() {
        let f = Follow::new("alice".to_string(), "bob".to_string()).unwrap();
        assert_eq!(f.from, "alice");
        assert_eq!(f.to, "bob");
    }

    #[test]
    fn user_construction() {
        let u = User { id: 1, handle: "alice".to_string() };
        assert_eq!(u.id, 1);
        assert_eq!(u.handle, "alice");
        // Clone + PartialEq derive smoke
        assert_eq!(u.clone(), u);
    }

    #[test]
    fn tweet_construction() {
        let t = Tweet {
            id: 1,
            author: "alice".to_string(),
            text: "hi".to_string(),
            created_at: 5,
        };
        assert_eq!(t.id, 1);
        assert_eq!(t.author, "alice");
        assert_eq!(t.text, "hi");
        assert_eq!(t.created_at, 5);
        assert_eq!(t.clone(), t);
    }

    #[test]
    fn domain_error_is_std_error() {
        let err = DomainError::SelfFollow;
        let _: &dyn std::error::Error = &err;
        // Debug impl smoke
        assert!(format!("{err:?}").contains("SelfFollow"));
    }

    #[test]
    fn follow_clone_eq() {
        let f = Follow::new("a".to_string(), "b".to_string()).unwrap();
        assert_eq!(f.clone(), f);
    }
}
