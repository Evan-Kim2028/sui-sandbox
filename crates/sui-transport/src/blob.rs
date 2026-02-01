//! Minimal blob decoding for Walrus checkpoint data.
//!
//! This is a stripped-down version of sui_storage::blob that avoids
//! pulling in RocksDB as a transitive dependency.

use anyhow::{anyhow, Result};
use num_enum::TryFromPrimitive;
use serde::de::DeserializeOwned;

#[derive(Copy, Clone, Debug, Eq, PartialEq, TryFromPrimitive)]
#[repr(u8)]
pub enum BlobEncoding {
    Bcs = 1,
}

pub struct Blob {
    pub data: Vec<u8>,
    pub encoding: BlobEncoding,
}

impl Blob {
    /// Decode a blob from raw bytes.
    ///
    /// Format: [encoding_byte || payload]
    pub fn from_bytes<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
        let (encoding, data) = bytes.split_first().ok_or(anyhow!("empty bytes"))?;
        Blob {
            data: data.to_vec(),
            encoding: BlobEncoding::try_from(*encoding)?,
        }
        .decode()
    }

    fn decode<T: DeserializeOwned>(self) -> Result<T> {
        match self.encoding {
            BlobEncoding::Bcs => {
                let res = bcs::from_bytes(&self.data)?;
                Ok(res)
            }
        }
    }
}
