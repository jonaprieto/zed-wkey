//! The which-key model: the keys available in a vim state, and what pressing one does.
use crate::context::KeyContext;
use crate::keymap::Keymap;
use crate::vim::{self, Category, Mode, Operator};

/// Zed's bundled keymaps for `os` ("macos" or "linux"): the defaults, then vim.
pub fn bundled(os: &str) -> Keymap {
    let defaults = match os {
        "macos" => include_str!("../keymaps/default-macos.json"),
        _ => include_str!("../keymaps/default-linux.json"),
    };
    let mut keymap = Keymap::default();
    for src in [defaults, include_str!("../keymaps/vim.json")] {
        keymap.load(src).expect("bundled keymaps parse");
    }
    keymap
}

/// The bundled keymaps with the user's keymap on top. A user keymap that does
/// not parse is skipped and its error returned, so the view still works.
pub fn with_user(os: &str, user: Option<&str>) -> (Keymap, Option<String>) {
    let mut keymap = bundled(os);
    let mut mine = Keymap::default();
    let error = match user.map(|src| mine.load(src)) {
        Some(Err(e)) => Some(format!("skipped your keymap.json: {e:#}")),
        _ => {
            keymap.bindings.extend(mine.bindings);
            None
        }
    };
    (keymap, error)
}

/// Where the cursor is and what has been typed so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub mode: Mode,
    /// The file extension at the cursor, for `extension == md` contexts.
    pub extension: Option<String>,
    pub os: String,
    pub operators: Vec<Operator>,
    /// Keys of an unfinished multi-key binding.
    pub prefix: Vec<String>,
    /// Everything typed since the last finished command, for display.
    pub typed: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Leads to this many bindings.
    Group(usize),
    Operator,
    Action,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: String,
    pub label: String,
    pub category: Category,
    pub kind: Kind,
    /// The full action name, e.g. `vim::NextWordStart`.
    pub action: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Prefix,
    Operator(Operator),
    /// A command finished: what was typed, what it does, the action and its context.
    Ran {
        keys: String,
        label: String,
        action: String,
        context: String,
    },
    Unbound(String),
}

impl State {
    pub fn new(os: &str) -> Self {
        Self {
            os: os.into(),
            ..Self::default()
        }
    }

    pub fn mac(&self) -> bool {
        self.os == "macos"
    }

