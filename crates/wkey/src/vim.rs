//! What wkey knows about Zed's vim mode: modes, operators, key names, actions.
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    Replace,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Normal, Mode::Insert, Mode::Visual, Mode::Replace];

    /// The `vim_mode` value Zed puts in the editor's key context.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::Insert => "insert",
            Mode::Visual => "visual",
            Mode::Replace => "replace",
        }
    }

    pub fn parse(name: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// A pending vim operator, as Zed's `Operator::id` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Operator {
    pub id: &'static str,
    pub name: &'static str,
    /// Waits for one character (`f`, `r`, `m`...) instead of a motion.
    pub waiting: bool,
}

/// The operator a `vim::Push*` action starts, if any.
pub fn operator(action: &str, args: &Value) -> Option<Operator> {
    let flag = |k: &str| args[k].as_bool().unwrap_or(false);
    let (id, name, waiting) = match action.strip_prefix("vim::Push")? {
        "Object" if flag("around") => ("a", "around object", false),
        "Object" => ("i", "inner object", false),
        "Change" => ("c", "change", false),
        "Delete" => ("d", "delete", false),
        "Yank" => ("y", "yank", false),
        "Replace" => ("r", "replace char", true),
        "Digraph" => ("^K", "digraph", true),
        "Literal" => ("^V", "literal char", true),
        "FindForward" if flag("before") => ("t", "till char", true),
        "FindForward" => ("f", "find char", true),
        "FindBackward" if flag("after") => ("T", "till char backward", true),
        "FindBackward" => ("F", "find char backward", true),
        "Sneak" => ("s", "sneak", true),
        "SneakBackward" => ("S", "sneak backward", true),
        "AddSurrounds" => ("ys", "add surrounds", false),
        "ChangeSurrounds" => ("cs", "change surrounds", false),
        "DeleteSurrounds" => ("ds", "delete surrounds", true),
        "Mark" => ("m", "set mark", true),
        "Jump" if flag("line") => ("'", "jump to mark line", true),
        "Jump" => ("`", "jump to mark", true),
        "Indent" => (">", "indent", false),
        "Outdent" => ("<", "outdent", false),
        "AutoIndent" => ("eq", "auto indent", false),
        "ShellCommand" => ("sh", "filter through shell", false),
        "Rewrap" => ("gq", "rewrap", false),
        "ReplaceWithRegister" => ("gR", "replace with register", false),
        "Exchange" => ("cx", "exchange", false),
        "Uppercase" => ("gU", "uppercase", false),
        "Lowercase" => ("gu", "lowercase", false),
        "OppositeCase" => ("g~", "toggle case", false),
        "Rot13" | "Rot47" => ("g?", "rot13", false),
        "Register" => ("\"", "use register", true),
        "RecordRegister" => ("q", "record macro", true),
        "ReplayRegister" => ("@", "replay macro", true),
        "ToggleComments" => ("gc", "toggle comments", false),
        "ToggleBlockComments" => ("gb", "toggle block comments", false),
        _ => return None,
    };
    Some(Operator { id, name, waiting })
}

/// The `vim_mode` Zed reports while `op` is pending in `mode`.
pub fn pending_mode(op: Operator) -> &'static str {
    match op {
        Operator { id: "^V", .. } => "literal",
        Operator { waiting: true, .. } => "waiting",
        _ => "operator",
    }
}

/// Vim notation for a Zed keystroke: `shift-g` is `G`, `ctrl-w` is `C-w`.
pub fn display(key: &str) -> String {
    let mut mods = String::new();
    let mut shift = false;
    let mut rest = key;
    while let Some((m, tail)) = rest.split_once('-')
        && !tail.is_empty()
    {
        match m {
            "ctrl" => mods.push_str("C-"),
            "alt" => mods.push_str("M-"),
            "cmd" => mods.push_str("D-"),
            "fn" => mods.push_str("fn-"),
            "shift" => shift = true,
            _ => break,
        }
        rest = tail;
    }
    let named = match rest {
        "space" => "SPC",
        "escape" => "ESC",
        "enter" => "RET",
        "tab" => "TAB",
        "backspace" => "BS",
        "delete" => "DEL",
        "pageup" => "PgUp",
        "pagedown" => "PgDn",
        r => r,
    };
    let key = match (shift, named.len(), named.chars().next()) {
        (true, 1, Some(c)) if c.is_ascii_lowercase() => c.to_ascii_uppercase().to_string(),
        (true, ..) => format!("S-{named}"),
        _ => named.to_string(),
    };
    format!("{mods}{key}")
}

