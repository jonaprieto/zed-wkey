use zed_extension_api::{self as zed, LanguageServerId, Result, Worktree};

const MISSING: &str = "wkey is not on PATH: run `cargo install --path crates/wkey` in the zed-wkey \
                       repository, or set lsp.wkey.binary.path and arguments [\"lsp\"]";

/// `wkey lsp` from the worktree's PATH. A configured `lsp.wkey.binary` never
/// gets here: Zed runs it directly with its own `arguments`.
fn command(on_path: Option<String>) -> Result<zed::Command> {
    Ok(zed::Command {
        command: on_path.ok_or(MISSING)?,
        args: vec!["lsp".into()],
        env: vec![],
    })
}

struct Wkey;

impl zed::Extension for Wkey {
    fn new() -> Self {
        Wkey
    }

    fn language_server_command(
        &mut self,
        _id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<zed::Command> {
        command(worktree.which("wkey"))
    }
}

zed::register_extension!(Wkey);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_the_server_from_path() {
        let c = command(Some("/bin/wkey".into())).unwrap();
        assert_eq!(
            (c.command.as_str(), c.args),
            ("/bin/wkey", vec!["lsp".to_string()])
        );
        assert_eq!(command(None).unwrap_err(), MISSING);
    }
}
