//! Persistence layer for Flaps.
//!
//! SQLx multi backend storage supporting SQLite and PostgreSQL, embedded
//! migrations and an append only audit log written in the same transaction
//! as every mutation. The store lands with the v0.1.0 milestone.
