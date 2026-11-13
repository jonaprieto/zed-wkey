//! The terminal view: a which-key panel for the vim state at Zed's cursor.
use crate::keymap::Keymap;
use crate::lsp::Cursor;
use crate::vim::{self, Category, Mode};
use crate::whichkey::{Entry, Kind, Outcome, State};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
};
use std::collections::BTreeSet;

/// The Zed keystroke for a terminal key event, e.g. `ctrl-w` or `shift-g`.
pub fn zed_key(ev: KeyEvent) -> Option<String> {
    let m = ev.modifiers;
    let mut shift = m.contains(KeyModifiers::SHIFT);
    let key = match ev.code {
        KeyCode::Char(' ') => "space".into(),
        KeyCode::Char(c) if c.is_ascii_uppercase() => {
            shift = true;
            c.to_ascii_lowercase().to_string()
        }
        KeyCode::Char(c) => {
            // `$` arrives with shift held; Zed writes it without.
            shift &= c.is_ascii_alphabetic();
            c.to_string()
        }
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "escape".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => {
            shift = true;
            "tab".into()
        }
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pageup".into(),
        KeyCode::PageDown => "pagedown".into(),
        KeyCode::F(n) => format!("f{n}"),
        _ => return None,
    };
    let mut parts = vec![];
    for (on, name) in [
        (m.contains(KeyModifiers::CONTROL), "ctrl"),
        (m.contains(KeyModifiers::ALT), "alt"),
        (shift, "shift"),
        (m.contains(KeyModifiers::SUPER), "cmd"),
    ] {
        if on {
            parts.push(name.to_string());
        }
    }
    parts.push(key);
    Some(parts.join("-"))
}

pub struct App {
    keymap: Keymap,
    pub state: State,
    /// Take the mode from Zed; off once a mode is picked by hand.
    pub follow: bool,
    cursor: Option<Cursor>,
    collapsed: BTreeSet<Category>,
    selected: usize,
    last: Option<Outcome>,
    /// Shown until the first command, e.g. a keymap that failed to load.
    pub notice: Option<String>,
    pub quit: bool,
}

const HELP: &str =
    "keys go to vim · tab mode · F2 follow Zed · ↑↓ section · ⏎ fold · ⌫ back · C-c quit";

impl App {
    pub fn new(keymap: Keymap, os: &str) -> Self {
        Self {
            keymap,
            state: State::new(os),
            follow: true,
            cursor: None,
            collapsed: BTreeSet::new(),
            selected: 0,
            last: None,
            notice: None,
            quit: false,
        }
    }

    pub fn on_cursor(&mut self, cursor: Cursor) {
        self.cursor = Some(cursor);
        if self.follow {
            self.follow_cursor();
        }
    }

    fn follow_cursor(&mut self) {
        let Some(c) = &self.cursor else { return };
        if self.state.mode != c.mode {
            self.state.set_mode(c.mode);
        }
        self.state.extension = c.extension();
    }

