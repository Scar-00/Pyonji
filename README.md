# Pyonji

A terminal emulator whose grid is drawn on the GPU, configured from Lua.

> **Status: not yet buildable from a fresh clone.** Four dependencies in
> `Cargo.toml` are `path` deps pointing outside the repository, at
> `../oss/gpui-component`. See [Building](#building).

## What it does

- **GPU-drawn grid.** One wgpu render pass per frame, with three pipelines:
  the window background, the terminal grid, and the pane dividers. Glyphs and
  cell backgrounds are both submitted as GPU primitives rather than composited
  on the CPU. The backend is whatever wgpu selects for the platform — it is not
  pinned to Vulkan.
- **Terminal emulation** via [vt100](https://github.com/doy/vt100-rust), with
  2000 lines of scrollback.
- **Tabs and split panes.** Up to 9 tabs, each holding a tree of splits
  (horizontal or vertical).
- **Colour.** A fixed 16-colour palette, the standard xterm 6×6×6 cube and
  grayscale ramp for 256-colour, and 24-bit truecolor passed straight through.
  The 16 base colours are not configurable.
- **Cursor shape** follows the terminal's own `DECSCUSR` sequence — bar, block
  or underline — so applications that set it are honoured.
- **IME preedit** and clipboard paste on <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd>.
- **SSH sessions** over libssh2, declared in the config.
- **Overlays** for the session list, a directory picker, and the release
  screen.
- **A status bar** that doubles as a prompt: tab chips, a session rename field,
  and a `>` Lua line with LSP completion.
- **Self-update** from GitHub releases, showing which build matches the
  machine it is running on.
- **Lua configuration**, reloaded from disk when it changes and written on
  first run if missing.
- **Bundled fonts** — nothing to install. Iosevka Term across nine weights,
  Iosevka, Noto Sans Mono CJK, and two Nerd Font files for icons and emoji.

There is no image protocol (sixel, iTerm inline images) and no session
multiplexer.

## Building

### Prerequisites

- Rust 1.85 or newer. The crate is edition 2024 and declares no `rust-version`.
- A checkout of [gpui-component](https://github.com/gpui-ce/gpui-component) as
  a **sibling directory**:

  ```
  <somewhere>/
    Pyonji/            <- this repository
    oss/
      gpui-component/  <- required
  ```

  `Cargo.toml` points four crates at `../oss/gpui-component/`. They are not
  vendored, so `cargo build` in a fresh clone fails on a missing path
  dependency until that checkout is in place.

`vendor/gpui-ce/` *is* in the repository. It is a patched copy of
gpui-ce — see [vendor/gpui-ce/README.md](vendor/gpui-ce/README.md) for why,
and drop it once the upstream fix it carries has been picked up.

### Build

```bash
cargo build --release
```

Run it in place, reading `./init.lua`:

```bash
cargo run --release
```

Build with the `install` feature for a machine-wide install. The config then
lives in the platform config directory (`%LOCALAPPDATA%\pyonji\init.lua` on
Windows) instead of the working directory:

```bash
cargo build --release --features install
```

### Passing a starting directory

```bash
cargo run --release -- ~/dev/project
```

The single optional positional argument is the directory the first session
opens in. Without it, `default_cwd` from the config is used, falling back to
the process working directory.

### Releases

Pushing a `v*` tag builds three binaries and attaches them to a GitHub
release, which is where the in-app updater looks:

| Target | Feature |
|---|---|
| `x86_64-unknown-linux-gnu` | — |
| `x86_64-pc-windows-msvc` | `install` |
| `aarch64-apple-darwin` | — |

Pull requests are built and linted with `clippy -D warnings`.

## Configuration

The config is a Lua file executed on load, not a table to return. It is
re-executed whenever it changes on disk.

By default it is `./init.lua`; with the `install` feature it is
`<config dir>/pyonji/init.lua`. A commented starter file is written on first
run.

```lua
py:config({
    font_family = "Iosevka",
    font_size = 24,
    line_height = 1.1,      -- a multiple of font_size
    fullscreen = false,
    default_cwd = nil,
    editor = "nvim",        -- typed into a session when the opener picks a file
    ssh_sessions = {
        { name = "server", user_name = "root", ip = "192.168.1.100" },
    },
})
```

`resources/default.lua` is meant to be the annotated reference for the `py`
object. Treat it as a draft: it lists several methods that are not implemented
(see below).

### Keybindings

Almost nothing is bound by default — the only global binding Pyonji installs
itself is <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd> for paste. Keys are
bound from Lua, which is also what makes them remappable:

```lua
py:bind('<ctrl-b> f', py.open_sessions())
py:bind('<ctrl-b> r', py.open_rename())
py:bind('<ctrl-b> 1', py.switch_tab(0))   -- first tab
py:bind('<ctrl-b> v', py.split('v'))
```

`py:bind` takes a binding string and a callback. A method called with `py:`
returns its value; called as `py.method` it returns the callback to bind.

Inside an overlay, <kbd>↑</kbd><kbd>↓</kbd> move, <kbd>Enter</kbd> confirms
and <kbd>Esc</kbd> closes — with one deliberate exception: on the release
screen <kbd>Esc</kbd> clears the version filter first, and closes on the second
press.

### Opening things

| Call | Opens |
|---|---|
| `py:open_sessions()` | session list |
| `py:open_opener()` | directory picker |
| `py:open_releases()` | release screen and updater |
| `py:open_rename()` | rename the focused session |
| `py:open_lua()` | `>` prompt in the status bar |

## Not implemented

The **command palette does not exist** in this build. `py:open_palette()` and
the `OpenPalette` action both reach an unimplemented branch in `Overlay::open`
and will panic; `resources/main.ts` is an unused prototype that is not
compiled into the binary. Remove those calls from your config.

`nucleo-matcher` is present in `Cargo.toml` but unused — there is no fuzzy
matching anywhere yet.

`resources/default.lua` annotates six methods that `config.rs` never registers,
so calling them from a config errors: `open_command`, `open_detached`, `quit`,
`toggle_fullscreen`, `toggle_decorations`, `toggle_status_bar`. It also
documents a `status_height` config field that nothing reads. The status bar's
`:` command mode exists in the type but is never entered, so the command prompt
is unreachable.

`py:bind` also accepts a `(modifiers, key, callback)` form, which
`default.lua` shows. That branch is a `todo!()` and panics; use the binding
string form.

## Tech stack

- [wgpu](https://wgpu.rs/) — GPU rasterisation
- [gpui-ce](https://github.com/gpui-ce/gpui-ce) — windowing, input, and the
  UI toolkit the overlays are built from (vendored, see above)
- [gpui-component](https://github.com/gpui-ce/gpui-component) — dialogs,
  inputs, buttons, scrollbars, and the theme
- [vt100](https://github.com/doy/vt100-rust) — terminal emulation
- [portable-pty](https://github.com/wezterm/wezterm) — local PTYs
- [ssh2](https://github.com/alexcrichton/ssh2-rs) — SSH
- [swash](https://github.com/BrianSharpe/swash) — font shaping and rasterising
- [mlua](https://github.com/mlua-rs/mlua) — Lua, vendored build
- [self_update](https://github.com/jaemk/self_update) — the updater
- [async-lsp](https://github.com/oxidecomputer/async-lsp) — completion in the
  Lua prompt

## License

No license file has been added to this repository yet.
