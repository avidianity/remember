//! The single seam to the TOON encoder (see ADR 0002).
//!
//! Everything Remember returns to an Agent goes through [`encode`], so replacing
//! `serde_toon_format` touches only this file.

use serde::Serialize;

use crate::error::{Error, Result};

pub fn encode<T: Serialize>(value: &T) -> Result<String> {
    serde_toon::to_string(value).map_err(|e| Error::Encode(e.to_string()))
}