    pub fn on_key(&mut self, ev: KeyEvent) {
        let pending = !self.state.prefix.is_empty() || !self.state.operators.is_empty();
        match ev.code {
            KeyCode::Char('c') if ev.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Tab | KeyCode::BackTab => {
                let i = Mode::ALL
                    .iter()
                    .position(|&m| m == self.state.mode)
                    .unwrap_or(0);
                let step = if ev.code == KeyCode::Tab {
                    1
                } else {
                    Mode::ALL.len() - 1
                };
                self.state.set_mode(Mode::ALL[(i + step) % Mode::ALL.len()]);
                self.follow = false;
            }
            KeyCode::F(2) => {
                self.follow = true;
                self.follow_cursor();
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.sections().len().saturating_sub(1))
            }
            KeyCode::Enter => {
                if let Some(&(category, _)) = self.sections().get(self.selected)
                    && !self.collapsed.remove(&category)
                {
                    self.collapsed.insert(category);
                }
            }
            KeyCode::Backspace if pending => self.state.back(),
            _ => {
                let Some(key) = zed_key(ev) else { return };
                let mode = self.state.mode;
                let outcome = self.state.press(&self.keymap, &key);
                if self.state.mode != mode {
                    self.follow = false;
                }
                if matches!(outcome, Outcome::Ran { .. } | Outcome::Unbound(_)) {
                    self.last = Some(outcome);
                }
                self.selected = self.selected.min(self.sections().len().saturating_sub(1));
            }
        }
    }

    fn sections(&self) -> Vec<(Category, Vec<Entry>)> {
        let mut entries = self.state.entries(&self.keymap);
        entries.sort_by_cached_key(|e| {
            let k = vim::display(&e.key);
            (e.category, k.to_lowercase(), k)
        });
        let mut out: Vec<(Category, Vec<Entry>)> = vec![];
        for e in entries {
            match out.last_mut() {
                Some((c, list)) if *c == e.category => list.push(e),
                _ => out.push((e.category, vec![e])),
            }
        }
        out
    }

    pub fn render(&self, frame: &mut Frame) {
        let [header, body, detail, help] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        frame.render_widget(Paragraph::new(self.header()), header);
        let (lines, selected_at) = self.body(body.width as usize);
        let scroll = selected_at.saturating_sub(body.height as usize * 2 / 3);
        frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), body);
        frame.render_widget(Paragraph::new(self.detail()), detail);
        frame.render_widget(Paragraph::new(HELP.dim()), help);
    }

    fn header(&self) -> Line<'_> {
        let vim_mode = self.state.vim_mode();
        let color = match (vim_mode, self.state.mode) {
            ("operator" | "waiting" | "literal", _) => Color::Yellow,
            (_, Mode::Normal) => Color::Blue,
            (_, Mode::Insert) => Color::Green,
            (_, Mode::Visual) => Color::Magenta,
            (_, Mode::Replace) => Color::Red,
        };
        let badge = format!(" {} ", vim_mode.to_uppercase());
        let mut spans = vec![badge.bold().fg(Color::Black).bg(color)];
        if !self.state.typed.is_empty() {
            let typed: Vec<_> = self.state.typed.iter().map(|k| vim::display(k)).collect();
            spans.push(format!(" {} ", typed.join(" ")).bold().fg(color));
        }
        if let Some(c) = &self.cursor {
            let file = c.uri.rsplit('/').next().unwrap_or_default();
            spans.push(format!("  {file}:{}:{}", c.line + 1, c.character + 1).into());
        }
        spans.push(if self.follow {
            "  following Zed".dim()
        } else {
            "  mode picked by hand".dim()
        });
        Line::from(spans)
    }

    /// The sections as lines, and the line of the selected section's title.
    fn body(&self, width: usize) -> (Vec<Line<'_>>, usize) {
        let mut lines = vec![];
        let mut selected_at = 0;
        for (i, (category, entries)) in self.sections().into_iter().enumerate() {
            let open = !self.collapsed.contains(&category);
            let title = format!(
                "{} {} ({})",
                if open { '▾' } else { '▸' },
                category.title(),
                entries.len()
            );
            let mut style = Style::new().bold();
            if i == self.selected {
                style = style.add_modifier(Modifier::REVERSED);
                selected_at = lines.len();
            }
            lines.push(Line::from(Span::styled(title, style)));
            if open {
                lines.extend(grid(&entries, width));
            }
        }
        (lines, selected_at)
    }

    fn detail(&self) -> Line<'_> {
        match &self.last {
            Some(Outcome::Ran {
                keys,
                label,
                action,
                context,
            }) => {
                let mut spans = vec![format!("{keys} → {label}").bold()];
                if !action.is_empty() {
                    spans.push(format!("  {action}").cyan());
                }
                if !context.is_empty() {
                    spans.push(format!("  [{context}]").dim());
                }
                Line::from(spans)
            }
            Some(Outcome::Unbound(keys)) => Line::from(format!("{keys} is not bound here").red()),
            _ => match &self.notice {
                Some(notice) => Line::from(notice.as_str().red()),
                None => Line::from("type a key to see what it does".dim()),
            },
        }
    }
}

