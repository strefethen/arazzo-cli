//! Shared model admission. Format envelopes preserve existing Serde behavior;
//! validity of expression names, versions, and syntax belongs to validation.

use std::{fmt, marker::PhantomData};

use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_yaml_ng::Value;

use crate::{ExpressionType, SelectorObject, SelectorType, VendorExtensions};

/// Checks the ValueSource envelope without copying its Content/value tree.
/// The returned string always belongs to the supplied Value.
pub(crate) fn borrowed_selector_context(value: &Value) -> Option<&str> {
    if contains_tag(value) {
        return None;
    }
    value
        .deserialize_map(SelectorVisitor::<Borrowed>(PhantomData))
        .ok()
        .map(|selector| selector.context)
}

// Derived untagged ValueSource rejects enum input anywhere while buffering
// Content. IgnoredAny on &Value deliberately skips descendants, so apply only
// that format-envelope rule here, without duplicating any model field rules.
fn contains_tag(value: &Value) -> bool {
    match value {
        Value::Tagged(_) => true,
        Value::Mapping(mapping) => mapping
            .iter()
            .any(|(key, value)| contains_tag(key) || contains_tag(value)),
        Value::Sequence(values) => values.iter().any(contains_tag),
        _ => false,
    }
}

trait Storage<'de>: Sized {
    type Text: Deserialize<'de> + AsRef<str>;
    type Extensions: Default;

    fn borrowed_text<E: de::Error>(text: &'de str) -> Result<Self::Text, E>;
    fn owned_text<E: de::Error>(text: String) -> Result<Self::Text, E>;
    fn extension<M: MapAccess<'de>>(
        map: &mut M,
        key: Key<Self::Text>,
        extensions: &mut Self::Extensions,
    ) -> Result<(), M::Error>;
    fn finish<E: de::Error>(extensions: Self::Extensions) -> Result<Self::Extensions, E>;
    fn selector_type<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<TypeFields<Self::Text, Self::Extensions>, D::Error>;
}

struct Owned;
struct Borrowed;

#[derive(Default)]
struct OwnedExtensions {
    values: VendorExtensions,
    error: Option<String>,
}

// Buffer this extension through the same untagged Content envelope as the old
// flattened map. Capture's successful outer Result retains conversion errors
// instead of replacing them with an untagged-enum variant failure.
#[derive(Deserialize)]
#[serde(untagged)]
enum Extension {
    Value(Capture),
}

struct Capture(Result<Value, String>);

impl<'de> Deserialize<'de> for Capture {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(
            Value::deserialize(deserializer).map_err(|error| error.to_string()),
        ))
    }
}

impl<'de> Storage<'de> for Owned {
    type Text = String;
    type Extensions = OwnedExtensions;

    fn borrowed_text<E: de::Error>(text: &'de str) -> Result<String, E> {
        Ok(text.into())
    }

    fn owned_text<E: de::Error>(text: String) -> Result<String, E> {
        Ok(text)
    }

    fn extension<M: MapAccess<'de>>(
        map: &mut M,
        key: Key<String>,
        extensions: &mut OwnedExtensions,
    ) -> Result<(), M::Error> {
        // Consume the envelope even for an invalid key: enum/tag rejection
        // happens during buffering, before known-field/required-field checks.
        let Extension::Value(Capture(value)) = map.next_value()?;
        match key {
            Key::Text(key) => match value {
                Ok(value) => {
                    extensions.values.insert(key, value);
                }
                Err(error) => {
                    extensions.error.get_or_insert(error);
                }
            },
            Key::InvalidBytes => {
                extensions
                    .error
                    .get_or_insert_with(|| "invalid value: byte array, expected a string".into());
            }
        }
        Ok(())
    }

    fn finish<E: de::Error>(mut extensions: OwnedExtensions) -> Result<OwnedExtensions, E> {
        if let Some(error) = extensions.error.take() {
            return Err(E::custom(error));
        }
        Ok(extensions)
    }

    fn selector_type<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<TypeFields<String, OwnedExtensions>, D::Error> {
        // Retain both the original Content strictness and public diagnostic
        // name. In particular, object string fields do not gain wire coercion.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum SelectorType {
            Type(OwnedType),
        }
        let SelectorType::Type(OwnedType(value)) = SelectorType::deserialize(deserializer)?;
        Ok(value)
    }
}

impl<'de> Storage<'de> for Borrowed {
    type Text = &'de str;
    type Extensions = ();

