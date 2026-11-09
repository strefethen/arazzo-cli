use std::cell::Cell;
use std::fmt;

use serde::de::{
    self, DeserializeSeed, Deserializer, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor,
};
use serde_yaml_ng::mapping::Entry;
use serde_yaml_ng::value::{Tag, TaggedValue};
use serde_yaml_ng::{Mapping, Sequence, Value};

const YAML_NODES_PER_INPUT_BYTE: usize = 16;
const YAML_NODE_ALLOWANCE: usize = 1024;

pub fn yaml_node_budget(input_bytes: usize) -> usize {
    input_bytes
        .saturating_mul(YAML_NODES_PER_INPUT_BYTE)
        .saturating_add(YAML_NODE_ALLOWANCE)
}

pub fn parse_yaml_value(data: &[u8]) -> Result<Value, YamlAdmissionError> {
    parse_yaml_value_within(data, yaml_node_budget(data.len()))
}

fn parse_yaml_value_within(data: &[u8], budget: usize) -> Result<Value, YamlAdmissionError> {
    let meter = NodeMeter {
        budget,
        input_bytes: data.len(),
        spent: Cell::new(0),
    };
    BudgetedValue(&meter)
        .deserialize(serde_yaml_ng::Deserializer::from_slice(data))
        .map_err(|error| YamlAdmissionError {
            error,
            node_budget_exceeded: meter.exceeded(),
        })
}

#[derive(Debug)]
pub struct YamlAdmissionError {
    error: serde_yaml_ng::Error,
    node_budget_exceeded: bool,
}

impl YamlAdmissionError {
    pub fn is_node_budget_exceeded(&self) -> bool {
        self.node_budget_exceeded
    }
}

impl fmt::Display for YamlAdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for YamlAdmissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl From<YamlAdmissionError> for serde_yaml_ng::Error {
    fn from(admission: YamlAdmissionError) -> Self {
        admission.error
    }
}

struct NodeMeter {
    budget: usize,
    input_bytes: usize,
    spent: Cell<usize>,
}

impl NodeMeter {
    fn spend<E: de::Error>(&self) -> Result<(), E> {
        let spent = self.spent.get().saturating_add(1);
        self.spent.set(spent);
        if self.exceeded() {
            return Err(E::custom(NodeBudgetExceeded {
                budget: self.budget,
                input_bytes: self.input_bytes,
            }));
        }
        Ok(())
    }

    fn exceeded(&self) -> bool {
        self.spent.get() > self.budget
    }
}

struct NodeBudgetExceeded {
    budget: usize,
    input_bytes: usize,
}

impl fmt::Display for NodeBudgetExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "YAML node budget exceeded: alias expansion yields more than {} nodes from {} input \
             bytes (limit: {} nodes per input byte plus {})",
            self.budget, self.input_bytes, YAML_NODES_PER_INPUT_BYTE, YAML_NODE_ALLOWANCE
        )
    }
}

#[derive(Clone, Copy)]
struct BudgetedValue<'m>(&'m NodeMeter);

impl<'de> DeserializeSeed<'de> for BudgetedValue<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        self.0.spend()?;
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for BudgetedValue<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any YAML value")
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
        Ok(Value::Number(value.into()))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Value, A::Error> {
        let mut sequence = Sequence::new();
        while let Some(item) = items.next_element_seed(self)? {
            sequence.push(item);
        }
        Ok(Value::Sequence(sequence))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut entries: A) -> Result<Value, A::Error> {
        let mut mapping = Mapping::new();
        while let Some(key) = entries.next_key_seed(self)? {
            match mapping.entry(key) {
                Entry::Occupied(entry) => {
                    return Err(de::Error::custom(DuplicateKey(entry.key())));
                }
                Entry::Vacant(entry) => {
                    entry.insert(entries.next_value_seed(self)?);
                }
            }
        }
        Ok(Value::Mapping(mapping))
    }

    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Value, A::Error> {
        let (tag, contents) = data.variant_seed(TagName)?;
        let value = contents.newtype_variant_seed(self)?;
        Ok(Value::Tagged(Box::new(TaggedValue { tag, value })))
    }
}

struct DuplicateKey<'a>(&'a Value);

impl fmt::Display for DuplicateKey<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("duplicate entry ")?;
        match self.0 {
            Value::Null => f.write_str("with null key"),
            Value::Bool(boolean) => write!(f, "with key `{boolean}`"),
            Value::Number(number) => write!(f, "with key {number}"),
            Value::String(string) => write!(f, "with key {string:?}"),
            Value::Sequence(_) | Value::Mapping(_) | Value::Tagged(_) => f.write_str("in YAML map"),
        }
    }
}

struct TagName;

impl<'de> DeserializeSeed<'de> for TagName {
    type Value = Tag;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Tag, D::Error> {
        deserializer.deserialize_string(self)
    }
}

impl<'de> Visitor<'de> for TagName {
    type Value = Tag;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a YAML tag string")
    }

    fn visit_str<E: de::Error>(self, tag: &str) -> Result<Tag, E> {
        self.visit_string(tag.to_owned())
    }

    fn visit_string<E: de::Error>(self, tag: String) -> Result<Tag, E> {
        if tag.is_empty() {
            return Err(E::custom("empty YAML tag is not allowed"));
        }
        Ok(Tag::new(tag))
    }
}

#[cfg(test)]
mod tests;
