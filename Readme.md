# termview

A terminal-based image viewer written in Rust. Renders images at native pixel resolution using the Kitty graphics protocol — works over SSH.

## Features

- **Native pixel rendering** — Uses the Kitty graphics protocol to display actual pixels, not Unicode approximations
- **SSH-friendly** — No GUI, X11 forwarding, or Wayland required
- **Browse images** — Arrow through all images in a directory with wraparound
- **Zoom & pan** — Inspect details with keyboard controls
- **Aspect ratio preservation** — Images are centered and scaled to fit (never upscaled)
- **Lanczos3 downscaling** — High quality resize filter
- **Wide format support** — PNG, JPEG, GIF, BMP, TIFF, WebP, QOI, TGA, ICO, PNM

## Compatible Terminals

The Kitty graphics protocol is supported by:

- **foot** (Wayland — great with Hyprland)
- **kitty**
- **WezTerm**
- **Windows Terminal** (Windows 11 — works for SSH sessions)
- **Ghostty**
- **Konsole** (recent versions)

## Installation

```bash
cargo install --path .
```

Or build manually:

```bash
cargo build --release
# Binary at target/release/termview
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

The Kitty graphics protocol sends base64-encoded RGBA pixel data to the terminal via escape sequences (`\033_G...\033\\`). The terminal renders these as actual pixels overlaid on the text grid. This gives you real image quality.

The image is resized to fit within the terminal's pixel dimensions (detected via `TIOCGWINSZ` ioctl) using Lanczos3 filtering, centered, and transmitted in 4096-byte chunks. Zoom/pan works by cropping the source image before transmission.

## License

MIT