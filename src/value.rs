//! A tiny document tree that JSON, TOML, XML, INI and YAML all collapse into,
//! so the rest of the program reads mod metadata without caring which format it
//! arrived in.
//!
//! Numbers and booleans are flattened to strings on load: every field we ever
//! read (id, version, dependency name, version requirement) is textual, and a
//! version like `1.0` must not be mangled into a float.

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};

use crate::limits;

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
    /// `key = value` under `[section]` headers. Flat by nature: a section is a
    /// map of strings, section-less keys sit at the top. Unlocks 7 Days to Die,
    /// several Unity titles, and parts of the Paradox games.
    Ini,
    /// YAML manifests, mapped the same way as JSON. The parser (libyaml) caps
    /// its own nesting, so — like TOML — no depth guard is needed here.
    Yaml,
}

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// Every text file the tool reads goes through here, so this is the one place
/// that has to cope with a UTF-8 BOM — plenty of real mod metadata and load
/// order files carry one, and every parser below chokes on it.
pub fn load(bytes: &[u8], format: Format) -> Result<Value> {
    let bytes = bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).context("file is not valid UTF-8")?;
    match format {
        // JSONC, not strict JSON: real Minecraft manifests (fabric.mod.json,
        // pack.mcmeta) carry `//` comments and trailing commas, and the modders
        // who write them expect the game's lenient loader. serde_json's strict
        // parser rejects them, so the whole mod falls back to its filename and
        // the folder looks cleaner than it is — a silent false negative.
        Format::Json => {
            // Checked before parsing, like XML: the JSONC parser recurses while
            // parsing and does not cap its own depth, so a deeply nested
            // document overflows the stack and aborts the process before any
            // error could be caught. serde_json used to guard this for us; its
            // lenient replacement does not.
            let depth = limits::max_json_depth(text);
            if depth > limits::MAX_DOCUMENT_DEPTH {
                bail!(
                    "JSON nested about {depth} levels deep, past the limit of {}. \
                     No real manifest is, and parsing it would crash the process.",
                    limits::MAX_DOCUMENT_DEPTH
                );
            }
            let value = jsonc_parser::parse_to_serde_value(text, &Default::default())
                .context("invalid JSON")?
                .context("JSON file has no value")?;
            Ok(from_json(&value))
        }
        Format::Toml => Ok(from_toml(&text.parse().context("invalid TOML")?)),
        // Flat, so no depth guard: INI cannot nest deep enough to overflow.
        Format::Ini => {
            let ini = ini::Ini::load_from_str(text).context("invalid INI")?;
            Ok(from_ini(&ini))
        }
        // libyaml caps its own recursion (a deeply nested document errors
        // rather than overflowing), so no depth guard is needed.
        Format::Yaml => Ok(from_yaml(
            &serde_yaml_ng::from_str(text).context("invalid YAML")?,
        )),
        Format::Xml => {
            // Checked before parsing, not after: roxmltree recurses while
            // parsing, and a stack overflow aborts the process rather than
            // unwinding into an error anyone could catch.
            let depth = limits::max_xml_depth(text);
            if depth > limits::MAX_DOCUMENT_DEPTH {
                bail!(
                    "XML nested about {depth} levels deep, past the limit of {}.                      No real manifest is, and parsing it would crash the process.",
                    limits::MAX_DOCUMENT_DEPTH
                );
            }

            let options = roxmltree::ParsingOptions {
                nodes_limit: limits::MAX_XML_NODES,
                ..Default::default()
            };
            let doc =
                roxmltree::Document::parse_with_options(text, options).context("invalid XML")?;
            let root = doc.root_element();
            let mut map = BTreeMap::new();
            map.insert(root.tag_name().name().to_string(), from_xml(root, 0)?);
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
        serde_json::Value::Object(o) => {
            Value::Map(o.iter().map(|(k, v)| (k.clone(), from_json(v))).collect())
        }
        serde_json::Value::String(s) => Value::Str(s.clone()),
        serde_json::Value::Null => Value::Str(String::new()),
        scalar => Value::Str(scalar.to_string()),
    }
}

/// Same shape as `from_json`: scalars flatten to strings so a version like
/// `1.0` survives as text. Non-string keys (rare in a manifest) are stringified.
fn from_yaml(value: &serde_yaml_ng::Value) -> Value {
    use serde_yaml_ng::Value as Y;
    match value {
        Y::Sequence(s) => Value::List(s.iter().map(from_yaml).collect()),
        Y::Mapping(m) => Value::Map(m.iter().map(|(k, v)| (yaml_key(k), from_yaml(v))).collect()),
        Y::String(s) => Value::Str(s.clone()),
        Y::Bool(b) => Value::Str(b.to_string()),
        Y::Number(n) => Value::Str(n.to_string()),
        Y::Null => Value::Str(String::new()),
        // A tagged node (`!Foo value`) carries a tag we do not use; keep the value.
        Y::Tagged(t) => from_yaml(&t.value),
    }
}

fn yaml_key(key: &serde_yaml_ng::Value) -> String {
    use serde_yaml_ng::Value as Y;
    match key {
        Y::String(s) => s.clone(),
        Y::Bool(b) => b.to_string(),
        Y::Number(n) => n.to_string(),
        // A non-scalar key is not something a real manifest uses.
        _ => String::new(),
    }
}