/// A short human description: `vim::NextWordStart` is "next word start".
pub fn describe(action: &str, args: &Value) -> String {
    if let Some(op) = operator(action, args) {
        return op.name.into();
    }
    let (namespace, name) = action.split_once("::").unwrap_or(("", action));
    let mut out = match namespace {
        "vim" | "editor" | "" => String::new(),
        ns => format!("{}: ", ns.replace('_', " ")),
    };
    out.push_str(&words(name));
    let details: Vec<String> = args
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| {
            let k = k.replace('_', " ");
            match v {
                Value::Bool(true) => Some(k),
                Value::Number(n) => Some(format!("{k} {n}")),
                Value::String(s) => Some(format!("{k} {s}")),
                _ => None,
            }
        })
        .collect();
    if !details.is_empty() {
        out.push_str(&format!(" ({})", details.join(", ")));
    }
    out
}

/// `NextWordStart` is "next word start".
fn words(camel: &str) -> String {
    let mut out = String::new();
    for c in camel.chars() {
        if c.is_uppercase() && !out.is_empty() {
            out.push(' ');
        }
        out.extend(c.to_lowercase());
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Prefixes,
    Operators,
    Motions,
    Objects,
    Modes,
    Scroll,
    Edit,
    Editor,
    Workspace,
}

impl Category {
    pub fn title(self) -> &'static str {
        match self {
            Category::Prefixes => "Prefixes",
            Category::Operators => "Operators",
            Category::Motions => "Motions",
            Category::Objects => "Text objects",
            Category::Modes => "Modes",
            Category::Scroll => "Scroll",
            Category::Edit => "Edit",
            Category::Editor => "Editor",
            Category::Workspace => "Workspace",
        }
    }
}

pub fn category(action: &str, args: &Value) -> Category {
    if operator(action, args).is_some() {
        return Category::Operators;
    }
    let (namespace, name) = action.split_once("::").unwrap_or(("", action));
    let scroll = name.starts_with("Scroll") || matches!(name, "LineUp" | "LineDown");
    match namespace {
        "vim" if MOTIONS.contains(&name) => Category::Motions,
        "vim" if OBJECTS.contains(&name) => Category::Objects,
        "vim" if scroll => Category::Scroll,
        "vim" if name.starts_with("Insert")
            || name.starts_with("SwitchTo")
            || name.starts_with("ToggleVisual")
            || name.starts_with("ToggleReplace") =>
        {
            Category::Modes
        }
        "vim" => Category::Edit,
        "editor" if scroll => Category::Scroll,
        "editor" => Category::Editor,
        _ => Category::Workspace,
    }
}

/// Zed's vim motions (`crates/vim/src/motion.rs`) and the search motions.
const MOTIONS: &[&str] = &[
    "Left", "WrappingLeft", "Right", "WrappingRight", "Up", "Down", "CurrentLine",
    "SentenceForward", "SentenceBackward", "StartOfParagraph", "EndOfParagraph",
    "StartOfDocument", "EndOfDocument", "GoToPercentage", "NextLineStart",
    "PreviousLineStart", "StartOfLineDownward", "EndOfLineDownward", "GoToColumn",
    "RepeatFind", "RepeatFindReversed", "WindowTop", "WindowMiddle", "WindowBottom",
    "NextSectionStart", "NextSectionEnd", "PreviousSectionStart", "PreviousSectionEnd",
    "NextMethodStart", "NextMethodEnd", "PreviousMethodStart", "PreviousMethodEnd",
    "NextComment", "PreviousComment", "PreviousLesserIndent", "PreviousGreaterIndent",
    "PreviousSameIndent", "NextLesserIndent", "NextGreaterIndent", "NextSameIndent",
    "NextWordStart", "NextWordEnd", "PreviousWordStart", "PreviousWordEnd",
    "NextSubwordStart", "NextSubwordEnd", "PreviousSubwordStart", "PreviousSubwordEnd",
    "FirstNonWhitespace", "Matching", "EndOfLine", "StartOfLine", "MiddleOfLine",
    "UnmatchedForward", "UnmatchedBackward", "Search", "MoveToNext", "MoveToPrevious",
    "MoveToNextMatch", "MoveToPreviousMatch", "Tab",
];

