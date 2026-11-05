//! Bounded canonical output and duplicate-rejecting input for the new profile.
//! Historical inspection and dry-run decoding remain unchanged.
use super::super::{KubernetesError, KubernetesResult};
use serde_json::Value;
use std::{collections::BTreeMap, io::Write};

pub(super) const MAX_DEPTH: usize = 64;

pub(super) fn decode(bytes: &[u8]) -> KubernetesResult<Value> {
    if bytes.len() > 1024 * 1024 {
        return Err(KubernetesError::ResponseTooLarge);
    }
    crate::json::decode_unique(bytes, MAX_DEPTH).map_err(|_| KubernetesError::InvalidResponse)
}

struct BoundedBytes {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("held JSON exceeds its bound"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn canonical_bytes(value: &Value, limit: usize) -> KubernetesResult<Vec<u8>> {
    let mut output = BoundedBytes {
        bytes: Vec::new(),
        limit,
    };
    canonical(value, &mut output, 0).map_err(|_| KubernetesError::InvalidResponse)?;
    Ok(output.bytes)
}
fn canonical(value: &Value, output: &mut BoundedBytes, depth: usize) -> std::io::Result<()> {
    if depth > MAX_DEPTH {
        return Err(std::io::Error::other("held JSON depth exceeds its bound"));
    }
    match value {
        Value::Object(fields) => {
            output.write_all(b"{")?;
            let sorted: BTreeMap<_, _> = fields.iter().collect();
            for (index, (key, value)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    output.write_all(b",")?;
                }
                serde_json::to_writer(&mut *output, key)?;
                output.write_all(b":")?;
                canonical(value, output, depth + 1)?;
            }
            output.write_all(b"}")
        }
        Value::Array(values) => {
            output.write_all(b"[")?;
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.write_all(b",")?;
                }
                canonical(value, output, depth + 1)?;
            }
            output.write_all(b"]")
        }
        scalar => serde_json::to_writer(output, scalar).map_err(std::io::Error::other),
    }
}
