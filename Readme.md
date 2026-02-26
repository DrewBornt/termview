# termview

A terminal-based image viewer written in Rust. Renders images using Unicode half-block characters (▀), so it works everywhere — including over SSH sessions with no GUI.

## Features

- **Universal rendering** — Uses Unicode half-blocks with 24-bit color, works in any modern terminal
- **SSH-friendly** — No GUI, X11, or Wayland required
- **Browse images** — Arrow through all images in a directory
- **Zoom & pan** — Zoom into details with keyboard controls
- **Aspect ratio preservation** — Images are centered and scaled to fit
- **Wide format support** — PNG, JPEG, GIF, BMP, TIFF, WebP, QOI, TGA, ICO, PNM

## Installation

```bash
cargo install --path .
```

Or build manually:

```bash
cargo build --release
# Binary is at target/release/termview
```

## Usage

```bash
# Open a specific image (browse siblings with arrow keys)
termview photo.jpg

# Browse all images in current directory
termview

# Browse images in a specific directory
termview -d ~/Pictures
```

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `←` / `h` | Previous image |
| `→` / `l` | Next image |
| `Home` / `g` | First image |
| `End` / `G` | Last image |
| `+` / `=` | Zoom in |
| `-` / `_` | Zoom out |
| `0` | Reset zoom & pan |
| `w/a/s/d` | Pan (when zoomed) |
| `?` | Toggle help overlay |
| `q` / `Esc` | Quit |

## How It Works

Each terminal cell is treated as a 1×2 pixel block using the upper-half-block character `▀`. The foreground color represents the top pixel and the background color represents the bottom pixel. This effectively doubles the vertical resolution compared to using full block characters, giving a surprisingly decent image preview.

The viewer uses `ratatui` for the TUI framework and the `image` crate for decoding. Images are resized to fit the terminal dimensions while maintaining aspect ratio, using triangle (bilinear) filtering for quality.

## Requirements

- A terminal with 24-bit (truecolor) support for best results
- UTF-8 support (virtually all modern terminals)
- Works great with: Kitty, Alacritty, foot, WezTerm, iTerm2, Windows Terminal, GNOME Terminal, etc.
- Degraded but functional in 256-color terminals

## License

MIT