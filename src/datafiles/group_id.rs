use std::fmt::Formatter;
use std::ops::Deref;

use serde::de::{Error, Unexpected, Visitor};
use serde::{Deserialize, Deserializer};

#[repr(transparent)]
#[derive(Hash, PartialOrd, Ord, PartialEq, Eq, Debug, Copy, Clone)]
pub struct GroupId(pub i64);

impl Deref for GroupId {
    type Target = i64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de> Deserialize<'de> for GroupId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(GroupIdVisitor)
    }
}

struct GroupIdVisitor;

impl<'de> Visitor<'de> for GroupIdVisitor {
    type Value = GroupId;

    fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
        formatter.write_str("an integer between -2^31 and 2^31, optionally represented as a string with arbitrary many leading zeros.")
    }

    fn visit_i8<E>(self, v: i8) -> Result<Self::Value, E>
    where
        E: Error,
    {
        Ok(GroupId(i64::from(v)))
    }

    fn visit_i16<E>(self, v: i16) -> Result<Self::Value, E>
    where
        E: Error,
    {
        Ok(GroupId(i64::from(v)))
    }

    fn visit_i32<E>(self, v: i32) -> Result<Self::Value, E>
    where
        E: Error,
    {
        Ok(GroupId(i64::from(v)))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: Error,
    {
        Ok(GroupId(v))
    }

    fn visit_u8<E>(self, v: u8) -> Result<Self::Value, E>
    where
        E: Error,
    {
        Ok(GroupId(i64::from(v)))
    }

    fn visit_u16<E>(self, v: u16) -> Result<Self::Value, E>
    where
        E: Error,
    {
        Ok(GroupId(i64::from(v)))
    }

    fn visit_u32<E>(self, v: u32) -> Result<Self::Value, E>
    where
        E: Error,
    {
        Ok(GroupId(i64::from(v)))
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: Error,
    {
        let string = v.trim_start_matches('0');
        if string.is_empty() {
            return Ok(GroupId(0));
        }
        Ok(GroupId(string.parse::<i64>().map_err(|_| {
            E::invalid_value(Unexpected::Str(v), &self)
        })?))
    }

    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E>
    where
        E: Error,
    {
        let string = v.trim_start_matches('0');
        if string.is_empty() {
            return Ok(GroupId(0));
        }
        Ok(GroupId(string.parse::<i64>().map_err(|_| {
            E::invalid_value(Unexpected::Str(v), &self)
        })?))
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: Error,
    {
        let string = v.trim_start_matches('0');
        if string.is_empty() {
            return Ok(GroupId(0));
        }
        Ok(GroupId(string.parse::<i64>().map_err(|_| {
            E::invalid_value(Unexpected::Str(&v), &self)
        })?))
    }
}
