//! A tiny document tree that JSON, TOML and XML all collapse into, so the rest
//! of the program reads mod metadata without caring which format it arrived in.
//!
//! Numbers and booleans are flattened to strings on load: every field we ever
//! read (id, version, dependency name, version requirement) is textual, and a
//! version like `1.0` must not be mangled into a float.

use std::collections::BTreeMap;

use anyhow::{Context, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Json,
    Toml,
    Xml,
}

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// Every text file the tool reads goes through here, so this is the one place
/// that has to cope with a UTF-8 BOM — plenty of real mod metadata and load
/// order files carry one, and every parser below chokes on it.
pub fn load(bytes: &[u8], format: Format) -> Result<Value> {
    let bytes = bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).context("file is not valid UTF-8")?;
    match format {
        Format::Json => Ok(from_json(
            &serde_json::from_str(text).context("invalid JSON")?,
        )),
        Format::Toml => Ok(from_toml(&text.parse().context("invalid TOML")?)),
        Format::Xml => {
            let doc = roxmltree::Document::parse(text).context("invalid XML")?;
            let root = doc.root_element();
            let mut map = BTreeMap::new();
            map.insert(root.tag_name().name().to_string(), from_xml(root));
            Ok(Value::Map(map))
        }
    }
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Everything reachable by a dotted path. A `*` segment fans out over all
    /// values of a map or all items of a list — which is how Forge's
    /// `dependencies.<your-own-mod-id>` tables are reached without knowing the
    /// key in advance.
    pub fn get_all(&self, path: &str) -> Vec<&Value> {
        let mut current = vec![self];
        for segment in path.split('.').filter(|s| !s.is_empty()) {
            let mut next = Vec::new();
            for value in current {
                match (segment, value) {
                    ("*", Value::Map(m)) => next.extend(m.values()),
                    ("*", Value::List(l)) => next.extend(l.iter()),
                    (key, Value::Map(m)) => next.extend(m.get(key)),
                    // Reaching into a list by name maps over its items, so a
                    // repeated XML element behaves like a single one.
                    (key, Value::List(l)) => {
                        for item in l {
                            if let Value::Map(m) = item {
                                next.extend(m.get(key));
                            }
                        }
                    }
                    _ => {}
                }
            }
            current = next;
        }
        current
    }

    pub fn get(&self, path: &str) -> Option<&Value> {
        self.get_all(path).into_iter().next()
    }

    pub fn get_str(&self, path: &str) -> Option<&str> {
        self.get(path).and_then(Value::as_str)
    }

    /// Flatten to a list of items, treating a lone value as a one-item list.
    pub fn items(&self) -> Vec<&Value> {
        match self {
            Value::List(l) => l.iter().collect(),
            other => vec![other],
        }
    }
}

fn from_json(value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Array(a) => Value::List(a.iter().map(from_json).collect()),
        serde_json::Value::Object(o) => Value::Map(
            o.iter()
                .map(|(k, v)| (k.clone(), from_json(v)))
                .collect(),
        ),
        serde_json::Value::String(s) => Value::Str(s.clone()),
        serde_json::Value::Null => Value::Str(String::new()),
        scalar => Value::Str(scalar.to_string()),
    }
}

fn from_toml(value: &toml::Value) -> Value {
    match value {
        toml::Value::Array(a) => Value::List(a.iter().map(from_toml).collect()),
        toml::Value::Table(t) => Value::Map(
            t.iter()
                .map(|(k, v)| (k.clone(), from_toml(v)))
                .collect(),
        ),
        toml::Value::String(s) => Value::Str(s.clone()),
        scalar => Value::Str(scalar.to_string()),
    }
}

