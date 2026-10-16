//! setup-as-code: a declarative manifest for a personal AI assistant setup.
//!
//! The data model is dynamic on purpose: `Doc` is a `serde_json::Value` with insertion-ordered
//! maps, and the JSON schema in `schema/` is the type system. Modules take and return `Doc`
//! fragments shaped like `setup.yaml`.

pub mod apply;
pub mod cli;
pub mod core;
pub mod ctx;
pub mod importers;
pub mod init;
pub mod layers;
pub mod lint;
pub mod live;
pub mod output;
pub mod plan;
pub mod policy;
pub mod redact;
pub mod schema;
pub mod secrets;
pub mod util;
pub mod yaml;

/// A fragment of, or a whole, `setup.yaml`.
pub type Doc = serde_json::Value;

/// The manifest schema (draft-07), embedded so the binary needs no files beside it.
pub const SCHEMA_JSON: &str = include_str!("../schema/setup.schema.json");

/// Version string shown by `sac --version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
