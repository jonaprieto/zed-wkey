//! A language server that answers nothing and only watches: Zed sends every
//! server the edits and asks each for code actions at the selection, which is
//! enough to follow the cursor and guess the vim mode.
use crate::vim::Mode;
use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// Where the cursor is, as the terminal view reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cursor {
    pub uri: String,
    pub line: u64,
    pub character: u64,
    pub mode: Mode,
}

impl Cursor {
    pub fn extension(&self) -> Option<String> {
        let name = self.uri.rsplit('/').next()?;
        let (stem, ext) = name.rsplit_once('.')?;
        (!stem.is_empty()).then(|| ext.to_string())
    }

    pub fn to_json(&self) -> Value {
        json!({"uri": self.uri, "line": self.line, "character": self.character, "mode": self.mode.name()})
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            uri: v["uri"].as_str()?.into(),
            line: v["line"].as_u64()?,
            character: v["character"].as_u64()?,
            mode: Mode::parse(v["mode"].as_str()?)?,
        })
    }
}

/// `$XDG_RUNTIME_DIR/wkey/cursor.json`, else `~/.cache/wkey/cursor.json`.
pub fn cursor_path(runtime: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    let dir = match (runtime, home) {
        (Some(r), _) if !r.is_empty() => PathBuf::from(r),
        (_, Some(h)) if !h.is_empty() => Path::new(h).join(".cache"),
        _ => return None,
    };
    Some(dir.join("wkey").join("cursor.json"))
}

/// The cursor file for this user.
pub fn default_cursor_path() -> Option<PathBuf> {
    let var = |k| std::env::var(k).ok();
    cursor_path(var("XDG_RUNTIME_DIR").as_deref(), var("HOME").as_deref())
}

/// Writes through a temporary file, so a reader never sees half a file.
pub fn publish(path: &Path, cursor: &Cursor) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}", std::process::id()));
    std::fs::write(&tmp, cursor.to_json().to_string())?;
    std::fs::rename(tmp, path)
}

/// Reads one `Content-Length` framed message; `None` at a clean end of input.
pub fn read(r: &mut impl BufRead) -> Result<Option<Value>> {
    let mut len = None;
    let mut line = String::new();
    let mut started = false;
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            if started {
                bail!("input ended inside a header");
            }
            return Ok(None);
        }
        started = true;
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            len = Some(v.trim().parse::<usize>().context("bad Content-Length")?);
        }
    }
    let mut body = vec![0; len.context("header without Content-Length")?];
    r.read_exact(&mut body)?;
    Ok(Some(
        serde_json::from_slice(&body).context("body is not JSON")?,
    ))
}

pub fn write(w: &mut impl Write, m: &Value) -> Result<()> {
    let body = serde_json::to_vec(m)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()?;
    Ok(())
}

type Pos = (u64, u64);

/// Follows the client's messages and infers the vim mode.
#[derive(Debug, Default)]
pub struct Tracker {
    pub cursor: Option<Cursor>,
    mode: Mode,
    /// Where the cursor lands if the last edit was typed in insert mode.
    expected: Option<Pos>,
}

pub enum Step {
    Reply(Value),
    Moved,
    Exit,
    Nothing,
}

impl Tracker {
    pub fn on_message(&mut self, m: &Value) -> Vec<Step> {
        let Some(method) = m["method"].as_str() else {
            return vec![Step::Nothing];
        };
        let id = m.get("id").cloned();
        let reply = |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result});
        let p = &m["params"];
        match (method, &id) {
            ("initialize", Some(_)) => vec![Step::Reply(reply(json!({
                "capabilities": {
                    "textDocumentSync": {"openClose": true, "change": 2},
                    "codeActionProvider": true,
                },
                "serverInfo": {"name": "wkey"},
            })))],
            ("shutdown", Some(_)) => vec![Step::Reply(reply(Value::Null))],
            ("exit", None) => vec![Step::Exit],
            ("textDocument/codeAction", Some(_)) => {
                let mut steps = vec![Step::Reply(reply(json!([])))];
                // Requests with `only` come from saving, not from moving.
                if p["context"].get("only").is_none() && self.moved(p).is_some() {
                    steps.push(Step::Moved);
                }
                steps
            }
            ("textDocument/didChange", None) => {
                for change in p["contentChanges"].as_array().into_iter().flatten() {
                    self.edited(change);
                }
                vec![Step::Nothing]
            }
            (_, Some(_)) => vec![Step::Reply(json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": format!("wkey does not answer {method}")},
            }))],
            (_, None) => vec![Step::Nothing],
        }
    }

    // ponytail: a guess from edits and selections. Escape out of insert mode
    // sends nothing, so insert lasts until the next motion or edit.
    fn edited(&mut self, change: &Value) -> Option<()> {
        let start = pos(&change["range"]["start"])?;
        let end = pos(&change["range"]["end"])?;
        let text = change["text"].as_str()?;
        let short = text.encode_utf16().count() <= 2 && !text.contains('\n');
        let new_line = text.starts_with('\n') && text.trim().is_empty();
        let typed = start == end && (short || new_line);
        // `x` deletes the character at the cursor, backspace the one before it.
        let backspace = text.is_empty() && self.mode == Mode::Insert && self.expected == Some(end);
        if typed {
            self.mode = Mode::Insert;
        }
        self.expected = match text.rsplit_once('\n') {
            _ if !typed && !backspace => None,
            Some((before, after)) => Some((
                start.0 + before.matches('\n').count() as u64 + 1,
                after.encode_utf16().count() as u64,
            )),
            None => Some((start.0, start.1 + text.encode_utf16().count() as u64)),
        };
        Some(())
    }

    fn moved(&mut self, p: &Value) -> Option<()> {
        let uri = p["textDocument"]["uri"].as_str()?;
        let start = pos(&p["range"]["start"])?;
        let end = pos(&p["range"]["end"])?;
        self.mode = if start != end {
            Mode::Visual
        } else if self.mode == Mode::Insert && self.expected == Some(start) {
            Mode::Insert
        } else {
            Mode::Normal
        };
        if self.expected != Some(start) {
            self.expected = None;
        }
        self.cursor = Some(Cursor {
            uri: uri.into(),
            line: start.0,
            character: start.1,
            mode: self.mode,
        });
        Some(())
    }
}