const MAX_KEY: usize = 8;
const MAX_LABEL: usize = 30;

/// Entries flowed into as many columns as fit, filled top to bottom.
fn grid(entries: &[Entry], width: usize) -> Vec<Line<'static>> {
    let keys: Vec<String> = entries.iter().map(|e| vim::display(&e.key)).collect();
    let key_w = keys
        .iter()
        .map(|k| k.chars().count())
        .max()
        .unwrap_or(1)
        .min(MAX_KEY);
    let label_w = entries
        .iter()
        .map(|e| e.label.chars().count())
        .max()
        .unwrap_or(1)
        .min(MAX_LABEL);
    let cell_w = key_w + 3 + label_w + 2;
    let cols = (width / cell_w).max(1);
    let rows = entries.len().div_ceil(cols);
    (0..rows)
        .map(|r| {
            let mut spans = vec![];
            for c in 0..cols {
                let Some(e) = entries.get(c * rows + r) else {
                    break;
                };
                let color = match e.kind {
                    Kind::Group(_) => Color::Magenta,
                    Kind::Operator => Color::Yellow,
                    Kind::Action => Color::Cyan,
                };
                spans.push(
                    format!("{:>key_w$}", fit(&keys[c * rows + r], key_w))
                        .bold()
                        .fg(color),
                );
                spans.push(" → ".dim());
                let label = format!("{:<w$}", fit(&e.label, label_w), w = label_w + 2);
                spans.push(match e.kind {
                    Kind::Group(_) => label.fg(Color::Magenta),
                    _ => label.into(),
                });
            }
            Line::from(spans)
        })
        .collect()
}

fn fit(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.into()
    } else {
        s.chars().take(w - 1).chain(['…']).collect()
    }
}

