#![doc = include_str!("../README.md")]
//! ## Feature flags
#![doc = document_features::document_features!()]
//!

use getrandom::getrandom;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use std::array::TryFromSliceError;
use std::error::Error;
use std::fmt::Display;
use std::{
    ops::{Add, AddAssign, Sub, SubAssign},
    sync::Mutex,
};

use once_cell::sync::Lazy;

#[cfg(not(target_arch = "wasm32"))]
use std::time::SystemTime;

/// ~0.4% chance of none of 10 clocks have matching id.
const CLOCK_MASK: u64 = (1 << 8) - 1;
const TIME_MASK: u64 = !0 >> 8;

pub struct TimestampFactory {
    clock_id: u64,
    last_time: u64,
}

impl TimestampFactory {
    /// Create a [TimestampFactory] with a random [TimestampFactory::clock_id],
    /// unless [getrandom] returned and error, in which case it defaults to `0`.
    pub fn new() -> Self {
        let mut bytes = [0; 8];
        let _ = getrandom(&mut bytes);

        Self {
            clock_id: u64::from_le_bytes(bytes) & CLOCK_MASK,
            last_time: system_time() & TIME_MASK,
        }
    }

    /// Set the factory's `clock_id`
    pub fn clock_id(mut self, clock_id: u8) -> TimestampFactory {
        self.clock_id = clock_id as u64;
        self
    }

    /// Generate a new [Timestamp]
    pub fn now(&mut self) -> Timestamp {
        // Ensure strict monotonicity.
        self.last_time = (system_time() & TIME_MASK).max(self.last_time + CLOCK_MASK + 1);

        // Add clock_id to the end of the timestamp
        Timestamp(self.last_time | self.clock_id)
    }
}

impl Default for TimestampFactory {
    fn default() -> Self {
        Self::new()
    }
}

pub static DEFAULT_FACTORY: Lazy<Mutex<TimestampFactory>> =
    Lazy::new(|| Mutex::new(TimestampFactory::default()));

/// Strictly monotonic timestamp since [SystemTime::UNIX_EPOCH] in microseconds.
///
/// The purpose of this timestamp is to unique per "user", not globally,
/// it achieves this by:
///     1. Override the last byte with a random `clock_id`, reducing the probability
///         of two matching timestamps across multiple machines/threads.
///     2. Gurantee that the remaining 3 bytes are ever increasing (strictly monotonic) within
///         the same thread regardless of the wall clock value
///
/// This timestamp is also serialized as BE bytes to remain sortable.
/// If a `utf-8` encoding is necessary, it is encoded as [base32::Alphabet::Crockford]
/// to act as a sortable Id.
///
/// U64 of microseconds is valid for the next 500 thousand years!
#[derive(Debug, Clone, PartialEq, PartialOrd, Hash, Eq, Ord)]
pub struct Timestamp(u64);

impl Timestamp {
    /// Generate a [Timestamp] from the [DEFAULT_FACTORY].
    pub fn now() -> Self {
        DEFAULT_FACTORY.lock().unwrap().now()
    }

    /// Return big endian bytes representation of this timestamp.
    pub fn to_bytes(&self) -> [u8; 8] {
        self.0.to_be_bytes()
    }

    /// Return the internal `u64` representation of this [Timestamp].
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl Default for Timestamp {
    fn default() -> Self {
        Timestamp::now()
    }
}

impl TryFrom<&[u8]> for Timestamp {
    type Error = TryFromSliceError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        let bytes: [u8; 8] = bytes.try_into()?;

        Ok(bytes.into())
    }
}

impl From<&Timestamp> for [u8; 8] {
    fn from(timestamp: &Timestamp) -> Self {
        timestamp.0.to_be_bytes()
    }
}

impl From<[u8; 8]> for Timestamp {
    fn from(bytes: [u8; 8]) -> Self {
        Self(u64::from_be_bytes(bytes))
    }
}

impl From<u64> for Timestamp {
    fn from(inner: u64) -> Self {
        Self(inner)
    }
}

impl From<&Timestamp> for Timestamp {
    fn from(timestamp: &Timestamp) -> Self {
        timestamp.clone()
    }
}

impl From<Timestamp> for u64 {
    fn from(value: Timestamp) -> Self {
        value.as_u64()
    }
}

