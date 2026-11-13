# wkey

Which-key for Zed's vim mode. A terminal panel beside the editor lists every
key you can press next in the vim state at your cursor, in folding sections,
flowed into columns and colored by kind: operators, motions, text objects,
edits, mode switches, prefixes, editor actions, scrolling (folded at start)
and workspace actions. On macOS modifiers read as the menus draw them, so
`alt-cmd-g` is `⌥⌘G`.

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
again restarts the view in the same terminal, and `ctrl-c` in the view closes
it. `"terminal": { "dock": "right" }` puts it beside the code.

## Use

Keep typing in the editor and the view follows: the file, the cursor, the
vim mode and the keys typed so far, so after `d` it lists only what can
follow `d`. Focus the terminal to explore: keys go to a simulated vim, so `g`
opens the `g` group, `d` shows what can follow the delete operator, and a
finished command such as `d i w` shows its action and the context that
enabled it at the bottom.

| Key | Does |
| --- | --- |
| `tab`, `shift-tab` | pick the mode by hand (normal, insert, visual, replace) |
| `f2` | follow Zed's mode again |
| `up`, `down`, `enter` | select a section, fold or unfold it |
| `j`, `k` | scroll through the selected section, unless a prefix or operator waits |
| `backspace` | undo the last pending key or operator |
| `ctrl-c` | quit |

Everything else, including `esc`, goes to the simulated vim.

## How wkey knows where you are

Zed tells extensions nothing about vim. On macOS wkey watches the keys you
type into Zed and plays them through the same keymaps. The watch only
listens: it never changes a key, and it skips keys meant for other apps or
for the wkey panel itself. The first run asks macOS to let Zed watch the
keyboard; allow Zed under System Settings > Privacy & Security > Input
Monitoring and restart Zed.

`wkey lsp` adds what every language server sees: Zed asks it for code
actions at the selection and sends it every edit. That gives the file and
the cursor, and a selection means visual mode (the mouse makes those too).
It writes them to `$XDG_RUNTIME_DIR/wkey/cursor.json` (or
`~/.cache/wkey/cursor.json`), which the view rereads. Without the key watch
(on Linux, or before Zed is allowed) the mode is a guess from edits: a
cursor right after text just typed means insert mode, anything else normal
mode. Escape out of insert mode then goes unseen, `O` reads as normal mode
and replace mode is never detected.

Known gaps: keys typed into Zed's command palette, file finder or panels
also reach the simulated vim, since the watch cannot tell them from the
editor; `esc` puts it right. Keys held with ctrl, alt or cmd are read on a
US layout. Pick the mode with `tab` when it is wrong. Only the contexts of a
plain focused editor are modeled, so bindings for menus, completions and
panels do not show. `base_keymap` other than the default is not supported.

## Development

```sh
cargo test --workspace
```

The bundled keymaps in `crates/wkey/keymaps` are Zed v1.28.1's
`assets/keymaps`, which are GPL-3.0-or-later, and so is wkey.
