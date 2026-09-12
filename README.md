# zjump

Fast fuzzy and marked jumps for Zellij sessions, tabs, and panes.

## Modes

- Search mode: fuzzy-search every live `(session, tab)` and jump with Enter.
- Marks/slots mode: keep an ordered list of favorite tabs or panes.

## Suggested keybinds

```kdl
bind "Alt w" {
    LaunchOrFocusPlugin "file:/path/to/zjump.wasm" {
        floating true
        move_to_focused_tab true
    };
}

bind "Alt m" {
    LaunchOrFocusPlugin "file:/path/to/zjump.wasm" {
        floating true
        move_to_focused_tab true
        mode "slots"
    };
}
```

## Search mode

- Type: filter sessions/tabs.
- `Enter`: jump to the selected tab.
- `Up`/`Down` or `Ctrl-p`/`Ctrl-n`: move selection.
- `Esc` or `Ctrl-c`: close.

## Marks/slots mode

Use `mode "marks"` or `mode "slots"`.

- `1`-`9`: jump directly to that saved slot.
- `a`: add the current focused tab/pane.
- `d`: delete the selected mark.
- `j`/`k`: move selection.
- `Ctrl-j`/`Ctrl-k`: move the selected mark down/up.
- `Enter`: jump to the selected mark.
- `Esc` or `Ctrl-c`: close.

Marks are stored in the Zellij plugin cache at `/cache/zjump-marks.tsv`.

## Build

```bash
nix build
# result/bin/zjump.wasm
```

Or with Cargo:

```bash
rustup target add wasm32-wasip1
cargo build --release --target wasm32-wasip1
# target/wasm32-wasip1/release/zjump.wasm
```
