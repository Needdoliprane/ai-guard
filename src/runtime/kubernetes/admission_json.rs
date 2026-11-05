//! Bounded canonical output and duplicate-rejecting input for the new profile.
//! Historical inspection and dry-run decoding remain unchanged.
use super::super::{KubernetesError, KubernetesResult};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::{collections::BTreeMap, fmt, io::Write};

pub(super) const MAX_DEPTH: usize = 64;

pub(super) fn decode(bytes: &[u8]) -> KubernetesResult<Value> {
    if bytes.len() > 1024 * 1024 {
        return Err(KubernetesError::ResponseTooLarge);
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = Seed(0)
        .deserialize(&mut decoder)
        .map_err(|_| KubernetesError::InvalidResponse)?;
    decoder
        .end()
        .map_err(|_| KubernetesError::InvalidResponse)?;
    Ok(value)
}

struct Seed(usize);
impl<'de> DeserializeSeed<'de> for Seed {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<Value, D::Error> {
        if self.0 > MAX_DEPTH {
            return Err(de::Error::custom("held JSON depth exceeds its bound"));
        }
        decoder.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Seed {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded held JSON without duplicate object fields")
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| de::Error::custom("invalid held JSON number"))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.into()))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(Seed(self.0 + 1))? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut fields: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = fields.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate held JSON field"));
            }
            values.insert(key, fields.next_value_seed(Seed(self.0 + 1))?);
        }
        Ok(Value::Object(values))
    }
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
