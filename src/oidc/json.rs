//! Reject duplicate claims before a map can erase their ambiguity.
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::fmt;

pub(super) struct UniqueValue(pub Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(UniqueVisitor)
    }
}
struct UniqueVisitor;
impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = UniqueValue;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded unambiguous JSON")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Bool(v)))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(v.into())))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(v.into())))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
        serde_json::Number::from_f64(v)
            .map(|v| UniqueValue(Value::Number(v)))
            .ok_or_else(|| E::custom("invalid number"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        if v.len() > 65_536 {
            return Err(E::custom("string limit"));
        }
        Ok(UniqueValue(Value::String(v.to_owned())))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
        self.visit_str(&v)
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.visit_unit()
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(v) = a.next_element::<UniqueValue>()? {
            if values.len() >= 256 {
                return Err(de::Error::custom("array limit"));
            }
            values.push(v.0);
        }
        Ok(UniqueValue(Value::Array(values)))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
        let mut values = serde_json::Map::new();
        while let Some(k) = a.next_key::<String>()? {
            if k.len() > 256 || values.len() >= 256 || values.contains_key(&k) {
                return Err(de::Error::custom("ambiguous object"));
            }
            values.insert(k, a.next_value::<UniqueValue>()?.0);
        }
        Ok(UniqueValue(Value::Object(values)))
    }
}
pub(super) fn parse(bytes: &[u8]) -> Result<Value, ()> {
    serde_json::from_slice::<UniqueValue>(bytes)
        .map(|v| v.0)
        .map_err(|_| ())
}
