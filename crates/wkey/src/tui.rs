//! The terminal view: a which-key panel for the vim state at Zed's cursor.
use crate::keymap::Keymap;
use crate::lsp::Cursor;
use crate::vim::{self, Category, Mode};
use crate::whichkey::{Entry, Kind, Outcome, State};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
};
use std::collections::{BTreeMap, BTreeSet};

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
    /// The section picked by hand, kept across modes by what it is.
    picked: Category,
    /// The section pending keys call for, until one is picked by hand.
    auto: Option<Category>,
    /// Columns scrolled past the one with the selected section's title.
    offset: usize,
    /// Keys another app takes before Zed sees them, with what it does instead.
    pub taken: BTreeMap<String, String>,
    last: Option<Outcome>,
    /// Shown until the first command, e.g. a keymap that failed to load.
    pub notice: Option<String>,
    pub quit: bool,
    /// Keys typed into Zed are watched, so they, not the server, give the mode.
    pub keys: bool,
    /// The panel has focus, so keys typed into Zed are meant for it.
    focused: bool,
}

const HELP: &str = "⇥ ↑↓ section · ⏎ fold · j k page · f3 vim mode · f2 follow Zed · ⌫ drop key · ctrl-c quit · other keys go to vim";
const TABS_HELP: &str = "⇥ ⇧⇥ tab · j k page · f3 vim mode · f2 follow Zed · ⌫ drop key · ctrl-c quit · other keys go to vim";

/// A panel this many times wider than tall, as the bottom dock makes, shows
/// one section at a time under a row of tabs.
const TABS_RATIO: u16 = 4;

impl App {
    pub fn new(keymap: Keymap, os: &str) -> Self {
        Self {
            keymap,
            state: State::new(os),
            follow: true,
            cursor: None,
            collapsed: BTreeSet::from([Category::Scroll]),
            picked: Category::Edit,
            auto: None,
            offset: 0,
            taken: BTreeMap::new(),
            last: None,
            notice: None,
            quit: false,
            keys: false,
            focused: false,
        }
    }

    pub fn on_cursor(&mut self, cursor: Cursor) {
        let at = format!(
            "{}:{}:{} {:?}",
            cursor.uri,
            cursor.line + 1,
            cursor.character + 1,
            cursor.mode
        );
        self.cursor = Some(cursor);
        if self.follow {
            self.follow_cursor();
        }
        self.note(format_args!("cursor {at}"));
    }

    /// Logs what happened with the vim state it left.
    fn note(&self, what: std::fmt::Arguments) {
        crate::log::event(format_args!(
            "{what} | {:?} {} typed={:?} follow={} focused={}",
            self.state.mode,
            self.state.vim_mode(),
            self.state.typed,
            self.follow,
            self.focused
        ));
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
        self.note(format_args!("focus {focused}"));
    }

    /// A key typed into Zed: the simulated vim takes it as Zed's vim did.
    pub fn on_zed_key(&mut self, ev: KeyEvent) {
        let Some(key) = zed_key(ev) else { return };
        if self.focused || !self.follow {
            self.note(format_args!("zed   {key} ignored"));
            return;
        }
        let outcome = self.state.press(&self.keymap, &key);
        self.note(format_args!("zed   {key} -> {outcome:?}"));
        // Text typed in insert mode is unbound; only commands are worth showing.
        if matches!(outcome, Outcome::Ran { .. }) {
            self.last = Some(outcome);
        }
        self.follow_pending();
    }

    /// Shows what pending keys wait for: motions after an operator, text
    /// objects after `i` or `a`.
    fn follow_pending(&mut self) {
        let auto = match self.state.operators.last() {
            Some(op) if matches!(op.id, "i" | "a") => Some(Category::Objects),
            Some(op) if !op.waiting => Some(Category::Motions),
            _ => None,
        };
        if auto != self.auto {
            self.auto = auto;
            self.offset = 0;
        }
    }

    /// The selected section: the one pending keys call for, else the one
    /// picked, else the first.
    fn selected(&self, sections: &[(Category, Vec<Entry>)]) -> usize {
        let find = |c: Category| sections.iter().position(|s| s.0 == c);
        self.auto
            .and_then(find)
            .or_else(|| find(self.picked))
            .unwrap_or(0)
    }

