use serde_yaml_ng::Value;

pub(super) const LIMIT: usize = 1000;

#[derive(Default)]
pub(super) struct Budget {
    used: usize,
    pub(super) trimmed: bool,
}

impl Budget {
    pub(super) fn charge(&mut self, units: usize) -> bool {
        if self.trimmed || units > LIMIT - self.used {
            self.trimmed = true;
            return false;
        }
        self.used += units;
        true
    }

    pub(super) fn remaining(&self) -> usize {
        LIMIT - self.used
    }

    pub(super) fn literal(&mut self, value: &Value) -> bool {
        let available = self.remaining() + 1;
        let mut remaining = available;
        if yaml_cost(value, &mut remaining) {
            self.charge(available - remaining - 1)
        } else {
            self.trimmed = true;
            false
        }
    }
}

fn subtract(remaining: &mut usize, cost: usize) -> bool {
    if cost > *remaining {
        return false;
    }
    *remaining -= cost;
    true
}

fn yaml_cost(value: &Value, remaining: &mut usize) -> bool {
    if !subtract(remaining, 1) {
        return false;
    }
    match value {
        Value::String(s) => subtract(remaining, s.chars().take(*remaining + 1).count()),
        Value::Sequence(values) => values.iter().all(|v| yaml_cost(v, remaining)),
        Value::Mapping(values) => values.iter().all(|(key, value)| {
            let Some(key) = key.as_str() else {
                return false;
            };
            subtract(remaining, key.chars().take(*remaining + 1).count())
                && yaml_cost(value, remaining)
        }),
        Value::Tagged(value) => yaml_cost(&value.value, remaining),
        _ => true,
    }
}

pub(super) fn json_cost(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::String(s) => 1 + s.chars().count(),
        serde_json::Value::Array(values) => 1 + values.iter().map(json_cost).sum::<usize>(),
        serde_json::Value::Object(values) => {
            1 + values
                .iter()
                .map(|(k, v)| k.chars().count() + json_cost(v))
                .sum::<usize>()
        }
        _ => 1,
    }
}