/// Zed's vim text objects (`crates/vim/src/object.rs`).
const OBJECTS: &[&str] = &[
    "Word", "Subword", "Sentence", "Paragraph", "Quotes", "BackQuotes", "MiniQuotes",
    "AnyQuotes", "DoubleQuotes", "VerticalBars", "MiniBrackets", "AnyBrackets", "AnyPair",
    "Argument", "Tag", "Method", "Class", "Comment", "IndentObj", "Parentheses",
    "SquareBrackets", "AngleBrackets", "CurlyBrackets", "EntireFile",
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn modes_round_trip_their_context_names() {
        for m in Mode::ALL {
            assert_eq!(Mode::parse(m.name()), Some(m));
        }
        assert_eq!(Mode::Visual.name(), "visual");
        assert_eq!(Mode::parse("operator"), None);
    }

    #[test]
    fn push_actions_name_zed_operator_ids() {
        let op = |a: &str, args: Value| operator(a, &args).map(|o| (o.id, o.waiting));
        assert_eq!(op("vim::PushDelete", Value::Null), Some(("d", false)));
        assert_eq!(
            op("vim::PushObject", json!({"around": true})),
            Some(("a", false))
        );
        assert_eq!(
            op("vim::PushObject", json!({"around": false})),
            Some(("i", false))
        );
        assert_eq!(
            op("vim::PushFindForward", json!({"before": true})),
            Some(("t", true))
        );
        assert_eq!(
            op("vim::PushFindBackward", json!({"after": false})),
            Some(("F", true))
        );
        assert_eq!(
            op("vim::PushJump", json!({"line": true})),
            Some(("'", true))
        );
        assert_eq!(
            op("vim::PushOppositeCase", Value::Null),
            Some(("g~", false))
        );
        assert_eq!(op("vim::PushAddSurrounds", json!({})), Some(("ys", false)));
        assert_eq!(
            op("vim::PushDeleteSurrounds", Value::Null),
            Some(("ds", true))
        );
        assert_eq!(op("vim::PushForcedMotion", Value::Null), None);
        assert_eq!(op("vim::NextWordStart", Value::Null), None);
    }

    #[test]
    fn pending_operators_change_the_reported_mode() {
        let mode = |a: &str| pending_mode(operator(a, &Value::Null).unwrap());
        assert_eq!(mode("vim::PushDelete"), "operator");
        assert_eq!(mode("vim::PushMark"), "waiting");
        assert_eq!(mode("vim::PushLiteral"), "literal");
    }

    #[test]
    fn keys_use_vim_notation() {
        assert_eq!(display("shift-g"), "G");
        assert_eq!(display("g"), "g");
        assert_eq!(display("ctrl-w"), "C-w");
        assert_eq!(display("ctrl-shift-w"), "C-W");
        assert_eq!(display("alt-shift-tab"), "M-S-TAB");
        assert_eq!(display("cmd-k"), "D-k");
        assert_eq!(display("space"), "SPC");
        assert_eq!(display("escape"), "ESC");
        assert_eq!(display("ctrl--"), "C--");
    }

    #[test]
    fn descriptions_read_like_words() {
        assert_eq!(
            describe("vim::NextWordStart", &Value::Null),
            "next word start"
        );
        assert_eq!(
            describe("vim::NextWordStart", &json!({"ignore_punctuation": true})),
            "next word start (ignore punctuation)"
        );
        assert_eq!(
            describe("pane::DeploySearch", &Value::Null),
            "pane: deploy search"
        );
        assert_eq!(
            describe("editor::GoToDefinition", &Value::Null),
            "go to definition"
        );
        assert_eq!(describe("vim::PushDelete", &Value::Null), "delete");
        assert_eq!(
            describe("workspace::ActivatePane", &json!({"index": 2})),
            "workspace: activate pane (index 2)"
        );
        assert_eq!(
            describe("task::Spawn", &json!({"task_name": "wkey"})),
            "task: spawn (task name wkey)"
        );
    }

    #[test]
    fn actions_fall_into_sections() {
        let cat = |a: &str| category(a, &Value::Null);
        assert_eq!(cat("vim::PushYank"), Category::Operators);
        assert_eq!(cat("vim::NextWordStart"), Category::Motions);
        assert_eq!(cat("vim::MoveToNextMatch"), Category::Motions);
        assert_eq!(cat("vim::Word"), Category::Objects);
        assert_eq!(cat("vim::InsertBefore"), Category::Modes);
        assert_eq!(cat("vim::ToggleVisualLine"), Category::Modes);
        assert_eq!(cat("vim::ScrollDown"), Category::Scroll);
        assert_eq!(cat("vim::Undo"), Category::Edit);
        assert_eq!(cat("editor::GoToDefinition"), Category::Editor);
        assert_eq!(cat("pane::DeploySearch"), Category::Workspace);
        assert_eq!(Category::Objects.title(), "Text objects");
    }
}