    /// Picks the next or previous section, wrapping around.
    fn step(&mut self, forward: bool) {
        let sections = self.sections();
        let n = sections.len().max(1);
        let i = self.selected(&sections);
        let j = if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        };
        if let Some(&(category, _)) = sections.get(j) {
            self.picked = category;
        }
        self.auto = None;
        self.offset = 0;
    }

    pub fn on_key(&mut self, ev: KeyEvent) {
        self.focused = true;
        let pending = !self.state.prefix.is_empty() || !self.state.operators.is_empty();
        match ev.code {
            KeyCode::Char('c') if ev.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Tab | KeyCode::Down => self.step(true),
            KeyCode::BackTab | KeyCode::Up => self.step(false),
            KeyCode::F(3) => {
                let i = Mode::ALL
                    .iter()
                    .position(|&m| m == self.state.mode)
                    .unwrap_or(0);
                let step = if ev.modifiers.contains(KeyModifiers::SHIFT) {
                    Mode::ALL.len() - 1
                } else {
                    1
                };
                self.state.set_mode(Mode::ALL[(i + step) % Mode::ALL.len()]);
                self.follow = false;
            }
            KeyCode::F(2) => {
                self.follow = true;
                self.follow_cursor();
            }
            // `j` and `k` only mean something to vim after a prefix or operator.
            KeyCode::Char('j') if ev.modifiers.is_empty() && !pending => self.offset += 1,
            KeyCode::Char('k') if ev.modifiers.is_empty() && !pending => {
                self.offset = self.offset.saturating_sub(1)
            }
            KeyCode::Enter => {
                let sections = self.sections();
                if let Some(&(category, _)) = sections.get(self.selected(&sections))
                    && !self.collapsed.remove(&category)
                {
                    self.collapsed.insert(category);
                }
            }
            KeyCode::Backspace if pending => {
                self.state.back();
                self.follow_pending();
            }
            _ => {
                let Some(key) = zed_key(ev) else { return };
                let mode = self.state.mode;
                let outcome = self.state.press(&self.keymap, &key);
                if self.state.mode != mode {
                    self.follow = false;
                }
                self.note(format_args!("panel {key} -> {outcome:?}"));
                if matches!(outcome, Outcome::Ran { .. } | Outcome::Unbound(_)) {
                    self.last = Some(outcome);
                }
                self.follow_pending();
            }
        }
    }

    fn sections(&self) -> Vec<(Category, Vec<Entry>)> {
        let mut entries = self.state.entries(&self.keymap);
        // Letters first, `g` before `G`, then symbols, counts, and named and modified keys.
        entries.sort_by_cached_key(|e| {
            let base = e.key.strip_prefix("shift-").filter(|k| k.len() == 1);
            let base = base.unwrap_or(&e.key);
            let rank = match base.chars().next() {
                _ if base.chars().count() > 1 => 3,
                Some(c) if c.is_ascii_alphabetic() => 0,
                Some(c) if c.is_ascii_digit() => 2,
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
        let area = frame.area();
        let tabs = area.width >= TABS_RATIO * area.height;
        let help_text = if tabs { TABS_HELP } else { HELP };
        let header = self.header();
        // Beside the header when it fits, so a short panel keeps its rows for keys.
        let beside = header.width() + 2 + help_text.chars().count() <= area.width as usize;
        let [top, bar, body, detail, help] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(tabs as u16),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(if beside { 0 } else { 1 }),
        ])
        .areas(area);
        if tabs {
            frame.render_widget(Paragraph::new(self.tab_bar(bar.width as usize)), bar);
        }
        let help_line = Line::from(help_text.dim());
        if beside {
            frame.render_widget(Paragraph::new(help_line.right_aligned()), top);
        } else {
            frame.render_widget(Paragraph::new(help_line), help);
        }
        frame.render_widget(Paragraph::new(header), top);
        self.render_body(frame, body, tabs);
        frame.render_widget(Paragraph::new(self.detail()), detail);
    }

    /// Each section's title and size, the selected one highlighted and,
    /// when they do not all fit, the tabs before it dropped until it does.
    fn tab_bar(&self, width: usize) -> Line<'static> {
        let sections = self.sections();
        let selected = self.selected(&sections);
        let tabs: Vec<String> = sections
            .iter()
            .map(|(category, entries)| format!(" {} {} ", category.title(), entries.len()))
            .collect();
        let mut start = 0;
        while start < selected
            && tabs[start..=selected]
                .iter()
                .map(|t| t.chars().count() + 1)
                .sum::<usize>()
                > width
        {
            start += 1;
        }
        let mut spans = vec![];
        if start > 0 {
            spans.push("‹".dim());
        }
        for (i, tab) in tabs.into_iter().enumerate().skip(start) {
            spans.push(if i == selected {
                tab.bold().reversed()
            } else {
                tab.dim()
            });
            spans.push(" ".into());
        }
        Line::from(spans)
    }

    /// The columns from the selected section's title on, or from where the
    /// last ones fit, `j` and `k` paging through the section's own columns.
    fn render_body(&mut self, frame: &mut Frame, area: Rect, tabs: bool) {
        let (columns, selected_at, span) = self.columns(area.height as usize, tabs);
        let width = area.width as usize;
        let mut last = columns.len().saturating_sub(1);
        let mut used = columns.get(last).map_or(0, |c| c.1);
        while last > 0 && used + columns[last - 1].1 <= width {
            last -= 1;
            used += columns[last].1;
        }
        let start = (selected_at + self.offset.min(span)).min(last);
        self.offset = start.saturating_sub(selected_at);
        let mut x = area.x;
        for (lines, w) in columns.into_iter().skip(start) {
            if x >= area.right() {
                break;
            }
            let w = (w as u16).min(area.right() - x);
            frame.render_widget(
                Paragraph::new(lines),
                Rect {
                    x,
                    width: w,
                    ..area
                },
            );
            x += w;
        }
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
        spans.push(if self.follow && self.keys {
            "  following Zed".dim()
        } else if self.follow {
            "  following Zed's cursor, not its keys".dim()
        } else {
            "  mode picked by hand".dim()
        });
        Line::from(spans)
    }

    /// The sections, or with `tabs` the selected one, flowed top to bottom
    /// into columns `height` rows tall, the column with the selected section's
    /// title and how many columns its section runs on past that one.
    fn columns(&self, height: usize, tabs: bool) -> (Vec<Column>, usize, usize) {
        let height = height.max(1);
        let sections = self.sections();
        let selected = self.selected(&sections);
        // Each column's first section and its titles and entries, by section.
        let mut flow: Vec<(usize, Vec<_>)> = vec![];
        for (i, (category, entries)) in sections.iter().enumerate() {
            // A tab shows its section alone, untitled and never folded.
            if tabs && i != selected {
                continue;
            }
            let open = tabs || !self.collapsed.contains(category);
            let title = (!tabs).then_some(None);
            let items = title
                .into_iter()
                .chain(entries.iter().filter(|_| open).map(Some));
            for item in items {
                // A title never ends a column its entries do not start.
                let need = if item.is_none() && open && !entries.is_empty() {
                    2
                } else {
                    1
                };
                match flow.last_mut() {
                    Some((_, col)) if col.len() + need <= height => col.push((i, item)),
                    _ => flow.push((i, vec![(i, item)])),
                }
            }
        }
        let selected_at = flow
            .iter()
            .position(|(_, col)| col.contains(&(selected, None)))
            .unwrap_or(0);
        let span = flow[selected_at..]
            .iter()
            .skip(1)
            .take_while(|(first, _)| *first == selected)
            .count();
        let mac = self.state.mac();
        let columns = flow
            .into_iter()
            .map(|(_, col)| {
                let keys: Vec<String> = col
                    .iter()
                    .map(|(_, e)| e.map(|e| vim::display(&e.key, mac)).unwrap_or_default())
                    .collect();
                let key_w = keys.iter().map(|k| k.chars().count()).max().unwrap_or(1);
                let key_w = key_w.clamp(1, MAX_KEY);
                let label_w = col
                    .iter()
                    .filter_map(|(_, e)| e.map(|e| e.label.chars().count()))
                    .max()
                    .unwrap_or(0)
                    .min(MAX_LABEL);
                let mut width = key_w + 3 + label_w;
                let lines = col
                    .iter()
                    .zip(&keys)
                    .map(|(&(i, e), key)| match e {
                        Some(e) => cell(e, key, key_w, label_w, &self.taken),
                        None => {
                            let (category, entries) = &sections[i];
                            let open = !self.collapsed.contains(category);
                            let title = format!(
                                "{} {} ({})",
                                if open { '▾' } else { '▸' },
                                category.title(),
                                entries.len()
                            );
                            width = width.max(title.chars().count());
                            let mut style = Style::new().bold();
                            if i == selected {
                                style = style.add_modifier(Modifier::REVERSED);
                            }
                            Line::from(Span::styled(title, style))
                        }
                    })
                    .collect();
                (lines, width + 2)
            })
            .collect();
        (columns, selected_at, span)
    }

    fn detail(&self) -> Line<'_> {
        match &self.last {
            // The action and its context are in the event log.
            Some(Outcome::Ran { keys, label, .. }) => {
                Line::from(format!("{keys} → {label}").bold())
            }
            Some(Outcome::Unbound(keys)) => Line::from(format!("{keys} is not bound here").red()),
            _ => match &self.notice {
                Some(notice) => Line::from(notice.as_str().red()),
                None => Line::from("type a key to see what it does".dim()),
            },
        }
    }
}

