//! Zed keymap files (JSON with comments) and Zed's rules for which binding wins.
use crate::context::{KeyContext, Predicate};
use anyhow::{Context as _, Result};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    /// Canonical keystrokes, e.g. `["g", "shift-e"]`.
    pub keys: Vec<String>,
    /// Action name and arguments; `None` for `null`, which disables a key.
    pub action: Option<(String, Value)>,
    pub context: Option<Predicate>,
    /// The context as written, for showing where a binding comes from.
    pub source: String,
}

#[derive(Clone, Debug, Default)]
pub struct Keymap {
    /// In load order: later files and later sections override earlier ones.
    pub bindings: Vec<Binding>,
}

/// What a key sequence does in a context stack.
#[derive(Debug, PartialEq)]
pub struct Resolved<'a> {
    pub exact: Option<&'a Binding>,
    /// Some longer binding starts with this sequence.
    pub pending: bool,
}

/// Turns JSON with `//` and `/* */` comments and trailing commas into JSON.
pub fn strip_jsonc(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                out.push(c);
                while let Some(c) = chars.next() {
                    out.push(c);
                    match c {
                        '\\' => out.extend(chars.next()),
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                chars.by_ref().find(|&c| c == '\n');
                out.push('\n');
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut last = ' ';
                chars
                    .by_ref()
                    .find(|&c| std::mem::replace(&mut last, c) == '*' && c == '/');
            }
            '}' | ']' => {
                let kept = out.trim_end().len();
                if out[..kept].ends_with(',') {
                    out.truncate(kept - 1);
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

const MODIFIERS: [&str; 5] = ["ctrl", "alt", "shift", "cmd", "fn"];

/// One keystroke with its modifiers in a fixed order: `shift-ctrl-x` is `ctrl-shift-x`.
pub fn canonical(keystroke: &str) -> String {
    let mut mods = vec![];
    let mut rest = keystroke;
    while let Some((m, tail)) = rest.split_once('-')
        && !tail.is_empty()
    {
        let m = match m {
            "control" => "ctrl",
            "option" => "alt",
            "super" | "win" | "command" => "cmd",
            m => m,
        };
        let Some(i) = MODIFIERS.iter().position(|&x| x == m) else {
            break;
        };
        mods.push(i);
        rest = tail;
    }
    mods.sort();
    mods.dedup();
    let mut out: Vec<&str> = mods.into_iter().map(|i| MODIFIERS[i]).collect();
    out.push(rest);
    out.join("-")
}

impl Keymap {
    /// Appends the bindings of one keymap file.
    pub fn load(&mut self, src: &str) -> Result<()> {
        let sections: Vec<Value> =
            serde_json::from_str(&strip_jsonc(src)).context("keymap is not a JSON array")?;
        for section in sections {
            let source = section["context"].as_str().unwrap_or_default().to_string();
            let context = match source.trim() {
                "" => None,
                s => Some(Predicate::parse(s).with_context(|| format!("context {s:?}"))?),
            };
            let Some(bindings) = section["bindings"].as_object() else {
                continue;
            };
            for (keys, action) in bindings {
                let action = match action {
                    Value::Null => None,
                    Value::String(name) => Some((name.clone(), Value::Null)),
                    Value::Array(a) => match a.first() {
                        Some(Value::String(name)) => {
                            Some((name.clone(), a.get(1).cloned().unwrap_or_default()))
                        }
                        _ => continue,
                    },
                    _ => continue,
                };
                self.bindings.push(Binding {
                    keys: keys.split_whitespace().map(canonical).collect(),
                    action,
                    context: context.clone(),
                    source: source.clone(),
                });
            }
        }
        Ok(())
    }

    /// The bindings whose context matches `stack`, with their depth. Evaluating
    /// the predicates once keeps listing many keys cheap.
    pub fn active(&self, stack: &[KeyContext]) -> Active<'_> {
        Active(
            self.bindings
                .iter()
                .enumerate()
                .filter_map(|(ix, b)| {
                    let depth = match &b.context {
                        Some(p) => p.depth_of(stack)?,
                        None => stack.len(),
                    };
                    Some((depth, ix, b))
                })
                .collect(),
        )
    }

    pub fn resolve(&self, input: &[String], stack: &[KeyContext]) -> Resolved<'_> {
        self.active(stack).resolve(input)
    }

    pub fn next_keys(
        &self,
        prefix: &[String],
        stack: &[KeyContext],
    ) -> Vec<(String, Resolved<'_>)> {
        self.active(stack).next_keys(prefix)
    }
}

/// A keymap seen from one context stack: (depth, load index, binding).
pub struct Active<'a>(Vec<(usize, usize, &'a Binding)>);

