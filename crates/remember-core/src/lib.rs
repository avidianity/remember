//! Remember core: the domain model, storage, recall and TOON encoding behind
//! every transport. Contains no MCP or transport code (see ADR 0001).

pub mod db;
pub mod error;
pub mod hub;
pub mod model;
pub mod project;
pub mod secrets;
pub mod toon;
pub mod views;

pub use error::{Error, Result};
pub use hub::{Hub, NewMemory, RecallQuery, Terminal};
pub use model::{Category, Limits, Scope};