    /// The `vim_mode` Zed reports, which a pending operator replaces.
    pub fn vim_mode(&self) -> &'static str {
        self.operators
            .last()
            .map_or(self.mode.name(), |&op| vim::pending_mode(op))
    }

    /// Workspace > Pane > Editor, as Zed builds it for a focused vim editor.
    // ponytail: only the contexts a plain focused editor has; no menus, panels or completions.
    pub fn stack(&self) -> Vec<KeyContext> {
        let mut workspace = KeyContext::default();
        workspace.add("Workspace").set("os", &self.os);
        let mut pane = KeyContext::default();
        pane.add("Pane");
        let mut editor = KeyContext::default();
        editor.add("Editor").set("os", &self.os).set("mode", "full");
        if let Some(ext) = &self.extension {
            editor.set("extension", ext);
        }
        let mode = self.vim_mode();
        if matches!(mode, "normal" | "visual" | "operator") {
            editor.add("VimControl");
        }
        // Zed names the operator only while it waits for a motion.
        let operator = match self.operators.last() {
            Some(op) if mode == "operator" => op.id,
            _ => "none",
        };
        editor.set("vim_mode", mode).set("vim_operator", operator);
        vec![workspace, pane, editor]
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.reset();
    }

    fn reset(&mut self) {
        self.operators.clear();
        self.prefix.clear();
        self.typed.clear();
    }

    /// Undoes the last key: a prefix key first, then an operator.
    pub fn back(&mut self) {
        if self.prefix.pop().is_some() || self.operators.pop().is_some() {
            self.typed.pop();
        }
    }

    pub fn entries(&self, keymap: &Keymap) -> Vec<Entry> {
        let active = keymap.active(&self.stack());
        active
            .next_keys(&self.prefix)
            .into_iter()
            .map(|(key, r)| {
                let action = r.exact.and_then(|b| b.action.as_ref());
                let described = action.map(|(name, args)| vim::describe(name, args));
                let (label, category, kind) = if r.pending {
                    let mut input = self.prefix.clone();
                    input.push(key.clone());
                    let n = active.next_keys(&input).len();
                    let name = group_name(&input).map(String::from).or(described);
                    let label = format!("+{}", name.unwrap_or_else(|| format!("{n} keys")));
                    (label, Category::Prefixes, Kind::Group(n))
                } else {
                    let (name, args) = action.expect("an entry is pending or bound");
                    let kind = match vim::operator(name, args) {
                        Some(_) => Kind::Operator,
                        None => Kind::Action,
                    };
                    (
                        described.unwrap_or_default(),
                        vim::category(name, args),
                        kind,
                    )
                };
                Entry {
                    key,
                    label,
                    category,
                    kind,
                    action: action.map(|(name, _)| name.clone()),
                }
            })
            .collect()
    }

    pub fn press(&mut self, keymap: &Keymap, key: &str) -> Outcome {
        let key = crate::keymap::canonical(key);
        let mut input = self.prefix.clone();
        input.push(key.clone());
        self.typed.push(key.clone());
        let r = keymap.resolve(&input, &self.stack());
        if r.pending {
            self.prefix = input;
            return Outcome::Prefix;
        }
        let keys = self
            .typed
            .iter()
            .map(|k| vim::display(k, self.mac()))
            .collect::<Vec<_>>()
            .join(" ");
        let waiting = self.operators.last().copied().filter(|op| op.waiting);
        let outcome = match (r.exact, waiting) {
            (Some(binding), _) => {
                let (name, args) = binding.action.as_ref().expect("resolve skips null");
                if let Some(op) = vim::operator(name, args) {
                    self.prefix.clear();
                    self.operators.push(op);
                    return Outcome::Operator(op);
                }
                self.mode = vim::mode_after(self.mode, name, self.operators.first().copied());
                Outcome::Ran {
                    keys,
                    label: vim::describe(name, args),
                    action: name.clone(),
                    context: binding.source.clone(),
                }
            }
            (None, Some(op)) => Outcome::Ran {
                keys,
                label: format!("{} {}", op.name, vim::display(&key, self.mac())),
                action: String::new(),
                context: String::new(),
            },
            (None, None) => Outcome::Unbound(keys),
        };
        self.reset();
        outcome
    }
}

