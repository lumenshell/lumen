//! Plugin settings: what a plugin's `plugin.toml` says it has
//! (`[settings.KEY]` tables), and the values you give it in `config.toml`
//! (`[plugin.NAME]`), checked against that and filled in with defaults.

use serde_json::{Map, Value};
use toml_edit::{DocumentMut, Item, Table};

/// What kind of value a setting takes.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    String,
    Integer {
        min: Option<i64>,
        max: Option<i64>,
    },
    Number {
        min: Option<f64>,
        max: Option<f64>,
    },
    Boolean,
    Choice(Vec<String>),
    /// A list of strings.
    List,
}

/// One setting a plugin has.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub key: String,
    pub kind: Kind,
    pub default: Value,
    pub description: String,
}

/// The `[settings.KEY]` tables of a `plugin.toml`, in file order. The error
/// names the setting and says what's wrong, for the banner.
pub fn parse_schema(doc: &DocumentMut) -> Result<Vec<Spec>, String> {
    let Some(item) = doc.get("settings") else { return Ok(Vec::new()) };
    let table = item.as_table().ok_or("`settings` should be tables like [settings.name]")?;
    table.iter().map(|(key, item)| spec(key, item).map_err(|e| format!("`settings.{key}`: {e}"))).collect()
}

fn spec(key: &str, item: &Item) -> Result<Spec, String> {
    let t = item.as_table_like().ok_or("should be a table with `type`, `default` and `description`")?;
    let kind_name = t
        .get("type")
        .and_then(Item::as_str)
        .ok_or("`type` should be \"string\", \"integer\", \"number\", \"boolean\", \"choice\" or \"list\"")?;
    let description = t.get("description").and_then(Item::as_str).ok_or("`description` is missing")?.to_string();
    let (min, max) = (t.get("min"), t.get("max"));
    let integer = |i: Option<&Item>, name: &str| match i {
        None => Ok(None),
        Some(i) => i.as_integer().map(Some).ok_or(format!("`{name}` should be a whole number")),
    };
    let number = |i: Option<&Item>, name: &str| match i {
        None => Ok(None),
        Some(i) => i
            .as_float()
            .or_else(|| i.as_integer().map(|n| n as f64))
            .map(Some)
            .ok_or(format!("`{name}` should be a number")),
    };
    let kind = match kind_name {
        "integer" => {
            let (lo, hi) = (integer(min, "min")?, integer(max, "max")?);
            if let (Some(lo), Some(hi)) = (lo, hi)
                && lo > hi
            {
                return Err("`min` is more than `max`".into());
            }
            Kind::Integer { min: lo, max: hi }
        }
        "number" => {
            let (lo, hi) = (number(min, "min")?, number(max, "max")?);
            if let (Some(lo), Some(hi)) = (lo, hi)
                && lo > hi
            {
                return Err("`min` is more than `max`".into());
            }
            Kind::Number { min: lo, max: hi }
        }
        other => {
            if min.is_some() || max.is_some() {
                return Err("`min` and `max` only apply to integer and number settings".into());
            }
            match other {
                "string" => Kind::String,
                "boolean" => Kind::Boolean,
                "list" => Kind::List,
                "choice" => {
                    let choices: Option<Vec<String>> = t
                        .get("choices")
                        .and_then(Item::as_array)
                        .map(|a| a.iter().map(|v| v.as_str().map(String::from)).collect())
                        .unwrap_or(None);
                    match choices {
                        Some(c) if !c.is_empty() => Kind::Choice(c),
                        _ => return Err("`choices` should be a list of the values allowed, like [\"a\", \"b\"]".into()),
                    }
                }
                _ => {
                    return Err(format!(
                        "unknown `type` \"{other}\"; types are \"string\", \"integer\", \"number\", \"boolean\", \"choice\" and \"list\""
                    ));
                }
            }
        }
    };
    let default = t.get("default").ok_or("`default` is missing")?;
    let default = check(&kind, default).ok_or_else(|| format!("`default` should be {}", describe(&kind)))?;
    Ok(Spec { key: key.to_string(), kind, default, description })
}

