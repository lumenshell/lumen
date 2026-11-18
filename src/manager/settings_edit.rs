//! Editing one plugin's settings in place: each type of setting has its
//! own control, and a value is checked against the plugin's schema before
//! it's saved.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use serde_json::{Value, json};

use crate::manager::data::Item;
use crate::plugin_settings::{Kind, Spec};

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub spec: Spec,
    /// The value in use.
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Editor {
    pub plugin: String,
    pub fields: Vec<Field>,
    pub selected: usize,
    /// Text being typed into the selected field.
    pub input: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum EditorResult {
    Stay,
    Close,
    /// Save this setting; `None` resets it (removes the key).
    Save(String, Option<Value>),
    /// The value isn't valid; nothing was saved.
    Problem(String),
}

/// A value as config.toml writes it.
pub fn to_toml(v: &Value) -> Option<toml_edit::Value> {
    Some(match v {
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.into(),
            None => n.as_f64()?.into(),
        },
        Value::String(s) => s.as_str().into(),
        Value::Array(list) => {
            let mut a = toml_edit::Array::new();
            for x in list {
                a.push(x.as_str()?);
            }
            a.into()
        }
        _ => return None,
    })
}

/// A value as typed, for showing and editing.
pub fn shown(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(list) => {
            list.iter().map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_string)).collect::<Vec<_>>().join(", ")
        }
        v => v.to_string(),
    }
}

impl Editor {
    /// `None` when the plugin has no settings.
    pub fn new(item: &Item) -> Option<Editor> {
        if item.settings.is_empty() {
            return None;
        }
        let fields = item
            .settings
            .iter()
            .map(|s| Field {
                spec: s.clone(),
                value: item.values.get(&s.key).cloned().unwrap_or_else(|| s.default.clone()),
            })
            .collect();
        Some(Editor { plugin: item.name.clone(), fields, selected: 0, input: None })
    }

