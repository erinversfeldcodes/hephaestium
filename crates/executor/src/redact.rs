//! Keeping the value of a secret the executor holds out of everything it says.
//!
//! Every string in an event and in the run record has each held value replaced by
//! a marker that names the secret. Only the exact value is found: a secret printed
//! in pieces, encoded or changed in any other way is not, and is not claimed to be.

use serde_json::Value;

/// A value shorter than this is left alone: it would damage ordinary text, and a
/// secret that short is not one worth the marker.
pub const SHORTEST: usize = 8;

/// The secrets held, by name, ready to be taken out of text.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    held: Vec<(String, String)>,
}

impl Redactor {
    /// A redactor for these `(name, value)` pairs. A value too short to redact is
    /// not kept.
    #[must_use]
    pub fn new<'a>(secrets: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut held: Vec<(String, String)> = secrets
            .into_iter()
            .filter(|(_, value)| value.chars().count() >= SHORTEST)
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        // The longest first, so that a value that holds another is taken out whole.
        held.sort_by_key(|(_, value)| std::cmp::Reverse(value.len()));
        Self { held }
    }

    /// Whether there is nothing to take out.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// The text with each held value replaced by `[redacted: NAME]`.
    #[must_use]
    pub fn text(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for (name, value) in &self.held {
            if text.contains(value.as_str()) {
                text = text.replace(value.as_str(), &format!("[redacted: {name}]"));
            }
        }
        text
    }

    /// Replace the held values in every string in `value`, its object keys
    /// included. Returns whether anything was replaced, so that what holds no
    /// secret is written exactly as it was.
    pub fn json(&self, value: &mut Value) -> bool {
        match value {
            Value::String(text) => {
                let redacted = self.text(text);
                let changed = redacted != *text;
                if changed {
                    *text = redacted;
                }
                changed
            }
            Value::Array(items) => items
                .iter_mut()
                .fold(false, |changed, item| self.json(item) | changed),
            Value::Object(map) => {
                let mut changed = false;
                let mut rebuilt = serde_json::Map::with_capacity(map.len());
                for (key, mut item) in std::mem::take(map) {
                    changed |= self.json(&mut item);
                    let redacted = self.text(&key);
                    changed |= redacted != key;
                    rebuilt.insert(redacted, item);
                }
                *map = rebuilt;
                changed
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TOKEN: &str = "sk-ant-oat01-abcdef";

    fn redactor() -> Redactor {
        Redactor::new([
            ("CLAUDE", TOKEN),
            ("SHORT", "1234567"),
            ("EXACT", "12345678"),
        ])
    }

    #[test]
    fn a_value_is_replaced_by_the_marker_that_names_it_wherever_it_is_in_the_text() {
        assert_eq!(
            redactor().text(&format!("a {TOKEN} b {TOKEN}")),
            "a [redacted: CLAUDE] b [redacted: CLAUDE]"
        );
    }

    #[test]
    fn a_value_is_redacted_from_eight_characters_and_not_before() {
        assert_eq!(
            redactor().text("1234567 and 12345678"),
            "1234567 and [redacted: EXACT]"
        );
    }

    #[test]
    fn a_value_that_holds_another_is_taken_out_whole() {
        let redactor = Redactor::new([("INNER", "abcdefgh"), ("OUTER", "abcdefghijkl")]);
        assert_eq!(redactor.text("abcdefghijkl"), "[redacted: OUTER]");
    }

    #[test]
    fn every_string_in_json_is_redacted_the_keys_too_and_what_holds_none_is_said_so() {
        let mut value = json!({
            "a": [format!("x{TOKEN}"), 3, null, true],
            "b": {"c": {TOKEN: "d"}},
        });
        assert!(redactor().json(&mut value));
        assert_eq!(
            value,
            json!({
                "a": ["x[redacted: CLAUDE]", 3, null, true],
                "b": {"c": {"[redacted: CLAUDE]": "d"}},
            })
        );

        let mut plain = json!({"a": ["nothing", 1], "b": {"c": "here"}});
        let before = plain.clone();
        assert!(!redactor().json(&mut plain));
        assert_eq!(plain, before);
    }

    #[test]
    fn nothing_is_held_when_every_value_is_too_short() {
        assert!(Redactor::new([("A", "short")]).is_empty());
    }
}
