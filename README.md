# wkey

Which-key for Zed's vim mode. A terminal panel beside the editor lists every
key you can press next in the vim state at your cursor, in folding sections,
flowed into columns and colored by kind: prefixes, operators, motions, text
objects, mode switches, scrolling, edits, editor and workspace actions.

```
 OPERATOR  d   main.rs:12:5  following Zed
▾ Operators (11)
' → jump to mark line   a → around object       i → inner object        T → till char backward
@ → replay macro        F → find char backward  m → set mark            t → till char
▾ Motions (48)
    $ → end of line                         G → end of document
    ...
d i w → word  vim::Word  [vim_operator == a || vim_operator == i || ...]
```

It reads the same keymaps Zed does (Zed's defaults and vim keymap, then your
`~/.config/zed/keymap.json`) and picks the winning binding with Zed's own
rules: the deepest matching context wins, then the later binding, and `null`
disables a key.

## Install

```sh
cargo install --path crates/wkey
```

Then in Zed run `zed: install dev extension` and pick this directory. The
extension starts `wkey lsp` next to each language's own server (see
`extension.toml` for the languages). It answers nothing; it only watches the
cursor. To use a binary that is not on `PATH`, set both its path and its
argument, since Zed then runs it as is:

```json
"lsp": { "wkey": { "binary": { "path": "/path/to/wkey", "arguments": ["lsp"] } } }
```

Open the view from a Zed terminal with `wkey`. Extensions cannot ship global
tasks or keybindings, so to open it with one key instead, add a task to
`~/.config/zed/tasks.json`:

```json
[
  {
    "label": "wkey",
    "command": "wkey",
    "use_new_terminal": false,
    "allow_concurrent_runs": true,
    "reveal": "no_focus",
    "hide": "on_success",
    "show_summary": false,
    "show_command": false
  }
]
```

and a binding to `~/.config/zed/keymap.json`:

```json
{ "context": "Workspace", "bindings": { "alt-shift-k": ["task::Spawn", { "task_name": "wkey" }] } }
```

The key opens the view without taking focus from the editor; pressing it
again restarts the view in the same terminal, and `C-c` in the view closes
it. `"terminal": { "dock": "right" }` puts it beside the code.

## Use

Keep typing in the editor and the view follows: the file, the cursor and the
vim mode. Focus the terminal to explore: keys go to a simulated vim, so `g`
opens the `g` group, `d` shows what can follow the delete operator, and a
finished command such as `d i w` shows its action and the context that
enabled it at the bottom.

| Key | Does |
| --- | --- |
| `Tab`, `S-Tab` | pick the mode by hand (normal, insert, visual, replace) |
| `F2` | follow Zed's mode again |
| `Up`, `Down`, `Enter` | select a section, fold or unfold it |
| `Backspace` | undo the last pending key or operator |
| `C-c` | quit |

Everything else, including `Esc`, goes to the simulated vim.

## How the mode is known

Zed tells extensions nothing about vim, so `wkey lsp` guesses from what every
language server sees: Zed asks each server for code actions at the selection
and sends it every edit. A non-empty selection means visual mode; a cursor
right after text it just saw typed means insert mode; anything else means
normal mode. The guess goes to `$XDG_RUNTIME_DIR/wkey/cursor.json` (or
`~/.cache/wkey/cursor.json`), which the view rereads.

Known gaps: Escape out of insert mode sends nothing (Zed 1.28.1 leaves the
cursor where it is), so the view says insert until the next motion or edit;
opening a line above with `O` reads as normal mode; replace mode is never
detected. Pick the mode with `Tab` when the guess is wrong. Only the
contexts of a plain focused editor are modeled, so bindings for menus,
completions and panels do not show. `base_keymap` other than the default is
not supported.

## Development

```sh
cargo test --workspace
```

The bundled keymaps in `crates/wkey/keymaps` are Zed v1.28.1's
`assets/keymaps`, which are GPL-3.0-or-later, and so is wkey.
