//! Strict wire schemas. Public names stay here; each message family owns
//! its validation and encoding in a private module.
use std::collections::HashSet;
use std::fmt;

use serde::de::{self, DeserializeOwned};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use thiserror::Error;

use crate::release_policy::{PolicyBlob, PolicySignature};
use crate::scalar::{Bytes32, Counter, FeatureBits, P256PublicKey, U64String};
use crate::{BASE_V1, COMPATIBILITY_EPOCH, PROTOCOL_MAJOR};

macro_rules! result_struct {
    ($name:ident { $($(#[$meta:meta])* $field:ident : $type:ty),* $(,)? }) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        pub struct $name { $($(#[$meta])* pub $field: $type),* }
    };
}

fn strict_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, SchemaError> {
    serde_json::from_slice(bytes).map_err(|_| SchemaError::Json)
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

mod bindings;
pub use bindings::*;

mod errors;
pub use errors::*;

mod events;
pub use events::*;

mod handshake;
pub use handshake::*;

mod method;
pub use method::*;

mod observability;
pub use observability::*;

mod params;
pub use params::*;

mod predecessor;
pub use predecessor::*;

mod protocol;
pub use protocol::*;

mod request;
pub use request::*;

mod response;
pub use response::*;

mod results;
pub use results::*;

mod text;
pub use text::*;