/// Attributes become `@name` keys, child elements become keys by tag name
/// (repeated tags collapse into a list), and text content becomes the value
/// itself when the element has nothing else.
fn from_xml(node: roxmltree::Node) -> Value {
    let text = node.text().map(str::trim).unwrap_or_default();
    let has_children = node.children().any(|c| c.is_element());
    let has_attributes = node.attributes().next().is_some();

    if !has_children && !has_attributes {
        return Value::Str(text.to_string());
    }

    let mut map: BTreeMap<String, Value> = BTreeMap::new();
    for attr in node.attributes() {
        map.insert(format!("@{}", attr.name()), Value::Str(attr.value().to_string()));
    }
    if !text.is_empty() {
        map.insert("#text".to_string(), Value::Str(text.to_string()));
    }
    for child in node.children().filter(|c| c.is_element()) {
        let key = child.tag_name().name().to_string();
        let value = from_xml(child);
        match map.remove(&key) {
            Some(Value::List(mut existing)) => {
                existing.push(value);
                map.insert(key, Value::List(existing));
            }
            Some(first) => {
                map.insert(key, Value::List(vec![first, value]));
            }
            None => {
                map.insert(key, value);
            }
        }
    }
    Value::Map(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_field_from_json() {
        let v = load(br#"{"name":"alpha","version":"1.0.0"}"#, Format::Json).unwrap();
        assert_eq!(v.get_str("name"), Some("alpha"));
    }

    #[test]
    fn keeps_numeric_versions_as_written() {
        let v = load(br#"{"version":1.10}"#, Format::Json).unwrap();
        assert_eq!(v.get_str("version"), Some("1.1"));
    }

    #[test]
    fn reads_a_nested_field_by_dotted_path() {
        let v = load(br#"{"a":{"b":{"c":"deep"}}}"#, Format::Json).unwrap();
        assert_eq!(v.get_str("a.b.c"), Some("deep"));
    }

    #[test]
    fn reads_a_field_from_toml() {
        let v = load(b"[[mods]]\nmodId=\"alpha\"\nversion=\"1.0\"", Format::Toml).unwrap();
        assert_eq!(v.get_str("mods.modId"), Some("alpha"));
    }

    #[test]
    fn a_wildcard_fans_out_over_map_values() {
        let v = load(
            br#"{"dependencies":{"mymod":[{"modId":"forge"},{"modId":"minecraft"}]}}"#,
            Format::Json,
        )
        .unwrap();

        let ids: Vec<_> = v
            .get_all("dependencies.*.*.modId")
            .into_iter()
            .filter_map(Value::as_str)
            .collect();

        assert_eq!(ids, vec!["forge", "minecraft"]);
    }

    #[test]
    fn reads_xml_elements_and_attributes() {
        let xml = br#"<modDesc descVersion="60"><version>1.2.3</version></modDesc>"#;
        let v = load(xml, Format::Xml).unwrap();

        assert_eq!(v.get_str("modDesc.@descVersion"), Some("60"));
        assert_eq!(v.get_str("modDesc.version"), Some("1.2.3"));
    }

    #[test]
    fn repeated_xml_tags_become_a_list() {
        let xml = br#"<modDesc><dependencies>
            <dependency>a</dependency><dependency>b</dependency>
        </dependencies></modDesc>"#;
        let v = load(xml, Format::Xml).unwrap();

        let deps: Vec<_> = v
            .get("modDesc.dependencies.dependency")
            .unwrap()
            .items()
            .into_iter()
            .filter_map(Value::as_str)
            .collect();

        assert_eq!(deps, vec!["a", "b"]);
    }

    #[test]
    fn a_single_xml_tag_still_reads_as_a_one_item_list() {
        let xml = br#"<modDesc><dependencies><dependency>a</dependency></dependencies></modDesc>"#;
        let v = load(xml, Format::Xml).unwrap();

        let deps = v.get("modDesc.dependencies.dependency").unwrap().items();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].as_str(), Some("a"));
    }

    #[test]
    fn a_missing_path_yields_nothing_rather_than_failing() {
        let v = load(br#"{"a":1}"#, Format::Json).unwrap();
        assert_eq!(v.get("nope.nope"), None);
    }

    #[test]
    fn a_utf8_bom_does_not_break_parsing() {
        let with_bom = format!("\u{feff}{}", r#"{"name":"alpha"}"#);

        let v = load(with_bom.as_bytes(), Format::Json).unwrap();

        assert_eq!(v.get_str("name"), Some("alpha"));
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert!(load(b"{ not json", Format::Json).is_err());
        assert!(load(b"<unclosed>", Format::Xml).is_err());
    }
}
