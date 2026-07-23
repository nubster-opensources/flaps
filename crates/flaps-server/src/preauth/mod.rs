//! Pre-authentication hardening: bounding what an unauthenticated request can
//! spend before it has proved anything (see issues #133 and #134).
//!
//! Every piece here exists to enforce a single ordering rule: the control that
//! bounds a cost always runs before the cost itself.

pub mod budget;
pub mod client_address;
pub mod limiter_key;
pub mod limits;
pub mod password_pool;