/// A column's lines and width.
type Column = (Vec<Line<'static>>, usize);

const MAX_KEY: usize = 12;
const MAX_LABEL: usize = 30;

/// One entry as its column draws it. Keys another app takes are struck out
/// and say which app.
fn cell(
    e: &Entry,
    key: &str,
    key_w: usize,
    label_w: usize,
    taken: &BTreeMap<String, String>,
) -> Line<'static> {
    let key = format!("{:>key_w$}", fit(key, key_w));
    if let Some(command) = taken.get(&e.key) {
        let label = format!("tess {command}: {}", e.label);
        return Line::from(vec![
            key.crossed_out().fg(Color::Red),
            " → ".dim(),
            fit(&label, label_w).dim(),
        ]);
    }
    let color = match e.kind {
        Kind::Group(_) => Color::Magenta,
        Kind::Operator => Color::Yellow,
        Kind::Action => Color::Cyan,
    };
    let label = fit(&e.label, label_w);
    Line::from(vec![
        key.bold().fg(color),
        " → ".dim(),
        match e.kind {
            Kind::Group(_) => label.fg(Color::Magenta),
            _ => label.into(),
        },
    ])
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
    if let Some(path) = crate::log::default_path()
        && let Err(e) = crate::log::open(&path)
    {
        eprintln!("wkey: no event log at {}: {e}", path.display());
    }
    let mut app = App::new(keymap, os);
    app.notice = notice;
    app.taken = crate::whichkey::installed_tess_keys();
    let (tx, keys) = std::sync::mpsc::channel();
    match crate::keys::watch(tx) {
        Ok(()) => app.keys = true,
        Err(e) if cfg!(target_os = "macos") => {
            app.notice.get_or_insert(format!("{e:#}"));
        }
        Err(_) => {}
    }
    crate::log::event(format_args!(
        "start os={os} keys watched={} notice={:?}",
        app.keys, app.notice
    ));
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
            } else {
                let text = String::from_utf8_lossy(&bytes);
                crate::log::event(format_args!("cursor file unreadable: {text}"));
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
    fn short_wide_panels_show_one_section_per_tab() {
        let mut a = app();
        let s = screen(&mut a, 200, 12);
        assert!(
            s[1].contains(" Edit 26 ") && s[1].contains(" Files ") && s[1].contains(" App "),
            "{}",
            s[1]
        );
        assert!(
            shows(&s, "u → undo") && !shows(&s, "d → delete"),
            "{}",
            s.join("\n")
        );
        // The help moves beside the header when it fits.
        assert!(
            s[0].contains("NORMAL") && s[0].contains("ctrl-c quit"),
            "{}",
            s[0]
        );
        assert!(s[0].contains("⇥ ⇧⇥ tab"), "{}", s[0]);
        a.on_key(key(KeyCode::Tab));
        let s = screen(&mut a, 200, 12);
        assert!(
            shows(&s, "d → delete") && !shows(&s, "u → undo"),
            "{}",
            s.join("\n")
        );
        // Scroll is folded, but a tab always shows its keys.
        for _ in 0..4 {
            a.on_key(key(KeyCode::Tab));
        }
        let s = screen(&mut a, 200, 12);
        assert!(
            s[1].contains(" Scroll 11 ") && shows(&s, "⌃F → page down"),
            "{}",
            s.join("\n")
        );
        // Taller panels keep the sections stacked, the help below.
        let tall = screen(&mut app(), 60, 20);
        assert!(tall[1].contains("▾ Edit"), "{}", tall[1]);
        assert!(tall[19].contains("⇥ ↑↓ section"), "{}", tall[19]);
    }

    #[test]
    fn pending_keys_pick_the_tab_until_one_is_picked() {
        let mut a = app();
        a.on_key(key(KeyCode::Tab)); // Operators, by hand
        a.on_focus(false); // back to Zed
        a.on_zed_key(ch('d'));
        let s = screen(&mut a, 200, 12);
        assert!(
            shows(&s, "w → next word start") && shows(&s, "i → inner object"),
            "{}",
            s.join("\n")
        );
        a.on_zed_key(ch('i'));
        assert!(shows(&screen(&mut a, 200, 12), "w → word"));
        a.on_zed_key(ch('w'));
        let s = screen(&mut a, 200, 12);
        assert!(
            shows(&s, "d → delete") && shows(&s, "d i w → delete inner word"),
            "{}",
            s.join("\n")
        );
        // A tab picked while keys wait wins over what they call for.
        a.on_zed_key(ch('d'));
        assert!(shows(&screen(&mut a, 200, 12), "w → next word start"));
        a.on_key(key(KeyCode::BackTab));
        assert!(!shows(&screen(&mut a, 200, 12), "w → next word start"));
    }

    #[test]
    fn the_tab_bar_keeps_the_selected_tab_in_view() {
        let mut a = app();
        a.on_key(key(KeyCode::BackTab)); // wraps to the last tab, App
        let s = screen(&mut a, 80, 12);
        assert!(s[1].starts_with('‹') && s[1].contains(" App "), "{}", s[1]);
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
        assert!(!shows(&s, "vim::StartOfDocument"), "actions are in the log");
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
        assert!(open[1].contains("▾ Edit"), "{}", open[1]);
        assert!(shows(&screen(&mut a, 120, 300), "▸ Scroll"));
        a.on_key(key(KeyCode::Enter));
        let folded = screen(&mut a, 120, 60);
        assert!(folded[1].contains("▸ Edit"), "{}", folded[1]);
        assert!(!shows(&folded, "u → undo"));
        a.on_key(key(KeyCode::Down));
        a.on_key(key(KeyCode::Up));
        a.on_key(key(KeyCode::Enter));
        assert!(shows(&screen(&mut a, 120, 60), "u → undo"));
    }

    #[test]
    fn j_and_k_page_the_selected_section_unless_vim_waits() {
        let mut a = app();
        a.on_key(key(KeyCode::Down));
        a.on_key(key(KeyCode::Down));
        let top = screen(&mut a, 40, 12);
        assert!(shows(&top, "▾ Motions"), "{}", top.join("\n"));
        a.on_key(ch('j'));
        let next = screen(&mut a, 40, 12);
        assert!(
            !shows(&next, "▾ Motions") && shows(&next, " → "),
            "{}",
            next.join("\n")
        );
        a.on_key(ch('k'));
        assert_eq!(screen(&mut a, 40, 12), top);
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
    fn keys_another_app_takes_are_struck_out() {
        let mut a = app();
        a.state.set_mode(Mode::Insert);
        a.taken = [("alt-/".to_string(), "flip".to_string())].into();
        let s = screen(&mut a, 120, 300);
        assert!(shows(&s, "⌥/ → tess flip: which key"), "{}", s.join("\n"));
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
        a.on_key(key(KeyCode::F(3)));
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