/// The value as JSON, if it suits `kind`.
fn check(kind: &Kind, item: &Item) -> Option<Value> {
    let v = item.as_value()?;
    let in_range =
        |n: f64, lo: Option<f64>, hi: Option<f64>| lo.is_none_or(|lo| n >= lo) && hi.is_none_or(|hi| n <= hi);
    match kind {
        Kind::String => v.as_str().map(Value::from),
        Kind::Integer { min, max } => {
            let n = v.as_integer()?;
            (min.is_none_or(|lo| n >= lo) && max.is_none_or(|hi| n <= hi)).then(|| Value::from(n))
        }
        Kind::Number { min, max } => {
            // JSON has no infinity or NaN: they'd reach the plugin as null.
            let n = v.as_float().or_else(|| v.as_integer().map(|n| n as f64)).filter(|n| n.is_finite())?;
            in_range(n, *min, *max).then(|| Value::from(n))
        }
        Kind::Boolean => v.as_bool().map(Value::from),
        Kind::Choice(choices) => v.as_str().filter(|s| choices.iter().any(|c| c == s)).map(Value::from),
        Kind::List => {
            let list: Option<Vec<Value>> = v.as_array()?.iter().map(|x| x.as_str().map(Value::from)).collect();
            list.map(Value::Array)
        }
    }
}

/// What a value of `kind` looks like, in words.
fn describe(kind: &Kind) -> String {
    fn range<T: std::fmt::Display>(min: &Option<T>, max: &Option<T>) -> String {
        match (min, max) {
            (Some(lo), Some(hi)) => format!(" from {lo} to {hi}"),
            (Some(lo), None) => format!(" of at least {lo}"),
            (None, Some(hi)) => format!(" of at most {hi}"),
            (None, None) => String::new(),
        }
    }
    match kind {
        Kind::String => "text in quotes".into(),
        Kind::Integer { min, max } => format!("a whole number{}", range(min, max)),
        Kind::Number { min, max } => format!("a number{}", range(min, max)),
        Kind::Boolean => "true or false".into(),
        Kind::Choice(choices) => {
            let names: Vec<String> = choices.iter().map(|c| format!("\"{c}\"")).collect();
            format!("one of {}", names.join(", "))
        }
        Kind::List => "a list of text in quotes, like [\"a\", \"b\"]".into(),
    }
}

/// How the plugin's table is written in config.toml: `[plugin.recent-dirs]`,
/// or quoted when the name needs it, `[plugin."mira/docker-panes"]`.
fn table_name(plugin: &str) -> String {
    let bare = !plugin.is_empty() && plugin.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if bare { format!("[plugin.{plugin}]") } else { format!("[plugin.\"{plugin}\"]") }
}