    fn borrowed_text<E: de::Error>(text: &'de str) -> Result<&'de str, E> {
        Ok(text)
    }

    fn owned_text<E: de::Error>(_text: String) -> Result<&'de str, E> {
        Err(E::custom("expected borrowed string"))
    }

    fn extension<M: MapAccess<'de>>(
        map: &mut M,
        _key: Key<&'de str>,
        _extensions: &mut (),
    ) -> Result<(), M::Error> {
        map.next_value::<IgnoredAny>()?;
        Ok(())
    }

    fn finish<E: de::Error>(extensions: ()) -> Result<(), E> {
        Ok(extensions)
    }

    fn selector_type<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<TypeFields<&'de str, ()>, D::Error> {
        deserializer.deserialize_any(TypeVisitor::<Self>(PhantomData))
    }
}

struct SelectorFields<Text, Extensions> {
    context: Text,
    selector: Text,
    type_: TypeFields<Text, Extensions>,
    extensions: Extensions,
}

struct ExpressionFields<Text, Extensions> {
    type_: Text,
    version: Text,
    extensions: Extensions,
}

enum TypeFields<Text, Extensions> {
    Name(Text),
    Object(ExpressionFields<Text, Extensions>),
}

enum Key<Text> {
    Text(Text),
    // A flattened unknown byte key is converted only after required fields.
    InvalidBytes,
}

struct KeySeed<S>(PhantomData<S>);
struct KeyVisitor<S>(PhantomData<S>);

impl<'de, S: Storage<'de>> DeserializeSeed<'de> for KeySeed<S> {
    type Value = Key<S::Text>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_identifier(KeyVisitor::<S>(PhantomData))
    }
}

impl<'de, S: Storage<'de>> Visitor<'de> for KeyVisitor<S> {
    type Value = Key<S::Text>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("field identifier")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Self::Value, E> {
        S::owned_text(text.into()).map(Key::Text)
    }

    fn visit_borrowed_str<E: de::Error>(self, text: &'de str) -> Result<Self::Value, E> {
        S::borrowed_text(text).map(Key::Text)
    }

    fn visit_string<E: de::Error>(self, text: String) -> Result<Self::Value, E> {
        S::owned_text(text).map(Key::Text)
    }

    fn visit_bytes<E: de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
        match std::str::from_utf8(bytes) {
            Ok(text) => self.visit_str(text),
            Err(_) => Ok(Key::InvalidBytes),
        }
    }

    fn visit_borrowed_bytes<E: de::Error>(self, bytes: &'de [u8]) -> Result<Self::Value, E> {
        match std::str::from_utf8(bytes) {
            Ok(text) => self.visit_borrowed_str(text),
            Err(_) => Ok(Key::InvalidBytes),
        }
    }

    fn visit_byte_buf<E: de::Error>(self, bytes: Vec<u8>) -> Result<Self::Value, E> {
        self.visit_bytes(&bytes)
    }
}

struct SelectorVisitor<S>(PhantomData<S>);

impl<'de, S: Storage<'de>> Visitor<'de> for SelectorVisitor<S> {
    type Value = SelectorFields<S::Text, S::Extensions>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("struct SelectorObject")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let (mut context, mut selector, mut type_) = (None, None, None);
        let mut extensions = S::Extensions::default();
        while let Some(key) = map.next_key_seed(KeySeed::<S>(PhantomData))? {
            match &key {
                Key::Text(text) => match text.as_ref() {
                    "context" => {
                        if context.is_some() {
                            return Err(de::Error::duplicate_field("context"));
                        }
                        context = Some(map.next_value::<S::Text>()?);
                    }
                    "selector" => {
                        if selector.is_some() {
                            return Err(de::Error::duplicate_field("selector"));
                        }
                        selector = Some(map.next_value::<S::Text>()?);
                    }
                    "type" => {
                        if type_.is_some() {
                            return Err(de::Error::duplicate_field("type"));
                        }
                        type_ = Some(map.next_value_seed(TypeSeed::<S>(PhantomData))?);
                    }
                    _ => S::extension(&mut map, key, &mut extensions)?,
                },
                Key::InvalidBytes => S::extension(&mut map, key, &mut extensions)?,
            }
        }
        Ok(SelectorFields {
            context: context.ok_or_else(|| de::Error::missing_field("context"))?,
            selector: selector.ok_or_else(|| de::Error::missing_field("selector"))?,
            type_: type_.ok_or_else(|| de::Error::missing_field("type"))?,
            extensions: S::finish(extensions)?,
        })
    }
}

struct ExpressionVisitor<S>(PhantomData<S>);