impl From<&Timestamp> for u64 {
    fn from(value: &Timestamp) -> Self {
        value.as_u64()
    }
}

// === U64 conversion ===

impl Add<u64> for &Timestamp {
    type Output = Timestamp;

    fn add(self, rhs: u64) -> Self::Output {
        Timestamp(self.0 + rhs)
    }
}

impl Sub<u64> for &Timestamp {
    type Output = Timestamp;

    fn sub(self, rhs: u64) -> Self::Output {
        Timestamp(self.0 - rhs)
    }
}

impl AddAssign<u64> for Timestamp {
    fn add_assign(&mut self, other: u64) {
        self.0 += other;
    }
}

impl SubAssign<u64> for Timestamp {
    fn sub_assign(&mut self, other: u64) {
        self.0 -= other;
    }
}

// === Serialization ===

#[cfg(feature = "serde")]
impl Serialize for Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let bytes = self.to_bytes();
        bytes.serialize(serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes: [u8; 8] = Deserialize::deserialize(deserializer)?;
        Ok(Timestamp(u64::from_be_bytes(bytes)))
    }
}

// === String representation (sortable base32 encoding) ===

impl Display for Timestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[cfg(feature = "base32")]
        {
            let bytes: [u8; 8] = self.into();
            return f.write_str(&base32::encode(base32::Alphabet::Crockford, &bytes));
        }
        #[cfg(not(feature = "base32"))]
        return f.write_str(&format!("Timestamp ({})", self.0));
    }
}

impl TryFrom<String> for Timestamp {
    type Error = InvalidEncoding;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        #[cfg(feature = "base32")]
        return match base32::decode(base32::Alphabet::Crockford, &value) {
            Some(vec) => {
                let bytes: [u8; 8] = vec.try_into().map_err(|_| InvalidEncoding)?;

                Ok(bytes.into())
            }
            None => Err(InvalidEncoding),
        };

        #[cfg(not(feature = "base32"))]
        Ok(Self(
            value[11..value.len() - 1]
                .parse()
                .map_err(|_| InvalidEncoding)?,
        ))
    }
}

/// Return the number of microseconds since [SystemTime::UNIX_EPOCH]
fn system_time() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("time drift")
            .as_micros() as u64
    }
    #[cfg(target_arch = "wasm32")]
    {
        // Won't be an issue for more than 5000 years!
        (js_sys::Date::now() as u64 )
        // Turn miliseconds to microseconds
        * 1000
    }
}

#[derive(Debug)]
pub struct InvalidEncoding;

impl Error for InvalidEncoding {}

impl Display for InvalidEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        return f.write_str("Invalid timestamp string encoding");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn strictly_monotonic() {
        const COUNT: usize = 100;

        let mut set = HashSet::with_capacity(COUNT);
        let mut vec = Vec::with_capacity(COUNT);

        for _ in 0..COUNT {
            let timestamp = Timestamp::now();

            set.insert(timestamp.clone());
            vec.push(timestamp);
        }

        let mut ordered = vec.clone();
        ordered.sort();

        assert_eq!(set.len(), COUNT, "unique");
        assert_eq!(ordered, vec, "ordered");
    }

    #[test]
    fn strings() {
        const COUNT: usize = 100;

        let mut set = HashSet::with_capacity(COUNT);
        let mut vec = Vec::with_capacity(COUNT);

        for _ in 0..COUNT {
            let string = Timestamp::now().to_string();

            set.insert(string.clone());
            vec.push(string)
        }

        let mut ordered = vec.clone();
        ordered.sort();

        assert_eq!(set.len(), COUNT, "unique");
        assert_eq!(ordered, vec, "ordered");
    }

    #[test]
    fn to_from_string() {
        let timestamp = Timestamp::now();
        let string = timestamp.to_string();
        let decoded: Timestamp = string.try_into().unwrap();

        assert_eq!(decoded, timestamp)
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde() {
        let timestamp = Timestamp::now();

        let serialized = postcard::to_allocvec(&timestamp).unwrap();

        assert_eq!(serialized, timestamp.to_bytes());

        let deserialized: Timestamp = postcard::from_bytes(&serialized).unwrap();

        assert_eq!(deserialized, timestamp);
    }
}
