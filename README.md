 Pyonji

A GPU-accelerated terminal emulator written in Rust.

## Features

- GPU-accelerated rendering via wgpu (Vulkan backend)
- Multi-tab support (up to 9 tabs)
- Split panes inside tabs
- VT100 terminal emulation
- 256-color ANSI support with Catppuccin-inspired theme
- Multiple cursor styles (Bar, Block, Underline)

## Requirements

- Rust 1.85+ (Edition 2024)
- [Iosevka](https://typeof.net/Iosevka/) font installed
- Windows (currently uses `cmd.exe`)

## Installation

```bash
cargo build --release
```

## Usage

```bash
cargo run --release
```

### Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Ctrl+B` -> `1-9` | Switch to/create tab |
| `Ctrl+B` -> `N` | Next tab |
| `Ctrl+B` -> `P` | Previous tab |
| `Ctrl+B` -> `H` | Split current pane horizontally |
| `Ctrl+B` -> `V` | Split current pane vertically |
| `Ctrl+B` -> `W` | Focus next pane |
| Hold `Ctrl+B` + `Arrow Keys` | Resize the nearest split around the active pane |
| `Left Mouse Drag` on a divider | Resize that split |

## Tech Stack

- [wgpu](https://wgpu.rs/) - GPU rendering
- [winit](https://github.com/rust-windowing/winit) - Window management
- [vt100](https://github.com/doy/vt100-rust) - Terminal emulation
- [portable-pty](https://github.com/wez/wezterm/tree/main/pty) - PTY handling

## License

MIT