/// Every setting in `schema`, from `given` (the plugin's `[plugin.NAME]`
/// table) where it's valid and the default otherwise, and a problem for each
/// value that isn't valid and each key the plugin doesn't have.
pub fn resolve(plugin: &str, schema: &[Spec], given: Option<&Table>) -> (Map<String, Value>, Vec<String>) {
    let (mut values, mut problems) = (Map::new(), Vec::new());
    let table = table_name(plugin);
    for spec in schema {
        let value = match given.and_then(|t| t.get(&spec.key)) {
            None => spec.default.clone(),
            Some(item) => check(&spec.kind, item).unwrap_or_else(|| {
                problems.push(format!(
                    "`{table}`: `{}` should be {}; using {}",
                    spec.key,
                    describe(&spec.kind),
                    spec.default
                ));
                spec.default.clone()
            }),
        };
        values.insert(spec.key.clone(), value);
    }
    for (key, _) in given.into_iter().flat_map(Table::iter) {
        if !schema.iter().any(|s| s.key == key) {
            problems.push(if schema.is_empty() {
                format!("`{table}`: unknown setting `{key}` (this plugin has no settings)")
            } else {
                format!("`{table}`: unknown setting `{key}`")
            });
        }
    }
    (values, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(text: &str) -> Result<Vec<Spec>, String> {
        parse_schema(&text.parse().unwrap())
    }

    fn table(text: &str) -> toml_edit::Table {
        text.parse::<toml_edit::DocumentMut>().unwrap().as_table().clone()
    }

    #[test]
    fn schema_parses_each_type() {
        let s = schema(
            r#"
            [settings.name]
            type = "string"
            default = "x"
            description = "d"
            [settings.count]
            type = "integer"
            default = 5
            min = 1
            max = 60
            description = "d"
            [settings.ratio]
            type = "number"
            default = 0.5
            description = "d"
            [settings.on]
            type = "boolean"
            default = true
            description = "d"
            [settings.engine]
            type = "choice"
            choices = ["docker", "podman"]
            default = "docker"
            description = "d"
            [settings.hosts]
            type = "list"
            default = ["a", "b"]
            description = "d"
        "#,
        )
        .unwrap();
        let keys: Vec<&str> = s.iter().map(|x| x.key.as_str()).collect();
        assert_eq!(keys, ["name", "count", "ratio", "on", "engine", "hosts"]);
        assert_eq!(s[1].kind, Kind::Integer { min: Some(1), max: Some(60) });
        assert_eq!(s[1].default, serde_json::json!(5));
        assert_eq!(s[4].kind, Kind::Choice(vec!["docker".into(), "podman".into()]));
        assert_eq!(s[5].default, serde_json::json!(["a", "b"]));
    }

    #[test]
    fn bad_schemas_say_why() {
        let cases = [
            ("[settings.a]\ntype = \"integer\"\ndescription = \"d\"", "default"),
            ("[settings.a]\ntype = \"integer\"\ndefault = 1", "description"),
            ("[settings.a]\ntype = \"colour\"\ndefault = 1\ndescription = \"d\"", "type"),
            ("[settings.a]\ntype = \"integer\"\ndefault = \"5\"\ndescription = \"d\"", "default"),
            ("[settings.a]\ntype = \"integer\"\ndefault = 0\nmin = 1\ndescription = \"d\"", "default"),
            ("[settings.a]\ntype = \"integer\"\ndefault = 5\nmin = 9\nmax = 1\ndescription = \"d\"", "min"),
            ("[settings.a]\ntype = \"choice\"\nchoices = []\ndefault = \"x\"\ndescription = \"d\"", "choices"),
            ("[settings.a]\ntype = \"choice\"\nchoices = [\"x\"]\ndefault = \"y\"\ndescription = \"d\"", "default"),
            ("[settings.a]\ntype = \"string\"\nmin = 1\ndefault = \"x\"\ndescription = \"d\"", "min"),
            ("settings = 5", "settings"),
        ];
        for (text, word) in cases {
            let e = schema(text).unwrap_err();
            assert!(e.contains(word), "{text:?} → {e}");
            assert!(e.contains("`settings"), "names the setting: {e}");
        }
    }

    #[test]
    fn numbers_must_be_finite() {
        let s = schema("[settings.ratio]\ntype = \"number\"\ndefault = 0.5\ndescription = \"d\"").unwrap();
        for bad in ["ratio = inf", "ratio = nan", "ratio = -inf"] {
            let (v, p) = resolve("p", &s, Some(&table(bad)));
            assert_eq!(v["ratio"], serde_json::json!(0.5), "{bad}");
            assert_eq!(p.len(), 1, "{bad}: {p:?}");
        }
        let e = schema("[settings.ratio]\ntype = \"number\"\ndefault = nan\ndescription = \"d\"").unwrap_err();
        assert!(e.contains("default"), "{e}");
    }

    #[test]
    fn resolve_fills_defaults() {
        let s = schema("[settings.count]\ntype = \"integer\"\ndefault = 5\ndescription = \"d\"").unwrap();
        let (values, problems) = resolve("p", &s, None);
        assert!(problems.is_empty());
        assert_eq!(serde_json::Value::Object(values), serde_json::json!({"count": 5}));
    }

    #[test]
    fn resolve_checks_types() {
        let s = schema(
            r#"
            [settings.count]
            type = "integer"
            default = 5
            min = 1
            max = 60
            description = "d"
            [settings.ratio]
            type = "number"
            default = 0.5
            description = "d"
            [settings.engine]
            type = "choice"
            choices = ["docker", "podman"]
            default = "docker"
            description = "d"
        "#,
        )
        .unwrap();
        let (v, p) = resolve("p", &s, Some(&table("count = 10\nratio = 1\nengine = \"podman\"")));
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(serde_json::Value::Object(v), serde_json::json!({"count": 10, "ratio": 1.0, "engine": "podman"}));
        let (v, p) = resolve("p", &s, Some(&table("count = 2.5\nratio = \"x\"\nengine = \"lxc\"\ncolour = 1")));
        assert_eq!(serde_json::Value::Object(v), serde_json::json!({"count": 5, "ratio": 0.5, "engine": "docker"}));
        assert_eq!(p.len(), 4, "{p:?}");
        assert!(p.iter().all(|x| x.starts_with("`[plugin.p]`")), "{p:?}");
        assert!(p.iter().any(|x| x.contains("colour")), "unknown keys are named: {p:?}");
        let (_, p) = resolve("p", &s, Some(&table("count = 99")));
        assert!(p[0].contains('1') && p[0].contains("60"), "says the range: {p:?}");
    }
}
