//! Zed's key context predicates (gpui `keymap/context.rs`, v1.28.1), ported so
//! wkey decides which bindings apply exactly as Zed does.
use anyhow::{Context as _, Result, bail};
use std::collections::BTreeMap;

/// One element's context: identifiers (`Editor`) and pairs (`vim_mode=normal`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyContext(BTreeMap<String, Option<String>>);

impl KeyContext {
    pub fn add(&mut self, id: &str) -> &mut Self {
        self.0.insert(id.into(), None);
        self
    }

    pub fn set(&mut self, key: &str, value: &str) -> &mut Self {
        self.0.insert(key.into(), Some(value.into()));
        self
    }

    fn contains(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key)?.as_deref()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Predicate {
    Identifier(String),
    Equal(String, String),
    NotEqual(String, String),
    Descendant(Box<Predicate>, Box<Predicate>),
    Not(Box<Predicate>),
    And(Box<Predicate>, Box<Predicate>),
    Or(Box<Predicate>, Box<Predicate>),
}

const CHILD: u32 = 1;
const OR: u32 = 2;
const AND: u32 = 3;
const EQ: u32 = 4;
const NOT: u32 = 5;

type Op = fn(Predicate, Predicate) -> Result<Predicate>;

impl Predicate {
    pub fn parse(source: &str) -> Result<Self> {
        let (predicate, rest) = Self::expr(source.trim_start(), 0)?;
        if let Some(next) = rest.chars().next() {
            bail!("unexpected character {next:?}");
        }
        Ok(predicate)
    }

    /// The deepest prefix of the stack (outermost first) the predicate matches.
    pub fn depth_of(&self, stack: &[KeyContext]) -> Option<usize> {
        (0..=stack.len())
            .rev()
            .find(|&depth| self.eval(&stack[..depth], stack))
    }

    fn eval(&self, contexts: &[KeyContext], all: &[KeyContext]) -> bool {
        let Some(context) = contexts.last() else {
            return false;
        };
        match self {
            Self::Identifier(name) => context.contains(name),
            Self::Equal(k, v) => context.get(k) == Some(v),
            Self::NotEqual(k, v) => context.get(k).is_none_or(|x| x != v),
            Self::Not(p) => !(0..all.len()).any(|i| p.eval(&all[..=i], all)),
            Self::Descendant(parent, child) => (0..contexts.len() - 1)
                .find(|&i| parent.eval(&contexts[..=i], all))
                .is_some_and(|i| child.eval(&contexts[i + 1..], &contexts[i + 1..])),
            Self::And(l, r) => l.eval(contexts, all) && r.eval(contexts, all),
            Self::Or(l, r) => l.eval(contexts, all) || r.eval(contexts, all),
        }
    }

    fn expr(mut source: &str, min: u32) -> Result<(Self, &str)> {
        let (mut predicate, rest) = Self::primary(source)?;
        source = rest;
        'parse: loop {
            for (op, precedence, build) in [
                (
                    ">",
                    CHILD,
                    (|a, b| Ok(Self::Descendant(Box::new(a), Box::new(b)))) as Op,
                ),
                ("&&", AND, |a, b| Ok(Self::And(Box::new(a), Box::new(b)))),
                ("||", OR, |a, b| Ok(Self::Or(Box::new(a), Box::new(b)))),
                ("==", EQ, |a, b| Ok(Self::Equal(ident(a)?, ident(b)?))),
                ("!=", EQ, |a, b| Ok(Self::NotEqual(ident(a)?, ident(b)?))),
            ] {
                if source.starts_with(op) && precedence >= min {
                    let (right, rest) =
                        Self::expr(source[op.len()..].trim_start(), precedence + 1)?;
                    predicate = build(predicate, right)?;
                    source = rest;
                    continue 'parse;
                }
            }
            return Ok((predicate, source));
        }
    }

