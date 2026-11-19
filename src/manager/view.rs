//! Drawing the manager. Colours are only the terminal's 16 (and its
//! default), so the Lumen theme it runs in shows through.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::manager::data::{Attention, Item, Kind};
use crate::manager::model::{Dialog, Model, Row, Tab};
use crate::registry::Entry;

/// Details sit beside the list from this width; narrower, they fill the screen.
const WIDE: u16 = 100;
const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 12;

const MUTED: Style = Style::new().fg(Color::DarkGray);
const ACCENT: Color = Color::Magenta;

/// `text` cut to `width` columns, ending in `…` when cut.
fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    if width > 0 {
        out.push('…');
    }
    out
}

fn tab_label(model: &Model, tab: Tab) -> String {
    let name = match tab {
        Tab::Installed => "Installed",
        Tab::Browse => "Browse",
        Tab::Updates => "Updates",
    };
    match model.count(tab) {
        Some(n) => format!("{name} ({n})"),
        None => name.to_string(),
    }
}

/// Where each tab's label sits on the header line: (tab, first column, end).
fn tab_spans(model: &Model) -> Vec<(Tab, u16, u16)> {
    let mut x = " Plugins    ".width() as u16;
    Tab::ALL
        .iter()
        .map(|&t| {
            let w = tab_label(model, t).width() as u16;
            let span = (t, x, x + w);
            x += w + 3;
            span
        })
        .collect()
}

struct Regions {
    header: Rect,
    list: Option<Rect>,
    details: Option<Rect>,
    status: Rect,
    footer: Rect,
}

fn regions(area: Rect, model: &Model) -> Regions {
    let [header, body, status, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)])
            .areas(area);
    let (list, details) = if area.width >= WIDE {
        let [l, d] = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(body);
        (Some(l), Some(d))
    } else if model.details || model.editing.is_some() {
        (None, Some(body))
    } else {
        (Some(body), None)
    };
    Regions { header, list, details, status, footer }
}

fn inner(r: Rect) -> Rect {
    Block::bordered().inner(r)
}

/// The first row shown, so the selection stays in view.
fn offset(model: &Model, height: u16) -> usize {
    model.selected.saturating_sub(usize::from(height.max(1)) - 1)
}

/// What a click at (`column`, `row`) lands on: a list row, or a tab.
pub fn hit(area: Rect, model: &Model, column: u16, row: u16) -> (Option<usize>, Option<Tab>) {
    let r = regions(area, model);
    if row == r.header.y {
        let tab = tab_spans(model).into_iter().find(|(_, a, b)| (*a..*b).contains(&column)).map(|(t, ..)| t);
        return (None, tab);
    }
    if let Some(list) = r.list.map(inner)
        && (list.y..list.y + list.height).contains(&row)
        && (list.x..list.x + list.width).contains(&column)
    {
        return (Some(offset(model, list.height) + usize::from(row - list.y)), None);
    }
    (None, None)
}

pub fn draw(frame: &mut Frame, model: &Model) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let text =
            Paragraph::new("Make the window larger to manage plugins (60×12 at least)").wrap(Wrap { trim: true });
        frame.render_widget(text, area);
        return;
    }
    let r = regions(area, model);
    draw_header(frame, model, r.header);
    if let Some(list) = r.list {
        draw_list(frame, model, list);
    }
    if let Some(details) = r.details {
        let block = Block::bordered().border_style(MUTED);
        let lines = match &model.editing {
            Some(e) => editor_lines(model, e),
            None => details_lines(model),
        };
        let text = Paragraph::new(lines).wrap(Wrap { trim: false }).block(block);
        frame.render_widget(text, details);
    }
    let (status, style) = match (&model.busy, &model.status) {
        (Some(b), _) => (b.as_str(), Style::new().fg(Color::Yellow)),
        (None, Some(s)) => (s.as_str(), Style::new()),
        (None, None) => ("", Style::new()),
    };
    let width = usize::from(r.status.width);
    frame.render_widget(Paragraph::new(Span::styled(fit(&format!(" {status}"), width), style)), r.status);
    frame.render_widget(Paragraph::new(Span::styled(fit(&format!(" {}", footer(model)), width), MUTED)), r.footer);
    if let Some(d) = &model.dialog {
        draw_dialog(frame, d, area);
    }
}