/// Runs the view until `C-c`, rereading Zed's cursor file a few times a second.
pub fn main() -> anyhow::Result<()> {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let user = std::env::var("HOME")
        .ok()
        .and_then(|home| std::fs::read_to_string(format!("{home}/.config/zed/keymap.json")).ok());
    let (keymap, notice) = crate::whichkey::with_user(os, user.as_deref());
    let mut app = App::new(keymap, os);
    app.notice = notice;
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, crate::lsp::default_cursor_path());
    ratatui::restore();
    result
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    cursor_file: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    use ratatui::crossterm::event::{self, Event, KeyEventKind};
    let mut seen = vec![];
    loop {
        // ponytail: polls a tiny file; a socket would only save the reads.
        if let Some(bytes) = cursor_file.as_ref().and_then(|p| std::fs::read(p).ok())
            && bytes != seen
        {
            if let Some(c) = serde_json::from_slice(&bytes)
                .ok()
                .as_ref()
                .and_then(Cursor::from_json)
            {
                app.on_cursor(c);
            }
            seen = bytes;
        }
        terminal.draw(|f| app.render(f))?;
        if event::poll(std::time::Duration::from_millis(150))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.on_key(key);
        }
        if app.quit {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::whichkey::bundled;
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    fn app() -> App {
        App::new(bundled("macos"), "macos")
    }

    fn screen(app: &App, width: u16, height: u16) -> Vec<String> {
        let mut t = Terminal::new(TestBackend::new(width, height)).unwrap();
        t.draw(|f| app.render(f)).unwrap();
        rows(t.backend().buffer())
    }

    fn rows(b: &Buffer) -> Vec<String> {
        (0..b.area.height)
            .map(|y| (0..b.area.width).map(|x| b[(x, y)].symbol()).collect())
            .collect()
    }

    fn shows(screen: &[String], text: &str) -> bool {
        screen.iter().any(|row| row.contains(text))
    }

    #[test]
    fn terminal_keys_become_zed_keystrokes() {
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(zed_key(ch('g')).as_deref(), Some("g"));
        assert_eq!(zed_key(ch('G')).as_deref(), Some("shift-g"));
        assert_eq!(
            zed_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT)).as_deref(),
            Some("shift-g")
        );
        assert_eq!(zed_key(ch('$')).as_deref(), Some("$"));
        assert_eq!(zed_key(ctrl('w')).as_deref(), Some("ctrl-w"));
        assert_eq!(zed_key(ch(' ')).as_deref(), Some("space"));
        assert_eq!(zed_key(key(KeyCode::Esc)).as_deref(), Some("escape"));
        assert_eq!(zed_key(key(KeyCode::F(5))).as_deref(), Some("f5"));
        assert_eq!(
            zed_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)).as_deref(),
            Some("alt-x")
        );
    }

    #[test]
    fn shows_mode_sections_and_bindings() {
        let s = screen(&app(), 120, 60);
        assert!(s[0].contains("NORMAL"), "{}", s[0]);
        for text in [
            "Motions",
            "Operators",
            "Prefixes",
            "next word start",
            "+go to / more",
        ] {
            assert!(shows(&s, text), "missing {text}:\n{}", s.join("\n"));
        }
    }

    #[test]
    fn wide_terminals_use_columns() {
        let narrow = screen(&app(), 40, 200);
        let wide = screen(&app(), 160, 200);
        let row_with =
            |s: &[String], a: &str, b: &str| s.iter().any(|r| r.contains(a) && r.contains(b));
        assert!(!row_with(&narrow, "next word start", "previous word start"));
        assert!(
            narrow.iter().filter(|r| !r.trim().is_empty()).count()
                > wide.iter().filter(|r| !r.trim().is_empty()).count()
        );
    }

    #[test]
    fn typing_follows_prefixes_and_reports_commands() {
        let mut a = app();
        a.on_key(ch('g'));
        let s = screen(&a, 120, 60);
        assert!(s[0].contains(" g "), "{}", s[0]);
        assert!(shows(&s, "start of document"));
        a.on_key(ch('g'));
        let s = screen(&a, 120, 60);
        assert!(shows(&s, "g g → start of document"), "{}", s.join("\n"));
        assert!(shows(&s, "vim::StartOfDocument"));
        a.on_key(ch('d'));
        a.on_key(key(KeyCode::Backspace));
        assert!(a.state.operators.is_empty());
        a.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(a.quit);
    }

    #[test]
    fn enter_folds_the_selected_section() {
        let mut a = app();
        let open = screen(&a, 120, 60);
        let first = open.iter().find(|r| r.contains('▾')).unwrap().clone();
        a.on_key(key(KeyCode::Enter));
        let folded = screen(&a, 120, 60);
        assert!(folded.iter().any(|r| r.contains('▸')));
        assert!(!shows(&folded, "+go to / more"), "{first}");
        a.on_key(key(KeyCode::Down));
        a.on_key(key(KeyCode::Up));
        a.on_key(key(KeyCode::Enter));
        assert!(shows(&screen(&a, 120, 60), "+go to / more"));
    }

    #[test]
    fn follows_zed_until_a_mode_is_picked() {
        let mut a = app();
        let cursor = Cursor {
            uri: "file:///p/notes.md".into(),
            line: 2,
            character: 7,
            mode: Mode::Insert,
        };
        a.on_cursor(cursor.clone());
        assert_eq!(a.state.mode, Mode::Insert);
        assert_eq!(a.state.extension.as_deref(), Some("md"));
        let s = screen(&a, 120, 40);
        assert!(
            s[0].contains("INSERT") && s[0].contains("notes.md:3:8"),
            "{}",
            s[0]
        );
        a.on_key(key(KeyCode::Tab));
        assert_eq!(a.state.mode, Mode::Visual);
        assert!(!a.follow);
        a.on_cursor(Cursor {
            mode: Mode::Normal,
            ..cursor
        });
        assert_eq!(a.state.mode, Mode::Visual);
        a.on_key(key(KeyCode::F(2)));
        assert!(a.follow);
        assert_eq!(a.state.mode, Mode::Normal);
    }
}