    fn primary(source: &str) -> Result<(Self, &str)> {
        let next = source.chars().next().context("unexpected end")?;
        match next {
            '(' => {
                let (p, rest) = Self::expr(source[1..].trim_start(), 0)?;
                let rest = rest.strip_prefix(')').context("expected a ')'")?;
                Ok((p, rest.trim_start()))
            }
            '!' => {
                let (p, rest) = Self::expr(source[1..].trim_start(), NOT)?;
                Ok((Self::Not(Box::new(p)), rest))
            }
            _ if identifier_char(next) => {
                let len = source
                    .find(|c| !identifier_char(c) && !operator_char(c))
                    .unwrap_or(source.len());
                let (name, rest) = source.split_at(len);
                Ok((Self::Identifier(name.into()), rest.trim_start()))
            }
            _ if operator_char(next) => {
                let (name, rest) = source.split_at(1);
                Ok((Self::Identifier(name.into()), rest.trim_start()))
            }
            _ => bail!("unexpected character {next:?}"),
        }
    }
}

fn ident(p: Predicate) -> Result<String> {
    match p {
        Predicate::Identifier(s) => Ok(s),
        _ => bail!("operands of == and != must be identifiers"),
    }
}

fn identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Vim operator ids such as `>`, `g~` and `"` are values in `vim_operator == x`.
fn operator_char(c: char) -> bool {
    matches!(c, '>' | '<' | '~' | '"' | '?')
}

#[cfg(test)]
mod tests {
    use super::Predicate::*;
    use super::*;

    fn id(s: &str) -> Box<Predicate> {
        Box::new(Identifier(s.into()))
    }

    fn ctx(ids: &[&str], pairs: &[(&str, &str)]) -> KeyContext {
        let mut c = KeyContext::default();
        ids.iter().for_each(|i| {
            c.add(i);
        });
        pairs.iter().for_each(|(k, v)| {
            c.set(k, v);
        });
        c
    }

    fn editor(pairs: &[(&str, &str)], ids: &[&str]) -> Vec<KeyContext> {
        let mut e = ctx(&["Editor"], pairs);
        ids.iter().for_each(|i| {
            e.add(i);
        });
        vec![ctx(&["Workspace"], &[]), ctx(&["Pane"], &[]), e]
    }

    #[test]
    fn and_binds_tighter_than_or_and_not_tighter_than_and() {
        assert_eq!(
            Predicate::parse("a && b || c").unwrap(),
            Or(Box::new(And(id("a"), id("b"))), id("c"))
        );
        assert_eq!(
            Predicate::parse("!menu && x").unwrap(),
            And(Box::new(Not(id("menu"))), id("x"))
        );
        assert_eq!(
            Predicate::parse("(a || b) && c").unwrap(),
            And(Box::new(Or(id("a"), id("b"))), id("c"))
        );
    }

    #[test]
    fn vim_operator_values_may_be_symbols() {
        for op in [">", "<", "g~", "g?", "\"", "gU"] {
            assert_eq!(
                Predicate::parse(&format!("vim_operator == {op}")).unwrap(),
                Equal("vim_operator".into(), op.into())
            );
        }
        assert_eq!(
            Predicate::parse("Picker > Editor").unwrap(),
            Descendant(id("Picker"), id("Editor"))
        );
    }

    #[test]
    fn malformed_predicates_are_errors() {
        for bad in ["a &&", "a == !b", "a b", "(a", "&"] {
            assert!(Predicate::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn depth_is_the_innermost_matching_element() {
        let stack = editor(&[("vim_mode", "normal")], &["VimControl"]);
        let depth = |s: &str| Predicate::parse(s).unwrap().depth_of(&stack);
        assert_eq!(depth("Workspace"), Some(1));
        assert_eq!(depth("Editor && vim_mode == normal"), Some(3));
        assert_eq!(depth("Pane > Editor"), Some(3));
        assert_eq!(depth("Picker > Editor"), None);
        assert_eq!(depth("vim_mode == insert"), None);
        // A missing key is "not equal" to anything.
        assert_eq!(depth("extension != md"), Some(3));
    }

    #[test]
    fn negation_looks_at_the_whole_stack() {
        let plain = editor(&[], &["VimControl"]);
        let menu = editor(&[], &["VimControl", "menu"]);
        let p = Predicate::parse("VimControl && !menu").unwrap();
        assert_eq!(p.depth_of(&plain), Some(3));
        assert_eq!(p.depth_of(&menu), None);
        // `!Editor` is false even at the Workspace depth, because Editor is on the stack.
        assert_eq!(Predicate::parse("!Editor").unwrap().depth_of(&plain), None);
    }
}