impl<'de, S: Storage<'de>> Visitor<'de> for ExpressionVisitor<S> {
    type Value = ExpressionFields<S::Text, S::Extensions>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("struct ExpressionType")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let (mut type_, mut version) = (None, None);
        let mut extensions = S::Extensions::default();
        while let Some(key) = map.next_key_seed(KeySeed::<S>(PhantomData))? {
            match &key {
                Key::Text(text) => match text.as_ref() {
                    "type" => {
                        if type_.is_some() {
                            return Err(de::Error::duplicate_field("type"));
                        }
                        type_ = Some(map.next_value::<S::Text>()?);
                    }
                    "version" => {
                        if version.is_some() {
                            return Err(de::Error::duplicate_field("version"));
                        }
                        version = Some(map.next_value::<S::Text>()?);
                    }
                    _ => S::extension(&mut map, key, &mut extensions)?,
                },
                Key::InvalidBytes => S::extension(&mut map, key, &mut extensions)?,
            }
        }
        Ok(ExpressionFields {
            type_: type_.unwrap_or(S::borrowed_text("")?),
            version: version.unwrap_or(S::borrowed_text("")?),
            extensions: S::finish(extensions)?,
        })
    }
}

struct TypeSeed<S>(PhantomData<S>);
struct TypeVisitor<S>(PhantomData<S>);

impl<'de, S: Storage<'de>> DeserializeSeed<'de> for TypeSeed<S> {
    type Value = TypeFields<S::Text, S::Extensions>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        S::selector_type(deserializer)
    }
}

impl<'de, S: Storage<'de>> Visitor<'de> for TypeVisitor<S> {
    type Value = TypeFields<S::Text, S::Extensions>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a string or ExpressionType")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Self::Value, E> {
        S::owned_text(text.into()).map(TypeFields::Name)
    }

    fn visit_borrowed_str<E: de::Error>(self, text: &'de str) -> Result<Self::Value, E> {
        S::borrowed_text(text).map(TypeFields::Name)
    }

    fn visit_string<E: de::Error>(self, text: String) -> Result<Self::Value, E> {
        S::owned_text(text).map(TypeFields::Name)
    }

    fn visit_bytes<E: de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
        match std::str::from_utf8(bytes) {
            Ok(text) => self.visit_str(text),
            Err(_) => Err(E::invalid_value(de::Unexpected::Bytes(bytes), &"a string")),
        }
    }

    fn visit_borrowed_bytes<E: de::Error>(self, bytes: &'de [u8]) -> Result<Self::Value, E> {
        match std::str::from_utf8(bytes) {
            Ok(text) => self.visit_borrowed_str(text),
            Err(_) => Err(E::invalid_value(de::Unexpected::Bytes(bytes), &"a string")),
        }
    }

    fn visit_byte_buf<E: de::Error>(self, bytes: Vec<u8>) -> Result<Self::Value, E> {
        self.visit_bytes(&bytes)
    }

    fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
        ExpressionVisitor::<S>(PhantomData)
            .visit_map(map)
            .map(TypeFields::Object)
    }
}

struct OwnedType(TypeFields<String, OwnedExtensions>);

impl<'de> Deserialize<'de> for OwnedType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer
            .deserialize_any(TypeVisitor::<Owned>(PhantomData))
            .map(Self)
    }
}

impl ExpressionFields<String, OwnedExtensions> {
    fn into_model(self) -> ExpressionType {
        ExpressionType {
            type_: self.type_,
            version: self.version,
            extensions: self.extensions.values,
        }
    }
}

impl TypeFields<String, OwnedExtensions> {
    fn into_model(self) -> SelectorType {
        match self {
            Self::Name(name) => SelectorType::Name(name),
            Self::Object(expression) => SelectorType::ExpressionType(expression.into_model()),
        }
    }
}

impl<'de> Deserialize<'de> for SelectorObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = deserializer.deserialize_map(SelectorVisitor::<Owned>(PhantomData))?;
        Ok(Self {
            context: fields.context,
            selector: fields.selector,
            type_: fields.type_.into_model(),
            extensions: fields.extensions.values,
        })
    }
}

impl<'de> Deserialize<'de> for ExpressionType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer
            .deserialize_map(ExpressionVisitor::<Owned>(PhantomData))
            .map(ExpressionFields::into_model)
    }
}

impl<'de> Deserialize<'de> for SelectorType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Owned::selector_type(deserializer).map(TypeFields::into_model)
    }
}

#[cfg(test)]
mod tests;
