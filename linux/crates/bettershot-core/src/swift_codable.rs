//! Helpers for the JSON shapes Swift's synthesized `Codable` produces.
//!
//! An enum case with an unlabeled payload encodes as `{"case": {"_0": value}}`
//! and a case without one as `{"case": {}}`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payload<T> {
    #[serde(rename = "_0")]
    pub value: T,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Empty {}