fn draw_header(frame: &mut Frame, model: &Model, area: Rect) {
    let mut spans = vec![Span::styled(" Plugins    ", Style::new().add_modifier(Modifier::BOLD))];
    for (i, t) in Tab::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        let style = if *t == model.tab {
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::new()
        };
        spans.push(Span::styled(tab_label(model, *t), style));
    }
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    let right = match &model.filter {
        Some(f) if model.typing => format!("/ {f}▏ "),
        Some(f) => format!("/ {f} "),
        None => "/ search   ? keys ".into(),
    };
    let room = usize::from(area.width).saturating_sub(used);
    if room > right.width() + 1 {
        spans.push(Span::raw(" ".repeat(room - right.width())));
        spans.push(Span::styled(right, MUTED));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// A list line: `left`, then `right` against the edge.
fn row_line(mark: Option<(&str, Color)>, left: &str, right: &str, width: usize) -> Line<'static> {
    let mut spans = Vec::new();
    let mut used = 0;
    if let Some((m, c)) = mark {
        spans.push(Span::styled(format!(" {m} "), Style::new().fg(c)));
        used += 3;
    } else {
        spans.push(Span::raw(" "));
        used += 1;
    }
    let right = fit(right, width.saturating_sub(used + 2) / 2);
    let room = width.saturating_sub(used + right.width() + 2);
    let left = fit(left, room);
    let gap = room.saturating_sub(left.width()) + 1;
    spans.push(Span::raw(left));
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(right, MUTED));
    Line::from(spans)
}

fn version_note(i: &Item) -> String {
    match (&i.attention, i.kind) {
        (Some(Attention::NotInstalled), _) => "not installed".into(),
        (Some(Attention::NotApproved), _) => "not approved".into(),
        (Some(Attention::Failed(_)), _) => "failed".into(),
        (_, Kind::BuiltIn) => "built in".into(),
        (_, Kind::Local) => "local".into(),
        _ => {
            let v = i.version.clone().unwrap_or_default();
            match &i.update {
                Some(u) if u.new_permissions.is_empty() => format!("{v} → {}", u.version),
                Some(u) => format!("{v} → {} +perms", u.version),
                None => v,
            }
        }
    }
}

fn mark(i: &Item) -> (&'static str, Color) {
    match (&i.attention, i.on) {
        (Some(_), _) => ("!", Color::Red),
        (None, true) => ("●", Color::Green),
        (None, false) => ("○", Color::Reset),
    }
}

fn draw_list(frame: &mut Frame, model: &Model, area: Rect) {
    let block = Block::bordered().border_style(MUTED);
    let room = block.inner(area);
    frame.render_widget(block, area);
    let rows = model.rows();
    let width = usize::from(room.width);
    let empty = match (model.tab, &model.snapshot.index, &model.index_error) {
        (Tab::Browse, None, Some(e)) => format!("Couldn't load the plugin index: {e}. Press r to try again."),
        (Tab::Browse, None, None) => "Loading the plugin index…".into(),
        _ if model.filter.as_deref().is_some_and(|f| !f.is_empty()) => "Nothing matches".into(),
        (Tab::Updates, ..) => "Everything is up to date".into(),
        _ => "Nothing here yet".into(),
    };
    if rows.is_empty() {
        frame.render_widget(Paragraph::new(format!(" {empty}")).style(MUTED).wrap(Wrap { trim: false }), room);
        return;
    }
    let first = offset(model, room.height);
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(first)
        .take(usize::from(room.height))
        .map(|(n, row)| {
            let line = match row {
                Row::Item(i) => row_line(Some(mark(i)), &i.title, &version_note(i), width),
                Row::Entry(e) => {
                    let have =
                        model.items.iter().any(|i| i.name == e.name && i.attention != Some(Attention::NotInstalled));
                    let right = if have { "installed".to_string() } else { format!("★ {}", e.stars) };
                    row_line(None, &e.name, &right, width)
                }
            };
            if n == model.selected { line.style(Style::new().add_modifier(Modifier::REVERSED)) } else { line }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), room);
}

