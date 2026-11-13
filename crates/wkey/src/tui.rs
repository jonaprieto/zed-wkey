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
    /// Lines scrolled past the selected section's title.
    offset: usize,
    last: Option<Outcome>,
    /// Shown until the first command, e.g. a keymap that failed to load.
    pub notice: Option<String>,
    pub quit: bool,
    /// Keys typed into Zed are watched, so they, not the server, give the mode.
    pub keys: bool,
    /// The panel has focus, so keys typed into Zed are meant for it.
    focused: bool,
}

const HELP: &str = "keys go to vim · tab mode · f2 follow Zed · ↑↓ section · j k scroll · ⏎ fold · ⌫ back · ctrl-c quit";

impl App {
    pub fn new(keymap: Keymap, os: &str) -> Self {
        Self {
            keymap,
            state: State::new(os),
            follow: true,
            cursor: None,
            collapsed: BTreeSet::from([Category::Scroll]),
            selected: 0,
            offset: 0,
            last: None,
            notice: None,
            quit: false,
            keys: false,
            focused: false,
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
        // With keys watched, the server only knows better about selections,
        // which the mouse makes too.
        let mode = match (self.keys, c.mode, self.state.mode) {
            (false, m, _) | (true, m @ Mode::Visual, _) => m,
            (true, _, Mode::Visual) => Mode::Normal,
            (true, _, m) => m,
        };
        if self.state.mode != mode {
            self.state.set_mode(mode);
        }
        self.state.extension = c.extension();
    }

    pub fn on_focus(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// A key typed into Zed: the simulated vim takes it as Zed's vim did.
    pub fn on_zed_key(&mut self, ev: KeyEvent) {
        if self.focused || !self.follow {
            return;
        }
        let Some(key) = zed_key(ev) else { return };
        let outcome = self.state.press(&self.keymap, &key);
        // Text typed in insert mode is unbound; only commands are worth showing.
        if matches!(outcome, Outcome::Ran { .. }) {
            self.last = Some(outcome);
        }
        self.selected = self.selected.min(self.sections().len().saturating_sub(1));
    }

    pub fn on_key(&mut self, ev: KeyEvent) {
        self.focused = true;
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
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.offset = 0;
            }
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.sections().len().saturating_sub(1));
                self.offset = 0;
            }
            // `j` and `k` only mean something to vim after a prefix or operator.
            KeyCode::Char('j') if ev.modifiers.is_empty() && !pending => self.offset += 1,
            KeyCode::Char('k') if ev.modifiers.is_empty() && !pending => {
                self.offset = self.offset.saturating_sub(1)
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
        // Letters and digits first, `g` before `G`, then symbols, then named and modified keys.
        entries.sort_by_cached_key(|e| {
            let base = e.key.strip_prefix("shift-").filter(|k| k.len() == 1);
            let base = base.unwrap_or(&e.key);
            let rank = match base.chars().next() {
                _ if base.chars().count() > 1 => 2,
                Some(c) if c.is_ascii_alphanumeric() => 0,
                _ => 1,
            };
            (
                e.category,
                rank,
                base.to_lowercase(),
                e.key.starts_with("shift-"),
            )
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

    pub fn render(&mut self, frame: &mut Frame) {
        let [header, body, detail, help] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        frame.render_widget(Paragraph::new(self.header()), header);
        let (lines, selected_at, rows) = self.body(body.width as usize);
        // The selected title on top, scrolled at most through its own section.
        let last = lines.len().saturating_sub(body.height as usize);
        let scroll = (selected_at + self.offset.min(rows)).min(last);
        self.offset = scroll.saturating_sub(selected_at);
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
            let mac = self.state.mac();
            let typed: Vec<_> = self
                .state
                .typed
                .iter()
                .map(|k| vim::display(k, mac))
                .collect();
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

    /// The sections as lines, the line of the selected section's title and
    /// the number of lines under it.
    fn body(&self, width: usize) -> (Vec<Line<'static>>, usize, usize) {
        let mut lines = vec![];
        let mut selected_at = 0;
        let mut rows = 0;
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
                let grid = grid(&entries, width, self.state.mac());
                if i == self.selected {
                    rows = grid.len();
                }
                lines.extend(grid);
            }
        }
        (lines, selected_at, rows)
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

const MAX_KEY: usize = 12;
const MAX_LABEL: usize = 30;

/// Entries flowed into as many columns as fit, filled top to bottom.
fn grid(entries: &[Entry], width: usize, mac: bool) -> Vec<Line<'static>> {
    let keys: Vec<String> = entries.iter().map(|e| vim::display(&e.key, mac)).collect();
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

/// Runs the view until `ctrl-c`, rereading Zed's cursor file a few times a second.
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
    let (tx, keys) = std::sync::mpsc::channel();
    match crate::keys::watch(tx) {
        Ok(()) => app.keys = true,
        Err(e) if cfg!(target_os = "macos") => {
            app.notice.get_or_insert(format!("{e:#}"));
        }
        Err(_) => {}
    }
    use ratatui::crossterm::{event, execute};
    let mut terminal = ratatui::init();
    // Zed's terminal reports focus, which tells keys for the panel from keys for Zed.
    execute!(std::io::stdout(), event::EnableFocusChange)?;
    let result = run(
        &mut terminal,
        &mut app,
        crate::lsp::default_cursor_path(),
        &keys,
    );
    let _ = execute!(std::io::stdout(), event::DisableFocusChange);
    ratatui::restore();
    result
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    cursor_file: Option<std::path::PathBuf>,
    keys: &std::sync::mpsc::Receiver<KeyEvent>,
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
        for key in keys.try_iter() {
            app.on_zed_key(key);
        }
        terminal.draw(|f| app.render(f))?;
        if event::poll(std::time::Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::FocusGained => app.on_focus(true),
                Event::FocusLost => app.on_focus(false),
                _ => {}
            }
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

    fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
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
        let s = screen(&mut app(), 120, 60);
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
        let narrow = screen(&mut app(), 40, 200);
        let wide = screen(&mut app(), 160, 200);
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
        let s = screen(&mut a, 120, 60);
        assert!(s[0].contains(" g "), "{}", s[0]);
        assert!(shows(&s, "start of document"));
        a.on_key(ch('g'));
        let s = screen(&mut a, 120, 60);
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
        let open = screen(&mut a, 120, 60);
        assert!(open[1].contains("▾ Operators"), "{}", open[1]);
        assert!(shows(&screen(&mut a, 120, 300), "▸ Scroll"));
        a.on_key(key(KeyCode::Enter));
        let folded = screen(&mut a, 120, 60);
        assert!(folded[1].contains("▸ Operators"), "{}", folded[1]);
        assert!(!shows(&folded, "d → delete"));
        a.on_key(key(KeyCode::Down));
        a.on_key(key(KeyCode::Up));
        a.on_key(key(KeyCode::Enter));
        assert!(shows(&screen(&mut a, 120, 60), "d → delete"));
    }

    #[test]
    fn j_and_k_scroll_the_selected_section_unless_vim_waits() {
        let mut a = app();
        a.on_key(key(KeyCode::Down));
        let top = screen(&mut a, 40, 12);
        assert!(top[1].contains("Motions"), "{}", top[1]);
        a.on_key(ch('j'));
        a.on_key(ch('j'));
        let down = screen(&mut a, 40, 12);
        assert_eq!(down[1], top[3]);
        a.on_key(ch('k'));
        assert_eq!(screen(&mut a, 40, 12)[1], top[2]);
        // Past the section's end `j` stops, so one `k` always moves back.
        for _ in 0..500 {
            a.on_key(ch('j'));
        }
        let end = screen(&mut a, 40, 12);
        a.on_key(ch('k'));
        assert_ne!(screen(&mut a, 40, 12), end);
        a.on_key(ch('g'));
        a.on_key(ch('j'));
        assert!(a.state.typed.is_empty(), "g j is a vim command");
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
        let s = screen(&mut a, 120, 40);
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

    fn at(mode: Mode) -> Cursor {
        Cursor {
            uri: "file:///p/main.rs".into(),
            line: 0,
            character: 0,
            mode,
        }
    }

    #[test]
    fn keys_typed_in_zed_show_what_can_follow() {
        let mut a = app();
        a.on_zed_key(ch('d'));
        assert_eq!(a.state.vim_mode(), "operator");
        assert!(screen(&mut a, 120, 60)[0].contains(" d "));
        a.on_zed_key(ch('i'));
        a.on_zed_key(ch('w'));
        assert!(a.state.typed.is_empty());
        assert!(shows(&screen(&mut a, 120, 60), "d i w →"));
        // Escape out of insert mode, which Zed never tells the server.
        a.on_zed_key(ch('i'));
        a.on_zed_key(ch('x'));
        assert_eq!(a.state.mode, Mode::Insert);
        a.on_zed_key(key(KeyCode::Esc));
        assert_eq!(a.state.mode, Mode::Normal);
    }

    #[test]
    fn keys_for_the_panel_are_not_zed_keys() {
        let mut a = app();
        a.on_focus(true);
        a.on_zed_key(ch('d'));
        assert!(a.state.typed.is_empty());
        a.on_focus(false);
        a.on_zed_key(ch('d'));
        assert_eq!(a.state.vim_mode(), "operator");
        // A key in the panel means it has focus, even before Zed says so.
        let mut a = app();
        a.on_key(ch('g'));
        a.on_zed_key(ch('d'));
        assert_eq!(a.state.typed, ["g"]);
    }

    #[test]
    fn with_keys_the_server_only_reports_selections() {
        let mut a = app();
        a.keys = true;
        a.on_zed_key(ch('a'));
        // `a` moves the cursor, which reads as normal mode to the server.
        a.on_cursor(at(Mode::Normal));
        assert_eq!(a.state.mode, Mode::Insert);
        a.on_zed_key(key(KeyCode::Esc));
        a.on_cursor(at(Mode::Visual));
        assert_eq!(a.state.mode, Mode::Visual);
        a.on_cursor(at(Mode::Normal));
        assert_eq!(a.state.mode, Mode::Normal);
    }
}
