//! Type-erased, immutable custom payload storage with zero-copy typed access and JSON fallback.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::any::{Any, type_name};
use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

/// Failure returned when reading or serializing a [`CustomPayload`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PayloadError {
    /// A typed in-memory payload held a different Rust type than requested.
    TypeMismatch {
        expected: &'static str,
        actual: &'static str,
    },
    /// Decoding a raw JSON payload into the expected Rust type failed.
    Deserialize(String),
    /// Serializing a typed payload into JSON failed.
    Serialize(String),
}

impl fmt::Display for PayloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TypeMismatch { expected, actual } => {
                write!(formatter, "payload type mismatch: expected {expected}, found {actual}")
            }
            Self::Deserialize(message) => write!(formatter, "payload decode error: {message}"),
            Self::Serialize(message) => write!(formatter, "payload serialize error: {message}"),
        }
    }
}

impl std::error::Error for PayloadError {}

/// Trait implemented automatically for strongly typed host payloads stored in [`CustomPayload`].
pub trait CustomData: Any + Send + Sync + fmt::Debug {
    fn as_any(&self) -> &dyn Any;
    fn rust_type_name(&self) -> &'static str;
    fn eq_data(&self, other: &dyn CustomData) -> bool;
    fn to_json(&self) -> Result<serde_json::Value, serde_json::Error>;
}

impl<T> CustomData for T
where
    T: Any + Send + Sync + fmt::Debug + PartialEq + Serialize + 'static,
{
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn rust_type_name(&self) -> &'static str {
        type_name::<T>()
    }

    fn eq_data(&self, other: &dyn CustomData) -> bool {
        other
            .as_any()
            .downcast_ref::<T>()
            .is_some_and(|rhs| self == rhs)
    }

    fn to_json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
    }
}

/// Immutable, `O(1)`-cloneable payload carried by open semantic blocks and custom inline objects.
///
/// Stores either a strongly typed host struct (`Arc<dyn CustomData>`) for zero-overhead runtime
/// access or a raw JSON value (`Arc<serde_json::Value>`) produced when deserializing documents or
/// clipboard data without an eager type registry.
///
/// Serializing a typed payload whose [`Serialize`] implementation fails returns a serialization
/// error rather than substituting JSON `null`.
#[derive(Clone, Debug)]
pub struct CustomPayload(PayloadInner);

#[derive(Clone, Debug)]
enum PayloadInner {
    Typed(Arc<dyn CustomData>),
    Raw(Arc<serde_json::Value>),
}

impl Default for CustomPayload {
    fn default() -> Self {
        Self::empty()
    }
}

impl CustomPayload {
    /// Creates an empty (`null`) payload.
    pub fn empty() -> Self {
        Self(PayloadInner::Raw(Arc::new(serde_json::Value::Null)))
    }

    /// Wraps a strongly typed host struct in an `Arc` without converting to JSON.
    pub fn typed<T: CustomData>(value: T) -> Self {
        Self(PayloadInner::Typed(Arc::new(value)))
    }

    /// Wraps an already-boxed [`CustomData`] trait object.
    pub fn from_arc(value: Arc<dyn CustomData>) -> Self {
        Self(PayloadInner::Typed(value))
    }

    /// Wraps a raw JSON value.
    pub fn from_json(value: serde_json::Value) -> Self {
        Self(PayloadInner::Raw(Arc::new(value)))
    }

    /// Returns `true` when this payload is a raw JSON `null`.
    pub fn is_empty(&self) -> bool {
        matches!(&self.0, PayloadInner::Raw(value) if value.is_null())
    }

    /// Returns `true` when this payload holds a strongly typed Rust struct in memory.
    pub fn is_typed(&self) -> bool {
        matches!(&self.0, PayloadInner::Typed(_))
    }

    /// Returns the backing raw JSON `Arc` when this payload is stored in raw JSON form.
    ///
    /// Keeping the original `Arc<serde_json::Value>` allows downstream caches to key resolved
    /// values by pointer identity while preserving unknown fields on re-export.
    pub fn raw_json(&self) -> Option<&Arc<serde_json::Value>> {
        match &self.0 {
            PayloadInner::Raw(value) => Some(value),
            PayloadInner::Typed(_) => None,
        }
    }

    /// Zero-cost downcast when this payload holds a typed value of concrete type `T`.
    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        match &self.0 {
            PayloadInner::Typed(data) => data.as_any().downcast_ref::<T>(),
            PayloadInner::Raw(_) => None,
        }
    }

    /// Borrows `T` directly when stored as `Typed(T)`, or decodes from `Raw` JSON with explicit errors.
    pub fn try_as_typed<T>(&self) -> Result<Cow<'_, T>, PayloadError>
    where
        T: Clone + DeserializeOwned + 'static,
    {
        match &self.0 {
            PayloadInner::Typed(data) => data
                .as_any()
                .downcast_ref::<T>()
                .map(Cow::Borrowed)
                .ok_or_else(|| PayloadError::TypeMismatch {
                    expected: type_name::<T>(),
                    actual: data.rust_type_name(),
                }),
            PayloadInner::Raw(value) => serde_json::from_value::<T>((**value).clone())
                .map(Cow::Owned)
                .map_err(|error| PayloadError::Deserialize(error.to_string())),
        }
    }

    /// Borrows `T` directly when stored as `Typed(T)`, or lazily deserializes from `Raw` JSON.
    pub fn as_typed<T>(&self) -> Option<Cow<'_, T>>
    where
        T: Clone + DeserializeOwned + 'static,
    {
        self.try_as_typed::<T>().ok()
    }

    /// Borrows the raw JSON value when stored as `Raw`, or serializes `Typed` data on demand.
    pub fn as_json(&self) -> Result<Cow<'_, serde_json::Value>, PayloadError> {
        match &self.0 {
            PayloadInner::Raw(value) => Ok(Cow::Borrowed(value.as_ref())),
            PayloadInner::Typed(data) => data
                .to_json()
                .map(Cow::Owned)
                .map_err(|error| PayloadError::Serialize(error.to_string())),
        }
    }

    /// Materializes a JSON representation of this payload.
    pub fn to_json(&self) -> Result<serde_json::Value, PayloadError> {
        match &self.0 {
            PayloadInner::Raw(value) => Ok((**value).clone()),
            PayloadInner::Typed(data) => data
                .to_json()
                .map_err(|error| PayloadError::Serialize(error.to_string())),
        }
    }
}

impl From<serde_json::Value> for CustomPayload {
    fn from(value: serde_json::Value) -> Self {
        Self::from_json(value)
    }
}

impl PartialEq for CustomPayload {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (PayloadInner::Typed(left), PayloadInner::Typed(right)) => {
                Arc::ptr_eq(left, right) || left.eq_data(right.as_ref())
            }
            (PayloadInner::Raw(left), PayloadInner::Raw(right)) => {
                Arc::ptr_eq(left, right) || left == right
            }
            (PayloadInner::Typed(typed), PayloadInner::Raw(raw))
            | (PayloadInner::Raw(raw), PayloadInner::Typed(typed)) => {
                typed.to_json().ok().is_some_and(|value| value == **raw)
            }
        }
    }
}

impl Serialize for CustomPayload {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.0 {
            PayloadInner::Typed(data) => {
                let value = data
                    .to_json()
                    .map_err(serde::ser::Error::custom)?;
                value.serialize(serializer)
            }
            PayloadInner::Raw(value) => value.as_ref().serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for CustomPayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_json::Value::deserialize(deserializer).map(Self::from_json)
    }
}