impl<'a> Active<'a> {
    /// Zed's resolution: the deepest matching context wins, then the later binding.
    // ponytail: every binding counts as a user binding, so a `null` hides all
    // weaker ones; Zed lets a user binding beat a base-keymap `null`.
    pub fn resolve(&self, input: &[String]) -> Resolved<'a> {
        let winner = self
            .0
            .iter()
            .filter(|(_, _, b)| b.keys == input)
            .max_by_key(|&&(depth, ix, _)| (depth, ix));
        let first = winner.map(|&(_, ix, _)| ix);
        let mut pending = std::collections::HashSet::new();
        for &(_, ix, b) in &self.0 {
            if b.keys.len() <= input.len() || !b.keys.starts_with(input) {
                continue;
            }
            if first.is_some_and(|first| first > ix) {
                continue;
            }
            if b.action.is_some() {
                pending.insert(&b.keys);
            } else {
                pending.remove(&b.keys);
            }
        }
        Resolved {
            exact: winner.map(|&(_, _, b)| b).filter(|b| b.action.is_some()),
            pending: !pending.is_empty(),
        }
    }

    /// The keys that can follow `prefix`, each with what pressing it does.
    pub fn next_keys(&self, prefix: &[String]) -> Vec<(String, Resolved<'a>)> {
        let next: std::collections::BTreeSet<&String> = self
            .0
            .iter()
            .filter(|(_, _, b)| b.keys.len() > prefix.len() && b.keys.starts_with(prefix))
            .map(|(_, _, b)| &b.keys[prefix.len()])
            .collect();
        next.into_iter()
            .filter_map(|key| {
                let mut input = prefix.to_vec();
                input.push(key.clone());
                let r = self.resolve(&input);
                (r.exact.is_some() || r.pending).then(|| (key.clone(), r))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn keys(s: &str) -> Vec<String> {
        s.split_whitespace().map(canonical).collect()
    }

    fn editor(mode: &str) -> Vec<KeyContext> {
        let mut e = KeyContext::default();
        e.add("Editor").set("vim_mode", mode);
        let mut w = KeyContext::default();
        w.add("Workspace");
        vec![w, e]
    }

    fn keymap(src: &str) -> Keymap {
        let mut k = Keymap::default();
        k.load(src).unwrap();
        k
    }

    fn action<'a>(k: &'a Keymap, input: &str, mode: &str) -> Option<&'a str> {
        let r = k.resolve(&keys(input), &editor(mode));
        r.exact
            .and_then(|b| b.action.as_ref())
            .map(|(n, _)| n.as_str())
    }

    #[test]
    fn comments_and_trailing_commas_go_but_strings_stay() {
        let src = r#"[ // top
          { /* block */ "bindings": { "g /": "a::B", "\"": "c::D", "x": "e::F", }, },
        ]"#;
        let v: Value = serde_json::from_str(&strip_jsonc(src)).unwrap();
        assert_eq!(
            v,
            json!([{"bindings": {"g /": "a::B", "\"": "c::D", "x": "e::F"}}])
        );
    }

    #[test]
    fn modifiers_are_ordered_and_dash_keys_survive() {
        assert_eq!(canonical("shift-ctrl-x"), "ctrl-shift-x");
        assert_eq!(canonical("alt-cmd-k"), "alt-cmd-k");
        assert_eq!(canonical("cmd-alt-k"), "alt-cmd-k");
        assert_eq!(canonical("ctrl--"), "ctrl--");
        assert_eq!(canonical("-"), "-");
        assert_eq!(canonical("shift-tab"), "shift-tab");
    }

    #[test]
    fn loads_string_array_and_null_actions() {
        let k = keymap(
            r#"[{"context": "Editor", "bindings": {
                "g e": "vim::PreviousWordEnd",
                "f": ["vim::PushFindForward", {"before": false}],
                "x": null }}]"#,
        );
        assert_eq!(k.bindings.len(), 3);
        assert_eq!(k.bindings[0].keys, ["g", "e"]);
        assert_eq!(k.bindings[0].source, "Editor");
        assert_eq!(
            k.bindings[1].action,
            Some(("vim::PushFindForward".into(), json!({"before": false})))
        );
        assert_eq!(k.bindings[2].action, None);
        assert!(
            Keymap::default()
                .load("[{\"context\": \"a &&\", \"bindings\": {}}]")
                .is_err()
        );
    }

    #[test]
    fn deeper_context_beats_later_binding() {
        let k = keymap(
            r#"[{"context": "Editor", "bindings": {"j": "deep::A"}},
                {"context": "Workspace", "bindings": {"j": "shallow::B"}},
                {"bindings": {"k": "first::C"}},
                {"bindings": {"k": "second::D"}}]"#,
        );
        assert_eq!(action(&k, "j", "normal"), Some("deep::A"));
        assert_eq!(action(&k, "k", "normal"), Some("second::D"));
    }

    #[test]
    fn null_disables_and_mode_filters() {
        let k = keymap(
            r#"[{"context": "vim_mode == normal", "bindings": {"x": "vim::DeleteRight", "i": "vim::InsertBefore"}},
                {"context": "Editor", "bindings": {"x": null}}]"#,
        );
        assert_eq!(action(&k, "x", "normal"), None);
        assert_eq!(action(&k, "i", "normal"), Some("vim::InsertBefore"));
        assert_eq!(action(&k, "i", "insert"), None);
    }

    #[test]
    fn longer_bindings_make_a_key_pending_unless_disabled() {
        let k = keymap(
            r#"[{"bindings": {"g g": "a::Top", "g d": "a::Def", "z z": "a::Center"}},
                {"bindings": {"z z": null}}]"#,
        );
        let stack = editor("normal");
        assert_eq!(
            k.resolve(&keys("g"), &stack),
            Resolved {
                exact: None,
                pending: true
            }
        );
        assert!(!k.resolve(&keys("z"), &stack).pending);
        assert!(!k.resolve(&keys("g g"), &stack).pending);
        let next: Vec<_> = k
            .next_keys(&keys("g"), &stack)
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        assert_eq!(next, ["d", "g"]);
        // `z` only leads to a disabled binding, so it is not offered.
        let top: Vec<_> = k
            .next_keys(&[], &stack)
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        assert_eq!(top, ["g"]);
    }
}