fn and_list(words: &[&str]) -> String {
    match words {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{}; {last}", rest.join("; ")),
    }
}

fn details_lines(model: &Model) -> Vec<Line<'static>> {
    let Some(row) = model.selected_row() else { return Vec::new() };
    let (item, entry): (Option<&Item>, Option<&Entry>) = match row {
        Row::Item(i) => (Some(i), i.entry.as_ref()),
        Row::Entry(e) => (model.items.iter().find(|i| i.name == e.name), Some(e)),
    };
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let mut out = Vec::new();
    let title = item.map_or_else(|| row.name().to_string(), |i| i.title.clone());
    let version = item
        .and_then(|i| i.version.clone())
        .or_else(|| {
            entry.and_then(|e| crate::registry::best(e, crate::registry::LUMEN_VERSION)).map(|r| r.version.clone())
        })
        .unwrap_or_default();
    out.push(Line::from(vec![Span::styled(title, bold), Span::raw("  "), Span::styled(version, MUTED)]));
    let mut about = vec![row.name().to_string()];
    match (item.map(|i| i.kind), entry) {
        (_, Some(e)) => {
            about.push(e.license.clone());
            about.push(format!("★ {}", e.stars));
        }
        (Some(Kind::BuiltIn), None) => about.push("built in".into()),
        (Some(Kind::Local), None) => about.push("local".into()),
        _ => {}
    }
    out.push(Line::styled(about.join(" · "), MUTED));
    if let Some(e) = entry.filter(|e| !e.updated.is_empty()) {
        out.push(Line::styled(format!("Updated {}", e.updated), MUTED));
    }
    out.push(Line::raw(""));
    let description =
        item.map(|i| i.description.clone()).filter(|d| !d.is_empty()).or_else(|| entry.map(|e| e.description.clone()));
    out.push(Line::raw(description.unwrap_or_default()));
    out.push(Line::raw(""));
    let permissions = item.map(|i| i.permissions.clone()).filter(|p| !p.is_empty()).or_else(|| {
        entry.and_then(|e| crate::registry::best(e, crate::registry::LUMEN_VERSION)).map(|r| r.permissions.clone())
    });
    match permissions.filter(|p| !p.is_empty()) {
        Some(p) => {
            let words: Vec<&str> = p.iter().map(|x| x.describe()).collect();
            out.push(Line::raw(format!("May: {}", and_list(&words))));
        }
        None => out.push(Line::raw("Needs no permissions.")),
    }
    if let Some(i) = item {
        let reason: Vec<String> = match &i.attention {
            Some(Attention::NotInstalled) => vec!["Not installed: press i to install it".into()],
            Some(Attention::NotApproved) => vec!["Not approved: press i to approve and install it".into()],
            Some(Attention::Failed(s)) => vec![format!("Failed: {s}")],
            Some(Attention::Settings(v)) => v.clone(),
            None => Vec::new(),
        };
        if !reason.is_empty() {
            out.push(Line::raw(""));
            out.extend(reason.into_iter().map(|r| Line::styled(r, bold.fg(Color::Red))));
        }
        if let Some(u) = &i.update {
            out.push(Line::raw(""));
            out.push(Line::raw(format!("Update: {} → {}", i.version.clone().unwrap_or_default(), u.version)));
            if !u.new_permissions.is_empty() {
                let words: Vec<&str> = u.new_permissions.iter().map(|x| x.describe()).collect();
                out.push(Line::styled(
                    format!("It also asks to: {}", and_list(&words)),
                    Style::new().fg(Color::Yellow),
                ));
            }
        }
    }
    if let Some(e) = entry {
        out.push(Line::raw(""));
        let versions: Vec<&str> = e.versions.iter().map(|v| v.version.as_str()).collect();
        if !versions.is_empty() {
            out.push(Line::styled(format!("Versions: {}", versions.join(", ")), MUTED));
        }
        out.push(Line::styled(e.repo.clone(), Style::new().fg(Color::Blue)));
    }
    if let Some(i) = item.filter(|i| !i.settings.is_empty()) {
        out.push(Line::raw(""));
        out.push(Line::styled("Settings", bold));
        for spec in &i.settings {
            let value = i.values.get(&spec.key).cloned().unwrap_or_else(|| spec.default.clone());
            let shown = |v: &serde_json::Value| v.as_str().map_or_else(|| v.to_string(), str::to_string);
            let mut spans = vec![Span::raw(format!("  {}   {}", spec.key, shown(&value)))];
            if value != spec.default {
                spans.push(Span::styled(format!("   (default {})", shown(&spec.default)), MUTED));
            }
            out.push(Line::from(spans));
        }
    }
    out
}