fn pos(v: &Value) -> Option<Pos> {
    Some((v["line"].as_u64()?, v["character"].as_u64()?))
}

/// Runs the server on stdio until the client says `exit` or closes the pipe.
pub fn serve() -> Result<()> {
    let path = default_cursor_path();
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut tracker = Tracker::default();
    while let Some(m) = read(&mut input)? {
        let steps = tracker.on_message(&m);
        // Publish before replying, so whoever sees the reply sees the cursor.
        if steps.iter().any(|s| matches!(s, Step::Moved))
            && let (Some(path), Some(cursor)) = (&path, &tracker.cursor)
            && let Err(e) = publish(path, cursor)
        {
            eprintln!("wkey: cannot write {}: {e}", path.display());
        }
        for step in steps {
            match step {
                Step::Reply(r) => write(&mut output, &r)?,
                Step::Exit => return Ok(()),
                Step::Moved | Step::Nothing => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(m: &Value) -> Vec<u8> {
        let mut out = vec![];
        write(&mut out, m).unwrap();
        out
    }

    #[test]
    fn frames_round_trip_and_count_bytes() {
        let a = json!({"id": 1, "s": "⊢"});
        let b = json!({"method": "exit"});
        let mut bytes = framed(&a);
        let body = serde_json::to_vec(&a).unwrap();
        assert!(bytes.starts_with(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes()));
        bytes.extend(framed(&b));
        let mut r = bytes.as_slice();
        assert_eq!(read(&mut r).unwrap(), Some(a));
        assert_eq!(read(&mut r).unwrap(), Some(b));
        assert_eq!(read(&mut r).unwrap(), None);
        assert!(read(&mut &b"Content-Length: 2\r\n"[..]).is_err());
        assert!(read(&mut &b"X: 1\r\n\r\n{}"[..]).is_err());
    }

    #[test]
    fn cursor_file_lives_in_the_runtime_dir_or_cache() {
        assert_eq!(
            cursor_path(Some("/run/u"), Some("/home/u")),
            Some(PathBuf::from("/run/u/wkey/cursor.json"))
        );
        assert_eq!(
            cursor_path(Some(""), Some("/home/u")),
            Some(PathBuf::from("/home/u/.cache/wkey/cursor.json"))
        );
        assert_eq!(cursor_path(None, None), None);
    }

    #[test]
    fn cursor_survives_json_and_knows_its_extension() {
        let c = Cursor {
            uri: "file:///p/notes.md".into(),
            line: 3,
            character: 7,
            mode: Mode::Visual,
        };
        assert_eq!(Cursor::from_json(&c.to_json()), Some(c.clone()));
        assert_eq!(c.extension().as_deref(), Some("md"));
        let none = Cursor {
            uri: "file:///p/Makefile".into(),
            ..c
        };
        assert_eq!(none.extension(), None);
        assert_eq!(Cursor::from_json(&json!({"uri": 1})), None);
    }

    #[test]
    fn publish_replaces_the_file() {
        let dir = std::env::temp_dir().join(format!("wkey-publish-{}", std::process::id()));
        let path = dir.join("cursor.json");
        let c = Cursor {
            uri: "file:///a.rs".into(),
            ..Cursor::default()
        };
        publish(&path, &c).unwrap();
        publish(&path, &Cursor { line: 9, ..c }).unwrap();
        let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(Cursor::from_json(&v).unwrap().line, 9);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn request(id: u64, method: &str, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    }

    fn at(t: &mut Tracker, from: Pos, to: Pos) -> Mode {
        let range = json!({"start": {"line": from.0, "character": from.1}, "end": {"line": to.0, "character": to.1}});
        let params = json!({"textDocument": {"uri": "file:///a.rs"}, "range": range, "context": {"diagnostics": []}});
        let steps = t.on_message(&request(7, "textDocument/codeAction", params));
        assert!(
            matches!(&steps[..], [Step::Reply(r), Step::Moved] if r["result"] == json!([]) && r["id"] == 7)
        );
        t.cursor.as_ref().unwrap().mode
    }

    fn edit(t: &mut Tracker, from: Pos, to: Pos, text: &str) {
        let range = json!({"start": {"line": from.0, "character": from.1}, "end": {"line": to.0, "character": to.1}});
        let m = json!({"method": "textDocument/didChange", "params": {
            "textDocument": {"uri": "file:///a.rs", "version": 2},
            "contentChanges": [{"range": range, "text": text}]}});
        assert!(matches!(&t.on_message(&m)[..], [Step::Nothing]));
    }

    #[test]
    fn initialize_offers_code_actions_only() {
        let mut t = Tracker::default();
        let steps = t.on_message(&request(1, "initialize", json!({})));
        let [Step::Reply(r)] = &steps[..] else {
            panic!()
        };
        let caps = &r["result"]["capabilities"];
        assert_eq!(caps["codeActionProvider"], true);
        assert!(caps.get("documentHighlightProvider").is_none());
        assert_eq!(caps["textDocumentSync"]["change"], 2);
        assert!(
            matches!(&t.on_message(&request(2, "shutdown", Value::Null))[..], [Step::Reply(r)] if r["result"].is_null())
        );
        assert!(matches!(
            &t.on_message(&json!({"method": "exit"}))[..],
            [Step::Exit]
        ));
        let unknown = t.on_message(&request(3, "textDocument/hover", json!({})));
        assert!(matches!(&unknown[..], [Step::Reply(r)] if r["error"]["code"] == -32601));
        assert!(matches!(
            &t.on_message(&json!({"method": "initialized"}))[..],
            [Step::Nothing]
        ));
    }

    #[test]
    fn selections_and_typing_tell_the_mode() {
        let mut t = Tracker::default();
        assert_eq!(at(&mut t, (1, 4), (1, 4)), Mode::Normal);
        assert_eq!(t.cursor.as_ref().unwrap().character, 4);
        assert_eq!(at(&mut t, (1, 4), (1, 9)), Mode::Visual);
        assert_eq!(at(&mut t, (1, 9), (1, 9)), Mode::Normal);
        // `i` then typing "ab": the cursor stays right after the typed text.
        edit(&mut t, (1, 9), (1, 9), "a");
        edit(&mut t, (1, 10), (1, 10), "b");
        assert_eq!(at(&mut t, (1, 11), (1, 11)), Mode::Insert);
        assert_eq!(at(&mut t, (1, 11), (1, 11)), Mode::Insert);
        // A motion away from the typed text is normal mode.
        assert_eq!(at(&mut t, (1, 10), (1, 10)), Mode::Normal);
        // `o` opens an indented line and enters insert mode.
        edit(&mut t, (1, 20), (1, 20), "\n    ");
        assert_eq!(at(&mut t, (2, 4), (2, 4)), Mode::Insert);
        assert_eq!(at(&mut t, (2, 3), (2, 3)), Mode::Normal);
        // Pasting, deleting and replacing are normal mode edits.
        edit(&mut t, (3, 0), (3, 0), "pasted line\n");
        assert_eq!(at(&mut t, (4, 0), (4, 0)), Mode::Normal);
        edit(&mut t, (3, 0), (3, 1), "");
        assert_eq!(at(&mut t, (3, 0), (3, 0)), Mode::Normal);
    }

    #[test]
    fn deleting_after_typing_is_normal_mode() {
        // Zed 1.28.1, captured: `i z`, Escape (no message at all), then `x`.
        let mut t = Tracker::default();
        edit(&mut t, (3, 12), (3, 12), "z");
        assert_eq!(at(&mut t, (3, 13), (3, 13)), Mode::Insert);
        // Backspace removes the character before the cursor and stays in insert.
        edit(&mut t, (3, 12), (3, 13), "");
        assert_eq!(at(&mut t, (3, 12), (3, 12)), Mode::Insert);
        edit(&mut t, (3, 12), (3, 13), "");
        assert_eq!(at(&mut t, (3, 12), (3, 12)), Mode::Normal);
    }
}