/// Named sections become nested maps (`section.key`); section-less keys sit at
/// the top level (`key`). Every value is a string, which is all INI has.
fn from_ini(ini: &ini::Ini) -> Value {
    let mut root: BTreeMap<String, Value> = BTreeMap::new();
    for (section, props) in ini.iter() {
        let map: BTreeMap<String, Value> = props
            .iter()
            .map(|(k, v)| (k.to_string(), Value::Str(v.to_string())))
            .collect();
        match section {
            Some(name) => {
                root.insert(name.to_string(), Value::Map(map));
            }
            None => root.extend(map),
        }
    }
    Value::Map(root)
}

fn from_toml(value: &toml::Value) -> Value {
    match value {
        toml::Value::Array(a) => Value::List(a.iter().map(from_toml).collect()),
        toml::Value::Table(t) => {
            Value::Map(t.iter().map(|(k, v)| (k.clone(), from_toml(v))).collect())
        }
        toml::Value::String(s) => Value::Str(s.clone()),
        scalar => Value::Str(scalar.to_string()),
    }
}

/// Attributes become `@name` keys, child elements become keys by tag name
/// (repeated tags collapse into a list), and text content becomes the value
/// itself when the element has nothing else.
fn from_xml(node: roxmltree::Node, depth: usize) -> Result<Value> {
    // roxmltree itself parses into a flat arena and does not care how deep the
    // document is; this conversion recurses, so the limit lives here.
    if depth > limits::MAX_DOCUMENT_DEPTH {
        bail!(
            "XML nested deeper than {} levels, which no real manifest is",
            limits::MAX_DOCUMENT_DEPTH
        );
    }

    let text = node.text().map(str::trim).unwrap_or_default();
    let has_children = node.children().any(|c| c.is_element());
    let has_attributes = node.attributes().next().is_some();

    if !has_children && !has_attributes {
        return Ok(Value::Str(text.to_string()));
    }

    let mut map: BTreeMap<String, Value> = BTreeMap::new();
    for attr in node.attributes() {
        map.insert(
            format!("@{}", attr.name()),
            Value::Str(attr.value().to_string()),
        );
    }
    if !text.is_empty() {
        map.insert("#text".to_string(), Value::Str(text.to_string()));
    }
    for child in node.children().filter(|c| c.is_element()) {
        let key = child.tag_name().name().to_string();
        let value = from_xml(child, depth + 1)?;
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
    Ok(Value::Map(map))
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
    fn reads_json_with_comments_and_trailing_commas() {
        // A real fabric.mod.json shape: line and block comments, plus a
        // trailing comma. serde_json rejects all of these.
        let jsonc = br#"{
            // the mod's id
            "id": "alpha",
            "version": "1.0.0", /* semantic version */
            "depends": {
                "fabricloader": ">=0.14",
            },
        }"#;
        let v = load(jsonc, Format::Json).unwrap();
        assert_eq!(v.get_str("id"), Some("alpha"));
        assert_eq!(v.get_str("version"), Some("1.0.0"));
        assert_eq!(v.get_str("depends.fabricloader"), Some(">=0.14"));
    }

    #[test]
    fn a_json_file_with_only_a_comment_is_an_error() {
        assert!(load(b"// nothing here\n", Format::Json).is_err());
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
    fn reads_a_field_from_yaml() {
        let yaml = b"id: alpha\nversion: \"1.0\"\ndeps:\n  - fabric\n  - forge\n";
        let v = load(yaml, Format::Yaml).unwrap();
        assert_eq!(v.get_str("id"), Some("alpha"));
        assert_eq!(v.get_str("version"), Some("1.0"));
        let deps: Vec<_> = v
            .get("deps")
            .unwrap()
            .items()
            .into_iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(deps, vec!["fabric", "forge"]);
    }

    #[test]
    fn reads_a_nested_yaml_field_by_dotted_path() {
        let yaml = b"mod:\n  meta:\n    name: deep\n";
        let v = load(yaml, Format::Yaml).unwrap();
        assert_eq!(v.get_str("mod.meta.name"), Some("deep"));
    }

    #[test]
    fn deeply_nested_yaml_is_rejected_rather_than_overflowing() {
        // libyaml caps its own nesting depth, so this errors, it does not crash.
        let yaml = format!("{}1{}", "[".repeat(100_000), "]".repeat(100_000));
        assert!(load(yaml.as_bytes(), Format::Yaml).is_err());
    }

    #[test]
    fn reads_a_field_from_ini() {
        // A section-less key must precede any header, or it joins the section.
        let ini = b"name = top-level\n\n[mod]\nid = alpha\nversion = 1.0\n";
        let v = load(ini, Format::Ini).unwrap();
        assert_eq!(v.get_str("mod.id"), Some("alpha"));
        assert_eq!(v.get_str("mod.version"), Some("1.0"));
        // A section-less key is reachable at the top level.
        assert_eq!(v.get_str("name"), Some("top-level"));
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
    fn xml_nested_past_the_limit_is_an_error_rather_than_a_stack_overflow() {
        let depth = limits::MAX_DOCUMENT_DEPTH + 5;
        let xml = format!("{}x{}", "<a>".repeat(depth), "</a>".repeat(depth));

        let err = load(xml.as_bytes(), Format::Xml).unwrap_err();

        assert!(format!("{err:#}").contains("past the limit"), "{err:#}");
    }

    #[test]
    fn xml_nested_within_the_limit_still_parses() {
        let depth = 20;
        let xml = format!("{}deep{}", "<a>".repeat(depth), "</a>".repeat(depth));

        let value = load(xml.as_bytes(), Format::Xml).unwrap();

        assert_eq!(value.get_str(&vec!["a"; depth].join(".")), Some("deep"));
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert!(load(b"{ not json", Format::Json).is_err());
        assert!(load(b"<unclosed>", Format::Xml).is_err());
    }
}