fn editor_lines(model: &Model, e: &crate::manager::settings_edit::Editor) -> Vec<Line<'static>> {
    use crate::plugin_settings::Kind as S;
    let title = model.items.iter().find(|i| i.name == e.plugin).map_or(e.plugin.clone(), |i| i.title.clone());
    let mut out =
        vec![Line::styled(format!("Settings for {title}"), Style::new().add_modifier(Modifier::BOLD)), Line::raw("")];
    let widest = e.fields.iter().map(|f| f.spec.key.width()).max().unwrap_or(0);
    for (n, f) in e.fields.iter().enumerate() {
        let selected = n == e.selected;
        let value = match (&e.input, selected) {
            (Some(text), true) => format!("{text}▏"),
            _ => crate::manager::settings_edit::shown(&f.value),
        };
        let pad = " ".repeat(widest - f.spec.key.width());
        let mut spans = vec![
            Span::raw(format!(" {}{pad}   ", f.spec.key)),
            Span::styled(value, Style::new().add_modifier(Modifier::BOLD)),
        ];
        if f.value != f.spec.default {
            spans.push(Span::styled(
                format!("   (default {})", crate::manager::settings_edit::shown(&f.spec.default)),
                MUTED,
            ));
        }
        let line = Line::from(spans);
        out.push(if selected { line.style(Style::new().add_modifier(Modifier::REVERSED)) } else { line });
        if selected {
            let hint = match &f.spec.kind {
                S::Boolean => "space or enter turns it on or off".to_string(),
                S::Choice(c) => format!("← → choose: {}", c.join(", ")),
                S::Integer { min, max } => format!(
                    "type a whole number, or ← →{}",
                    bounds(min.map(|v| v.to_string()), max.map(|v| v.to_string()))
                ),
                S::Number { min, max } => {
                    format!("type a number, or ← →{}", bounds(min.map(|v| v.to_string()), max.map(|v| v.to_string())))
                }
                S::String => "enter to edit".into(),
                S::List => "enter to edit, separated by commas".into(),
            };
            out.push(Line::styled(format!("   {}", f.spec.description), MUTED));
            out.push(Line::styled(format!("   {hint}"), MUTED));
        }
    }
    out
}

fn bounds(min: Option<String>, max: Option<String>) -> String {
    match (min, max) {
        (Some(a), Some(b)) => format!("; {a} to {b}"),
        (Some(a), None) => format!("; at least {a}"),
        (None, Some(b)) => format!("; at most {b}"),
        (None, None) => String::new(),
    }
}