    pub fn key(&mut self, key: KeyEvent) -> EditorResult {
        let field = &self.fields[self.selected];
        let kind = field.spec.kind.clone();
        let text_kind = matches!(kind, Kind::String | Kind::List);
        if let Some(input) = self.input.as_mut() {
            match key.code {
                KeyCode::Esc => self.input = None,
                KeyCode::Enter => {
                    let text = self.input.take().unwrap_or_default();
                    return self.save(parse(&kind, &text));
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Left | KeyCode::Right if !text_kind => {
                    let base = parse(&kind, input).unwrap_or_else(|| field.value.clone());
                    *input = shown(&step(&kind, &base, key.code == KeyCode::Right));
                }
                KeyCode::Char(c)
                    if text_kind
                        || c.is_ascii_digit()
                        || c == '-'
                        || (c == '.' && matches!(kind, Kind::Number { .. })) =>
                {
                    input.push(c);
                }
                _ => {}
            }
            return EditorResult::Stay;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return EditorResult::Close,
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(self.fields.len() - 1),
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('d') => {
                let f = &mut self.fields[self.selected];
                f.value = f.spec.default.clone();
                return EditorResult::Save(f.spec.key.clone(), None);
            }
            KeyCode::Char(' ') | KeyCode::Enter if kind == Kind::Boolean => {
                let flipped = json!(!field.value.as_bool().unwrap_or(false));
                return self.save(Some(flipped));
            }
            KeyCode::Enter if text_kind => self.input = Some(shown(&field.value)),
            KeyCode::Enter => return self.save(Some(field.value.clone())),
            KeyCode::Left | KeyCode::Right => {
                let up = key.code == KeyCode::Right;
                match &kind {
                    Kind::Choice(_) | Kind::Integer { .. } | Kind::Number { .. } => {
                        let next = step(&kind, &field.value, up);
                        if matches!(kind, Kind::Choice(_)) {
                            self.fields[self.selected].value = next;
                        } else {
                            self.input = Some(shown(&next));
                        }
                    }
                    _ => {}
                }
            }
            KeyCode::Char(c) if !text_kind && (c.is_ascii_digit() || c == '-' || c == '.') => {
                if matches!(kind, Kind::Integer { .. } | Kind::Number { .. }) {
                    self.input = Some(c.to_string());
                }
            }
            _ => {}
        }
        EditorResult::Stay
    }

    /// Check `value` against the schema; keep and save it if it's valid.
    fn save(&mut self, value: Option<Value>) -> EditorResult {
        let spec = self.fields[self.selected].spec.clone();
        let Some(value) = value else {
            return EditorResult::Problem(format!("`{}` needs {}", spec.key, needs(&spec.kind)));
        };
        let Some(toml) = to_toml(&value) else {
            return EditorResult::Problem(format!("`{}` can't be saved", spec.key));
        };
        let mut table = toml_edit::Table::new();
        table.insert(&spec.key, toml_edit::Item::Value(toml));
        let (_, problems) = crate::plugin_settings::resolve(&self.plugin, std::slice::from_ref(&spec), Some(&table));
        if let Some(p) = problems.into_iter().next() {
            // Nothing is saved, so the value in use doesn't change: drop the
            // banner's "; using DEFAULT".
            let p = p.split("; using ").next().unwrap_or_default().to_string();
            return EditorResult::Problem(format!("{p}; not saved"));
        }
        self.fields[self.selected].value = value.clone();
        EditorResult::Save(spec.key, Some(value))
    }
}

/// What a kind of setting takes, for problems.
fn needs(kind: &Kind) -> String {
    match kind {
        Kind::Integer { min, max } => format!("a whole number{}", range(min.map(|m| m as f64), max.map(|m| m as f64))),
        Kind::Number { min, max } => format!("a number{}", range(*min, *max)),
        _ => "a value".into(),
    }
}

fn range(min: Option<f64>, max: Option<f64>) -> String {
    match (min, max) {
        (Some(a), Some(b)) => format!(" from {a} to {b}"),
        (Some(a), None) => format!(" of at least {a}"),
        (None, Some(b)) => format!(" of at most {b}"),
        (None, None) => String::new(),
    }
}

/// Typed text as a value of `kind`, if it is one.
fn parse(kind: &Kind, text: &str) -> Option<Value> {
    let t = text.trim();
    match kind {
        Kind::Integer { .. } => t.parse::<i64>().ok().map(Value::from),
        Kind::Number { .. } => t.parse::<f64>().ok().filter(|f| f.is_finite()).map(Value::from),
        Kind::String => Some(Value::from(text)),
        Kind::List => Some(Value::from(
            text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(Value::from).collect::<Vec<_>>(),
        )),
        Kind::Boolean | Kind::Choice(_) => None,
    }
}

/// One step up or down from `value`, within the kind's range or choices.
fn step(kind: &Kind, value: &Value, up: bool) -> Value {
    match kind {
        Kind::Integer { min, max } => {
            let v = value.as_i64().unwrap_or(0).saturating_add(if up { 1 } else { -1 });
            json!(v.clamp(min.unwrap_or(i64::MIN), max.unwrap_or(i64::MAX)))
        }
        Kind::Number { min, max } => {
            let v = value.as_f64().unwrap_or(0.0) + if up { 1.0 } else { -1.0 };
            json!(v.clamp(min.unwrap_or(f64::MIN), max.unwrap_or(f64::MAX)))
        }
        Kind::Choice(choices) if !choices.is_empty() => {
            let i = choices.iter().position(|c| Some(c.as_str()) == value.as_str()).unwrap_or(0);
            let n = choices.len();
            json!(choices[if up { (i + 1) % n } else { (i + n - 1) % n }])
        }
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::data::{Item, Kind as ItemKind};
    use crate::plugin_settings::{Kind, Spec};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use serde_json::json;

    fn spec(key: &str, kind: Kind, default: serde_json::Value) -> Spec {
        Spec { key: key.into(), kind, default, description: format!("{key} setting") }
    }

    fn item(settings: Vec<Spec>) -> Item {
        let values = settings.iter().map(|s| (s.key.clone(), s.default.clone())).collect();
        Item {
            name: "mira/x".into(),
            title: "mira/x".into(),
            kind: ItemKind::Registry,
            on: true,
            version: None,
            update: None,
            attention: None,
            description: String::new(),
            permissions: vec![],
            manifest: None,
            entry: None,
            settings,
            values,
        }
    }

    fn press(e: &mut Editor, code: KeyCode) -> EditorResult {
        e.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn typed(e: &mut Editor, text: &str) {
        for c in text.chars() {
            press(e, KeyCode::Char(c));
        }
    }

    fn all() -> Editor {
        Editor::new(&item(vec![
            spec("on", Kind::Boolean, json!(true)),
            spec("engine", Kind::Choice(vec!["docker".into(), "podman".into()]), json!("docker")),
            spec("count", Kind::Integer { min: Some(1), max: Some(60) }, json!(5)),
            spec("ratio", Kind::Number { min: None, max: None }, json!(0.5)),
            spec("name", Kind::String, json!("x")),
            spec("hosts", Kind::List, json!(["a"])),
        ]))
        .unwrap()
    }

    #[test]
    fn each_type_has_its_control() {
        let mut e = all();
        assert_eq!(press(&mut e, KeyCode::Char(' ')), EditorResult::Save("on".into(), Some(json!(false))));
        press(&mut e, KeyCode::Down);
        press(&mut e, KeyCode::Right);
        assert_eq!(press(&mut e, KeyCode::Enter), EditorResult::Save("engine".into(), Some(json!("podman"))));
        press(&mut e, KeyCode::Down);
        typed(&mut e, "42");
        assert_eq!(press(&mut e, KeyCode::Enter), EditorResult::Save("count".into(), Some(json!(42))));
        press(&mut e, KeyCode::Right);
        assert_eq!(press(&mut e, KeyCode::Enter), EditorResult::Save("count".into(), Some(json!(43))));
        press(&mut e, KeyCode::Down);
        typed(&mut e, "-1.25");
        assert_eq!(press(&mut e, KeyCode::Enter), EditorResult::Save("ratio".into(), Some(json!(-1.25))));
        press(&mut e, KeyCode::Down);
        assert_eq!(press(&mut e, KeyCode::Enter), EditorResult::Stay, "enter starts editing text");
        press(&mut e, KeyCode::Backspace);
        typed(&mut e, "yz");
        assert_eq!(press(&mut e, KeyCode::Enter), EditorResult::Save("name".into(), Some(json!("yz"))));
        press(&mut e, KeyCode::Down);
        press(&mut e, KeyCode::Enter);
        typed(&mut e, ", b ,, c");
        assert_eq!(press(&mut e, KeyCode::Enter), EditorResult::Save("hosts".into(), Some(json!(["a", "b", "c"]))));
        assert_eq!(press(&mut e, KeyCode::Esc), EditorResult::Close);
    }

    #[test]
    fn out_of_range_is_refused() {
        let mut e = Editor::new(&item(vec![spec("n", Kind::Integer { min: Some(10), max: None }, json!(10))])).unwrap();
        typed(&mut e, "5");
        match press(&mut e, KeyCode::Enter) {
            EditorResult::Problem(p) => {
                assert!(p.contains("10") && p.ends_with("; not saved"), "{p}");
                assert!(!p.contains("using"), "{p}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(e.fields[0].value, json!(10), "nothing changed");
        press(&mut e, KeyCode::Left);
        assert_eq!(press(&mut e, KeyCode::Enter), EditorResult::Save("n".into(), Some(json!(10))), "← stops at min");
    }

    #[test]
    fn reset_removes_the_key() {
        let mut e = all();
        press(&mut e, KeyCode::Down);
        press(&mut e, KeyCode::Down);
        assert_eq!(press(&mut e, KeyCode::Char('d')), EditorResult::Save("count".into(), None));
        assert_eq!(e.fields[2].value, json!(5));
    }

    #[test]
    fn plugins_without_settings_have_no_editor() {
        assert!(Editor::new(&item(vec![])).is_none());
    }
}
