//! Shared bounded, duplicate-rejecting JSON decoding. Callers bound bytes first.
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::fmt;

pub(crate) fn decode_unique(bytes: &[u8], max_depth: usize) -> Result<Value, serde_json::Error> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = Seed {
        depth: 0,
        max_depth,
    }
    .deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(value)
}
struct Seed {
    depth: usize,
    max_depth: usize,
}
impl Seed {
    fn child(&self) -> Self {
        Self {
            depth: self.depth + 1,
            max_depth: self.max_depth,
        }
    }
}
impl<'de> DeserializeSeed<'de> for Seed {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<Value, D::Error> {
        if self.depth > self.max_depth {
            return Err(de::Error::custom("JSON depth exceeds its bound"));
        }
        decoder.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Seed {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON without duplicate object fields")
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
            .ok_or_else(|| de::Error::custom("invalid JSON number"))
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
        while let Some(value) = sequence.next_element_seed(self.child())? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut fields: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = fields.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON field"));
            }
            values.insert(key, fields.next_value_seed(self.child())?);
        }
        Ok(Value::Object(values))
    }
}