fn footer(model: &Model) -> &'static str {
    if model.dialog.is_some() {
        return "esc cancel";
    }
    if let Some(e) = &model.editing {
        return if e.input.is_some() {
            "type the value   enter save   esc cancel"
        } else {
            "↑ ↓ choose   ← → change   enter save   d default   esc done"
        };
    }
    if model.typing {
        return "type to filter   enter done   esc clear";
    }
    match model.tab {
        Tab::Installed => "space on/off   enter details   s settings   u update   x remove   q quit",
        Tab::Browse => "i install   enter details   / search   r refresh   q quit",
        Tab::Updates => "u update   U update all   enter details   q quit",
    }
}

/// `text` broken at spaces into rows of at most `width` columns.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut rows = vec![String::new()];
    for word in text.split(' ') {
        let row = rows.last_mut().expect("starts with a row");
        if row.is_empty() {
            row.push_str(word);
        } else if row.width() + 1 + word.width() > width {
            rows.push(word.to_string());
        } else {
            row.push(' ');
            row.push_str(word);
        }
    }
    rows
}

/// A dialog's keys: each key in the accent colour, then what it does.
fn key_hints(keys: &[(&str, &str)]) -> Line<'static> {
    let key = Style::new().fg(ACCENT).add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    for (n, (k, does)) in keys.iter().enumerate() {
        if n > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled(k.to_string(), key));
        spans.push(Span::raw(format!(" {does}")));
    }
    Line::from(spans)
}

/// What a dialog says, before it's fitted to a width.
enum Part {
    /// Kept on one row.
    Row(Line<'static>),
    /// Wrapped at spaces.
    Text(String, Style),
    /// Wrapped, behind a bullet, with later rows indented to match.
    Bullet(String),
    Gap,
}

impl Part {
    fn width(&self) -> usize {
        match self {
            Part::Row(l) => l.width(),
            Part::Text(t, _) => t.width(),
            Part::Bullet(t) => t.width() + 2,
            Part::Gap => 0,
        }
    }

    fn rows(self, width: usize) -> Vec<Line<'static>> {
        match self {
            Part::Row(l) => vec![l],
            Part::Text(t, style) => wrap(&t, width).into_iter().map(|r| Line::styled(r, style)).collect(),
            Part::Bullet(t) => wrap(&t, width.saturating_sub(2).max(1))
                .into_iter()
                .enumerate()
                .map(|(n, r)| {
                    Line::from(vec![
                        Span::styled(if n == 0 { "• " } else { "  " }, Style::new().fg(ACCENT)),
                        Span::raw(r),
                    ])
                })
                .collect(),
            Part::Gap => vec![Line::raw("")],
        }
    }
}

