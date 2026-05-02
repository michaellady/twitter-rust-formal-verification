//! Pure value types and the small constructors that enforce per-value
//! invariants.
//!
//! # F-properties
//! - **F4**: `Follow::new` rejects self-follow at construction time. There is
//!   no other public constructor, so no `Follow` value with `from == to` can
//!   exist anywhere in the program.
//!
//! # Verus annotations
//! `Follow::new`'s contract under Verus is:
//!
//!   ensures
//!       result.is_ok() ==> result.unwrap().from != result.unwrap().to,
//!       (from == to) ==> result.is_err()
//!
//! This is what F4 requires; everything downstream takes it as a precondition.

use std::fmt;

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

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DomainError::SelfFollow => f.write_str("self_follow_forbidden"),
        }
    }
}

impl std::error::Error for DomainError {}

impl Follow {
    /// Builds a `Follow`, rejecting self-follow (F4).
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Result<Self, DomainError> {
        let from = from.into();
        let to = to.into();
        if from == to {
            return Err(DomainError::SelfFollow);
        }
        Ok(Self { from, to })
    }
}

// =============================================================================
// Verus proof obligations (F4).
// =============================================================================
#[cfg(verus)]
mod verus_proof {
    use super::*;
    verus! {
        #[verifier::external_body]
        pub fn follow_new_ensures(from: String, to: String)
            -> (out: Result<Follow, DomainError>)
            ensures
                from == to ==> out.is_err(),
                out.is_ok() ==> out.unwrap().from != out.unwrap().to;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_rejects_self() {
        let err = Follow::new("alice", "alice").unwrap_err();
        assert_eq!(err, DomainError::SelfFollow);
        assert_eq!(err.to_string(), "self_follow_forbidden");
    }

    #[test]
    fn follow_accepts_different() {
        let f = Follow::new("alice", "bob").unwrap();
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
        let f = Follow::new("a", "b").unwrap();
        assert_eq!(f.clone(), f);
    }
}
