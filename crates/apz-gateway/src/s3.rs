//! A minimal S3 client (path-style, SigV4) for the plaza's object store (Garage):
//! put, get and delete one object. Bodies are held in memory.

/// One bucket on an S3-compatible endpoint.
pub struct S3 {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
}

impl S3 {
    /// Stores `body` at `key` with `content_type`.
    pub fn put(&self, _key: &str, _body: &[u8], _content_type: &str) -> Result<(), String> {
        Err("s3 is not available yet".to_string())
    }

    /// The object at `key`.
    pub fn get(&self, _key: &str) -> Result<Vec<u8>, String> {
        Err("s3 is not available yet".to_string())
    }

    /// Removes the object at `key` (absent is not an error).
    pub fn delete(&self, _key: &str) -> Result<(), String> {
        Err("s3 is not available yet".to_string())
    }
}