fn draw_dialog(frame: &mut Frame, dialog: &Dialog, area: Rect) {
    use ratatui::widgets::{BorderType, Padding};
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let accent = bold.fg(ACCENT);
    let plain = Style::new();
    let (title, parts): (&str, Vec<Part>) = match dialog {
        Dialog::Approve { name, permissions, .. } => {
            let mut l = vec![
                Part::Row(Line::from(vec![
                    Span::styled("Allow ", bold),
                    Span::styled(name.clone(), accent),
                    Span::styled(" to:", bold),
                ])),
                Part::Gap,
            ];
            l.extend(permissions.iter().map(|p| Part::Bullet(p.describe().to_string())));
            l.push(Part::Gap);
            l.push(Part::Text("A plugin is a program running as you.".into(), MUTED));
            l.push(Part::Text("Only allow ones you trust.".into(), MUTED));
            l.push(Part::Gap);
            l.push(Part::Row(key_hints(&[("y", "allow"), ("n", "cancel")])));
            ("Permissions", l)
        }
        Dialog::RemoveData { name } => (
            "Remove",
            vec![
                Part::Row(Line::from(vec![
                    Span::styled("Remove ", bold),
                    Span::styled(name.clone(), accent),
                    Span::styled(".", bold),
                ])),
                Part::Gap,
                Part::Text(format!("Also delete {name}'s data? (y/N)"), plain),
                Part::Gap,
                Part::Row(key_hints(&[("y", "delete it"), ("n", "keep it"), ("esc", "cancel")])),
            ],
        ),
        Dialog::Keys => (
            "Keys",
            [
                ("tab, 1 2 3", "switch tabs"),
                ("↑ ↓  j k", "move"),
                ("/", "filter, esc clears"),
                ("enter", "details"),
                ("space", "turn on or off"),
                ("i", "install"),
                ("u, U", "update one, or all"),
                ("x", "remove"),
                ("s", "settings"),
                ("r", "refresh the index"),
                ("q", "quit"),
            ]
            .into_iter()
            .map(|(k, does)| Part::Row(Line::from(vec![Span::styled(format!("{k:<13}"), accent), Span::raw(does)])))
            .collect(),
        ),
    };
    // As wide as what it says, within reason; the border and its padding
    // take 3 columns a side and 2 rows top and bottom.
    let widest = parts.iter().map(Part::width).max().unwrap_or(0).clamp(36, 58);
    let width = (widest as u16 + 6).min(area.width.saturating_sub(4));
    let lines: Vec<Line> =
        parts.into_iter().flat_map(|p| p.rows(usize::from(width.saturating_sub(6)).max(1))).collect();
    let height = (lines.len() as u16 + 4).min(area.height);
    let r = Rect::new(area.x + (area.width - width) / 2, area.y + (area.height - height) / 2, width, height);
    frame.render_widget(Clear, r);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::styled(format!(" {title} "), accent))
        .padding(Padding::new(2, 2, 1, 1));
    frame.render_widget(Paragraph::new(lines).block(block), r);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::data::{Attention, Item, Kind, Snapshot, Upgrade};
    use crate::manager::model::{Dialog, Model, Tab};
    use crate::plugins::Permission;
    use crate::registry::Index;
    use crate::registry::testing::{entry, release};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::style::Color;
    use unicode_width::UnicodeWidthStr;

    fn render(model: &Model, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, model)).unwrap();
        t
    }

    fn lines(t: &Terminal<TestBackend>) -> Vec<String> {
        let b = t.backend().buffer();
        (0..b.area.height)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0;
                while x < b.area.width {
                    let s = b[(x, y)].symbol();
                    line.push_str(s);
                    x += s.width().max(1) as u16;
                }
                line
            })
            .collect()
    }

    fn screen(model: &Model, w: u16, h: u16) -> String {
        lines(&render(model, w, h)).join("\n")
    }

    fn item(name: &str, title: &str, kind: Kind, on: bool) -> Item {
        Item {
            name: name.into(),
            title: title.into(),
            kind,
            on,
            version: (kind != Kind::BuiltIn).then(|| "1.0.0".into()),
            update: None,
            attention: None,
            description: format!("{title} does things."),
            permissions: vec![],
            manifest: None,
            entry: None,
            settings: vec![],
            values: Default::default(),
        }
    }

    fn model() -> Model {
        let mut docker = entry("mira/docker-panes", 214, &[]);
        docker.versions = vec![release("1.3.0", vec![Permission::Panes], String::new())];
        let mut m =
            Model::new(Snapshot { index: Some(Index { version: 1, plugins: vec![docker] }), ..Default::default() });
        let mut missing = item("jira/picker", "jira/picker", Kind::Registry, true);
        missing.attention = Some(Attention::NotInstalled);
        missing.version = None;
        m.items = vec![
            item("recent-dirs", "Recent Directories", Kind::BuiltIn, true),
            item("pomodoro", "Pomodoro Timer", Kind::BuiltIn, false),
            item("where", "where", Kind::Local, true),
            missing,
        ];
        m
    }

    #[test]
    fn installed_tab_shows_marks_and_reasons() {
        let mut m = model();
        m.selected = 3;
        let s = screen(&m, 120, 30);
        for want in [
            "● Recent Directories",
            "○ Pomodoro Timer",
            "! jira/picker",
            "not installed",
            "built in",
            "local",
            "Installed (4)",
            "Browse",
            "Updates (0)",
        ] {
            assert!(s.contains(want), "{want:?} missing:\n{s}");
        }
        assert!(s.contains("space on/off   enter details   s settings   u update   x remove   q quit"), "{s}");
        assert!(s.contains("Not installed: press i to install it"), "details give the reason:\n{s}");
    }

    #[test]
    fn details_show_permissions_in_plain_words() {
        let mut m = model();
        m.items[0].permissions = vec![Permission::Panes];
        let s = screen(&m, 120, 30);
        assert!(s.contains("May: open, close and rearrange panes and tabs"), "{s}");
        m.items[0].permissions.clear();
        assert!(screen(&m, 120, 30).contains("Needs no permissions."));
    }

    #[test]
    fn browse_shows_the_registry() {
        let mut m = model();
        m.tab = Tab::Browse;
        let s = screen(&m, 120, 30);
        assert!(s.contains("mira/docker-panes") && s.contains("★ 214"), "{s}");
        assert!(s.contains("MIT"), "licence in details:\n{s}");
        m.snapshot.index = None;
        m.index_error = Some("curl: (6) Could not resolve host".into());
        let s = screen(&m, 120, 30);
        assert!(s.contains("Couldn't load the plugin index: curl: (6)"), "{s}");
        assert!(s.contains("Press r to try again."), "{s}");
    }

    #[test]
    fn narrow_screens_show_details_full_screen() {
        let mut m = model();
        let s = screen(&m, 80, 24);
        assert!(!s.contains("Recent Directories does things."), "no details column at 80:\n{s}");
        m.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let s = screen(&m, 80, 24);
        assert!(s.contains("Recent Directories does things."), "{s}");
        assert!(!s.contains("○ Pomodoro Timer"), "the list is hidden:\n{s}");
    }

    #[test]
    fn narrow_screens_truncate() {
        let mut m = model();
        let long = format!("{}/{}", "o".repeat(39), "né".repeat(19) + "x");
        let mut it = item(&long, &long, Kind::Registry, true);
        it.description = "word ".repeat(60);
        it.version = Some("10.20.30".into());
        it.update = Some(Upgrade { version: "10.20.31".into(), new_permissions: vec![Permission::Screen] });
        m.items.push(it.clone());
        m.updates.push(it);
        m.selected = 4;
        for tab in Tab::ALL {
            m.tab = tab;
            let t = render(&m, 60, 12);
            for l in lines(&t) {
                assert!(l.width() <= 60, "{l:?}");
            }
        }
        m.tab = Tab::Installed;
        let s = screen(&m, 60, 12);
        assert!(s.contains('…'), "{s}");
        assert!(s.contains("10.20.30 → 10.20.31"), "{s}");
    }

    #[test]
    fn the_settings_editor_shows_fields() {
        let mut m = model();
        m.items[0].settings = vec![crate::plugin_settings::Spec {
            key: "max-shown".into(),
            kind: crate::plugin_settings::Kind::Integer { min: Some(10), max: Some(1000) },
            default: serde_json::json!(200),
            description: "how many folders the picker lists".into(),
        }];
        m.items[0].values.insert("max-shown".into(), serde_json::json!(50));
        m.editing = crate::manager::settings_edit::Editor::new(&m.items[0]);
        for (w, h) in [(120, 30), (70, 20)] {
            let s = screen(&m, w, h);
            assert!(s.contains("Settings for Recent Directories"), "{s}");
            assert!(s.contains("max-shown") && s.contains("50"), "{s}");
            assert!(s.contains("how many folders the picker lists") && s.contains("10 to 1000"), "{s}");
            assert!(s.contains("enter save"), "{s}");
        }
    }

    #[test]
    fn too_small_says_so() {
        let s = screen(&model(), 59, 20);
        assert!(s.contains("Make the window larger to manage plugins (60×12 at least)"), "{s}");
        assert!(screen(&model(), 80, 11).contains("60×12"));
    }

    #[test]
    fn dialogs_draw_over_the_list() {
        let mut m = model();
        m.dialog = Some(Dialog::Approve {
            name: "mira/x".into(),
            permissions: vec![Permission::Panes, Permission::Screen],
            then: Box::new(crate::manager::model::Effect::Install { name: "mira/x".into(), allowed: vec![] }),
        });
        let s = screen(&m, 100, 30);
        assert!(s.contains("Allow mira/x to:"), "{s}");
        assert!(s.contains("open, close and rearrange panes and tabs"), "{s}");
        assert!(s.contains("read what's on screen and the commands you run"), "{s}");
        assert!(s.contains("y allow   n cancel"), "{s}");
        m.dialog = Some(Dialog::RemoveData { name: "mira/x".into() });
        assert!(screen(&m, 100, 30).contains("Also delete mira/x's data? (y/N)"));
    }

    #[test]
    fn dialogs_are_padded_and_wrap_to_fit() {
        let mut m = model();
        m.dialog = Some(Dialog::Approve {
            name: "mira/x".into(),
            permissions: vec![Permission::Panes, Permission::Input],
            then: Box::new(crate::manager::model::Effect::Install { name: "mira/x".into(), allowed: vec![] }),
        });
        let ls = lines(&render(&m, 60, 20));
        let top = ls.iter().position(|l| l.contains("╭ Permissions ─")).unwrap_or_else(|| panic!("{ls:#?}"));
        let bottom = ls.iter().position(|l| l.contains('╰')).unwrap();
        // One empty row inside the border, top and bottom, and two columns at the side.
        let inside = ls[top].chars().skip_while(|c| *c != '╭').take_while(|c| *c != '╮').count() - 1;
        let empty = format!("│{}│", " ".repeat(inside));
        assert!(ls[top + 1].contains(&empty) && ls[bottom - 1].contains(&empty), "{ls:#?}");
        assert!(ls[top + 2].contains("│  Allow mira/x to:"), "{ls:#?}");
        assert!(ls[bottom - 2].contains("│  y allow   n cancel"), "{ls:#?}");
        // Nothing wraps into the padding or is cut off.
        assert!(ls.iter().any(|l| l.contains("which can run any command  │")), "{ls:#?}");
        assert!(ls.iter().any(|l| l.contains("• type into your panes, which can run any command")), "{ls:#?}");
    }

    #[test]
    fn wrapping_breaks_at_spaces() {
        assert_eq!(wrap("only allow ones you trust", 12), ["only allow", "ones you", "trust"]);
        assert_eq!(wrap("short", 12), ["short"]);
        assert_eq!(wrap("unbreakablylongword x", 5), ["unbreakablylongword", "x"]);
    }

    #[test]
    fn only_ansi_colours_are_used() {
        let mut m = model();
        m.status = Some("something went wrong".into());
        m.dialog = Some(Dialog::Keys);
        for tab in Tab::ALL {
            m.tab = tab;
            let t = render(&m, 120, 30);
            let b = t.backend().buffer();
            for c in &b.content {
                for col in [c.fg, c.bg] {
                    assert!(!matches!(col, Color::Rgb(..) | Color::Indexed(_)), "{col:?}");
                }
            }
        }
    }

    #[test]
    fn clicks_hit_rows_and_tabs() {
        let m = model();
        let t = render(&m, 120, 30);
        let ls = lines(&t);
        let area = ratatui::layout::Rect::new(0, 0, 120, 30);
        let x = ls[0].find("Browse").unwrap() as u16;
        assert_eq!(hit(area, &m, x + 1, 0), (None, Some(Tab::Browse)));
        let y = ls.iter().position(|l| l.contains("Pomodoro Timer") && l.contains('○')).unwrap() as u16;
        assert_eq!(hit(area, &m, 4, y), (Some(1), None));
        assert_eq!(hit(area, &m, 4, y + 1), (Some(2), None));
    }
}
