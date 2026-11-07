use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::fmt;

pub fn empty_string_as_none<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    Ok(opt.filter(|s| !s.is_empty()))
}

/// Deserialize `Option<Option<T>>`: absent (`None`) leaves, `null` (`Some(None)`) clears, a value sets.
/// Requires `#[serde(default, deserialize_with = "…")]`.
pub fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Ok(Some(Option::deserialize(deserializer)?))
}

/// Chat content as a string, `null`, or a list of text parts.
///
/// Mistral (and some OpenAI-compatible hosts) return
/// `[{"type":"text","text":"..."}]` where the OpenAI schema uses a string.
pub fn string_or_text_parts<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(TextPartsVisitor)
}

/// Tool-call `arguments` as a JSON string or a JSON object/array.
pub fn string_or_json<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    string_or_json_opt(deserializer).map(|value| value.unwrap_or_default())
}

/// Same as [`string_or_json`], preserving absence and JSON null as `None`.
pub fn string_or_json_opt<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(JsonStringVisitor)
}

struct TextPartsVisitor;

impl<'de> Visitor<'de> for TextPartsVisitor {
    type Value = Option<String>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a string, null, or a list of text parts")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(Some(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(Some(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut text = String::new();
        while let Some(part) = seq.next_element::<serde_json::Value>()? {
            append_text_part(&mut text, &part);
        }
        Ok(Some(text))
    }
}

fn append_text_part(text: &mut String, part: &serde_json::Value) {
    match part {
        serde_json::Value::String(value) => text.push_str(value),
        serde_json::Value::Object(map) => {
            for key in ["text", "content", "thinking", "reasoning"] {
                if let Some(value) = map.get(key).and_then(|v| v.as_str()) {
                    text.push_str(value);
                    return;
                }
            }
        }
        _ => {}
    }
}

struct JsonStringVisitor;

impl<'de> Visitor<'de> for JsonStringVisitor {
    type Value = Option<String>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a string or a JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(Some(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(Some(value))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut object = serde_json::Map::new();
        while let Some((key, value)) = map.next_entry::<String, serde_json::Value>()? {
            object.insert(key, value);
        }
        Ok(Some(serde_json::Value::Object(object).to_string()))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::new();
        while let Some(value) = seq.next_element::<serde_json::Value>()? {
            items.push(value);
        }
        Ok(Some(serde_json::Value::Array(items).to_string()))
    }
}

#[cfg(test)]
mod tests {
    use crate::ChatCompletionChunk;

    #[test]
    fn chunk_content_may_be_a_list_of_text_parts() {
        let raw = r#"{"id":"c","object":"chat.completion.chunk","created":1,"model":"large-model-latest","choices":[{"index":0,"delta":{"content":[{"type":"text","text":"Hello"}]},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(raw).unwrap();
        let choice = chunk.choices.first().unwrap();
        assert_eq!(choice.delta.content.as_deref(), Some("Hello"));
    }

    #[test]
    fn tool_arguments_may_be_an_object() {
        let raw = r#"{"id":"c","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call","type":"function","function":{"name":"do","arguments":{"path":"a"}}}]}}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(raw).unwrap();
        let call = chunk
            .choices
            .first()
            .and_then(|choice| choice.delta.tool_calls.first())
            .unwrap();
        let args = call.function.as_ref().and_then(|function| function.arguments.as_deref());
        assert_eq!(args, Some(r#"{"path":"a"}"#));
    }
}