/// Names for the common prefixes, as which-key plugins show them.
fn group_name(keys: &[String]) -> Option<&'static str> {
    Some(match keys.join(" ").as_str() {
        "g" => "go to / more",
        "z" => "fold / scroll",
        "ctrl-w" => "window",
        "[" => "previous",
        "]" => "next",
        "space" => "leader",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::LazyLock;

    static KEYMAP: LazyLock<Keymap> = LazyLock::new(|| bundled("macos"));

    fn entry(state: &State, key: &str) -> Option<Entry> {
        state.entries(&KEYMAP).into_iter().find(|e| e.key == key)
    }

    fn action(state: &State, key: &str) -> Option<String> {
        entry(state, key)?.action
    }

    fn typed(state: &mut State, keys: &str) -> Outcome {
        let mut last = Outcome::Prefix;
        for k in keys.split_whitespace() {
            last = state.press(&KEYMAP, k);
        }
        last
    }

    #[test]
    fn the_stack_carries_mode_operator_and_extension() {
        let mut s = State::new("macos");
        s.extension = Some("rs".into());
        let p = |src: &str| crate::context::Predicate::parse(src).unwrap();
        let stack = s.stack();
        assert!(
            p("Workspace > Pane > Editor && mode == full")
                .depth_of(&stack)
                .is_some()
        );
        assert!(p("VimControl && vim_mode == normal && vim_operator == none && extension == rs && os == macos")
            .depth_of(&stack)
            .is_some());
        s.set_mode(Mode::Insert);
        assert!(p("VimControl").depth_of(&s.stack()).is_none());
    }

    #[test]
    fn normal_mode_offers_motions_operators_and_prefixes() {
        let s = State::new("macos");
        assert_eq!(action(&s, "w").as_deref(), Some("vim::NextWordStart"));
        assert_eq!(entry(&s, "w").unwrap().category, Category::Motions);
        assert_eq!(entry(&s, "d").unwrap().kind, Kind::Operator);
        let g = entry(&s, "g").unwrap();
        assert!(matches!(g.kind, Kind::Group(n) if n > 20), "{g:?}");
        assert_eq!(g.category, Category::Prefixes);
        assert_eq!(entry(&s, "shift-g").unwrap().label, "end of document");
    }

    #[test]
    fn a_prefix_key_shows_what_can_follow() {
        let mut s = State::new("macos");
        assert_eq!(typed(&mut s, "g"), Outcome::Prefix);
        assert_eq!(action(&s, "g").as_deref(), Some("vim::StartOfDocument"));
        assert_eq!(action(&s, "d").as_deref(), Some("editor::GoToDefinition"));
        assert!(entry(&s, "w").is_some_and(|e| e.label != "next word start"));
        s.back();
        assert!(s.prefix.is_empty());
        assert_eq!(action(&s, "w").as_deref(), Some("vim::NextWordStart"));
    }

    #[test]
    fn operators_switch_to_their_pending_bindings() {
        let mut s = State::new("macos");
        assert!(matches!(
            typed(&mut s, "d"),
            Outcome::Operator(Operator { id: "d", .. })
        ));
        assert_eq!(s.vim_mode(), "operator");
        assert_eq!(action(&s, "d").as_deref(), Some("vim::CurrentLine"));
        assert_eq!(action(&s, "w").as_deref(), Some("vim::NextWordStart"));
        assert_eq!(entry(&s, "i").unwrap().kind, Kind::Operator);
        typed(&mut s, "i");
        assert_eq!(action(&s, "w").as_deref(), Some("vim::Word"));
        let done = typed(&mut s, "w");
        assert_eq!(
            done,
            Outcome::Ran {
                keys: "d i w".into(),
                label: "word".into(),
                action: "vim::Word".into(),
                context: "vim_operator == a || vim_operator == i || vim_operator == cs || vim_operator == helix_next || vim_operator == helix_previous".into(),
            }
        );
        assert!(s.operators.is_empty() && s.typed.is_empty());
        assert_eq!(s.vim_mode(), "normal");
    }

    #[test]
    fn waiting_operators_take_any_character() {
        let mut s = State::new("macos");
        typed(&mut s, "f");
        assert_eq!(s.vim_mode(), "waiting");
        assert!(matches!(typed(&mut s, "x"), Outcome::Ran { ref keys, .. } if keys == "f x"));
        assert!(s.operators.is_empty());
    }

    #[test]
    fn unbound_keys_reset_and_mode_actions_switch_mode() {
        let mut s = State::new("macos");
        typed(&mut s, "g");
        assert_eq!(typed(&mut s, "f13"), Outcome::Unbound("g f13".into()));
        assert!(s.prefix.is_empty());
        typed(&mut s, "i");
        assert_eq!(s.mode, Mode::Insert);
        assert_eq!(action(&s, "w"), None);
        typed(&mut s, "escape");
        assert_eq!(s.mode, Mode::Normal);
        typed(&mut s, "shift-v");
        assert_eq!(s.mode, Mode::Visual);
        // In visual mode `d` deletes the selection right away.
        assert_eq!(entry(&s, "d").unwrap().kind, Kind::Action);
    }

    #[test]
    fn extension_contexts_follow_the_file() {
        let mut s = State::new("macos");
        let has_md_binding = |s: &State| {
            s.entries(&KEYMAP)
                .iter()
                .any(|e| e.action.as_deref() == Some("markdown::OpenPreview"))
        };
        assert!(!has_md_binding(&s));
        s.extension = Some("md".into());
        s.set_mode(Mode::Insert);
        assert!(has_md_binding(&s));
    }

    #[test]
    fn user_bindings_win_and_broken_user_keymaps_are_skipped() {
        let user = r#"[{"context": "vim_mode == normal", "bindings": {"w": "editor::SelectAll"}}]"#;
        let (keymap, error) = with_user("macos", Some(user));
        assert_eq!(error, None);
        let s = State::new("macos");
        let w = s
            .entries(&keymap)
            .into_iter()
            .find(|e| e.key == "w")
            .unwrap();
        assert_eq!(w.action.as_deref(), Some("editor::SelectAll"));
        let (keymap, error) = with_user("macos", Some("[{"));
        assert!(error.is_some_and(|e| e.contains("keymap.json")));
        assert_eq!(keymap.bindings.len(), KEYMAP.bindings.len());
    }
}
