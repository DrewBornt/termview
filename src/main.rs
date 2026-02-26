use std::io::{self, stdout, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use clap::Parser;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{self, SetBackgroundColor, SetForegroundColor},
    terminal::{self, disable_raw_mode, enable_raw_mode, ClearType},
};
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView};

/// A terminal-based image viewer using the Kitty graphics protocol.
/// Displays native pixels — works in foot, kitty, WezTerm, and Windows Terminal.
#[derive(Parser, Debug)]
#[command(name = "termview", version, about)]
struct Args {
    /// Image file to open (defaults to first image in current directory)
    #[arg()]
    file: Option<PathBuf>,

    /// Directory to browse images from
    #[arg(short, long, default_value = ".")]
    directory: PathBuf,
}

const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "tiff", "tif", "webp", "ico", "pnm", "pbm", "pgm",
    "ppm", "qoi", "tga",
];

fn is_image_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| IMAGE_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn collect_images(dir: &Path) -> Vec<PathBuf> {
    let mut images: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_image_file(p))
        .collect();

    images.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .cmp(&b.file_name().unwrap_or_default().to_ascii_lowercase())
    });

    images
}

// ---------------------------------------------------------------------------
// Kitty graphics protocol
// ---------------------------------------------------------------------------

/// Delete all kitty graphics placements from the screen.
fn kitty_clear(out: &mut impl Write) -> io::Result<()> {
    // a=d (delete), d=A (all placements)
    write!(out, "\x1b_Ga=d,d=A\x1b\\")?;
    Ok(())
}

/// Display an image using the Kitty graphics protocol.
///
/// The image is transmitted as raw RGBA pixels, chunked into 4096-byte base64
/// payloads. It is placed at the current cursor position and scaled to fit
/// within `cols` x `rows` terminal cells.
fn kitty_display(
    out: &mut impl Write,
    img: &DynamicImage,
    cols: u16,
    rows: u16,
    cell_width_px: u16,
    cell_height_px: u16,
) -> io::Result<()> {
    let avail_px_w = cols as u32 * cell_width_px as u32;
    let avail_px_h = rows as u32 * cell_height_px as u32;

    let (img_w, img_h) = img.dimensions();

    // Scale to fit while preserving aspect ratio
    let scale_x = avail_px_w as f64 / img_w as f64;
    let scale_y = avail_px_h as f64 / img_h as f64;
    let scale = scale_x.min(scale_y).min(1.0); // don't upscale

    let disp_w = ((img_w as f64 * scale) as u32).max(1);
    let disp_h = ((img_h as f64 * scale) as u32).max(1);

    let resized = if disp_w != img_w || disp_h != img_h {
        img.resize_exact(disp_w, disp_h, FilterType::Lanczos3)
    } else {
        img.clone()
    };

    let rgba = resized.to_rgba8();
    let raw_pixels = rgba.as_raw();

    // Center the image: compute the column/row offset
    let img_cols = (disp_w + cell_width_px as u32 - 1) / cell_width_px as u32;
    let img_rows = (disp_h + cell_height_px as u32 - 1) / cell_height_px as u32;
    let col_offset = (cols as u32).saturating_sub(img_cols) / 2;
    let row_offset = (rows as u32).saturating_sub(img_rows) / 2;

    // Move cursor to centering position
    queue!(out, cursor::MoveTo(col_offset as u16, row_offset as u16))?;

    // Encode as base64 and send in chunks
    let b64 = base64::engine::general_purpose::STANDARD.encode(raw_pixels);
    let chunks: Vec<&str> = b64.as_bytes().chunks(4096).map(|c| {
        std::str::from_utf8(c).unwrap()
    }).collect();

    for (i, chunk) in chunks.iter().enumerate() {
        let is_first = i == 0;
        let is_last = i == chunks.len() - 1;
        let more = if is_last { 0 } else { 1 };

        if is_first {
            // a=T (transmit and display), f=32 (RGBA), s=width, v=height
            write!(
                out,
                "\x1b_Ga=T,f=32,s={},v={},m={};{}\x1b\\",
                disp_w, disp_h, more, chunk
            )?;
        } else {
            write!(out, "\x1b_Gm={};{}\x1b\\", more, chunk)?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Terminal cell size detection
// ---------------------------------------------------------------------------

/// Try to detect the pixel dimensions of a terminal cell.
/// Uses the TIOCGWINSZ ioctl on Linux to get pixel size.
/// Falls back to reasonable defaults if unavailable.
fn get_cell_size() -> (u16, u16) {
    #[cfg(unix)]
    {
        use std::mem::MaybeUninit;

        #[repr(C)]
        struct Winsize {
            ws_row: u16,
            ws_col: u16,
            ws_xpixel: u16,
            ws_ypixel: u16,
        }

        unsafe {
            let mut ws = MaybeUninit::<Winsize>::uninit();
            // TIOCGWINSZ = 0x5413 on Linux
            let ret = libc::ioctl(1, 0x5413, ws.as_mut_ptr());
            if ret == 0 {
                let ws = ws.assume_init();
                if ws.ws_xpixel > 0 && ws.ws_ypixel > 0 && ws.ws_col > 0 && ws.ws_row > 0 {
                    let cw = ws.ws_xpixel / ws.ws_col;
                    let ch = ws.ws_ypixel / ws.ws_row;
                    if cw > 0 && ch > 0 {
                        return (cw, ch);
                    }
                }
            }
        }
    }

    // Fallback: assume ~8x16 px cells (common for most fonts)
    (8, 16)
}

// ---------------------------------------------------------------------------
// Status bar drawing (manual, no ratatui needed)
// ---------------------------------------------------------------------------

fn draw_status_bar(
    out: &mut impl Write,
    row: u16,
    cols: u16,
    left: &str,
    right: &str,
) -> io::Result<()> {
    queue!(out, cursor::MoveTo(0, row))?;
    queue!(
        out,
        SetForegroundColor(style::Color::White),
        SetBackgroundColor(style::Color::DarkGrey),
    )?;

    let left_len = left.len().min(cols as usize);
    let right_len = right.len().min(cols as usize);
    let pad = (cols as usize).saturating_sub(left_len + right_len);

    write!(out, "{}", &left[..left_len])?;
    write!(out, "{}", " ".repeat(pad))?;
    write!(out, "{}", &right[..right_len])?;

    queue!(
        out,
        SetForegroundColor(style::Color::Reset),
        SetBackgroundColor(style::Color::Reset),
    )?;

    Ok(())
}

fn draw_help_overlay(out: &mut impl Write, cols: u16, rows: u16) -> io::Result<()> {
    let help_lines = [
        "",
        "  termview — Keyboard Shortcuts",
        "",
        "  ← / h       Previous image",
        "  → / l       Next image",
        "  Home / g    First image",
        "  End / G     Last image",
        "  + / =       Zoom in",
        "  - / _       Zoom out",
        "  0           Reset zoom",
        "  w/a/s/d     Pan (when zoomed)",
        "  ?           Toggle help",
        "  q / Esc     Quit",
        "",
    ];

    let box_w: u16 = 40;
    let box_h = help_lines.len() as u16 + 2; // +2 for top/bottom border
    let start_col = cols.saturating_sub(box_w) / 2;
    let start_row = rows.saturating_sub(box_h) / 2;

    queue!(
        out,
        SetForegroundColor(style::Color::White),
        SetBackgroundColor(style::Color::Black),
    )?;

    // Top border
    queue!(out, cursor::MoveTo(start_col, start_row))?;
    write!(out, "┌{}┐", "─".repeat((box_w - 2) as usize))?;

    // Content lines
    for (i, line) in help_lines.iter().enumerate() {
        let r = start_row + 1 + i as u16;
        queue!(out, cursor::MoveTo(start_col, r))?;
        let content_w = (box_w - 2) as usize;
        let padded = format!("{:<width$}", line, width = content_w);
        // Truncate if needed
        let display: String = padded.chars().take(content_w).collect();
        write!(out, "│{}│", display)?;
    }

    // Bottom border
    queue!(out, cursor::MoveTo(start_col, start_row + box_h - 1))?;
    write!(out, "└{}┘", "─".repeat((box_w - 2) as usize))?;

    queue!(
        out,
        SetForegroundColor(style::Color::Reset),
        SetBackgroundColor(style::Color::Reset),
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

struct App {
    images: Vec<PathBuf>,
    index: usize,
    current_image: Option<DynamicImage>,
    error_message: Option<String>,
    show_help: bool,
    zoom: f64,
    pan_x: f64,
    pan_y: f64,
}

impl App {
    fn new(images: Vec<PathBuf>, start_index: usize) -> Self {
        let mut app = App {
            images,
            index: start_index,
            current_image: None,
            error_message: None,
            show_help: false,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
        };
        app.load_current();
        app
    }

    fn load_current(&mut self) {
        self.error_message = None;
        self.zoom = 1.0;
        self.pan_x = 0.0;
        self.pan_y = 0.0;

        if self.images.is_empty() {
            self.current_image = None;
            self.error_message = Some("No images found in directory".into());
            return;
        }

        let path = &self.images[self.index];
        match image::open(path) {
            Ok(img) => self.current_image = Some(img),
            Err(e) => {
                self.current_image = None;
                self.error_message = Some(format!("Failed to load {}: {}", path.display(), e));
            }
        }
    }

    fn next(&mut self) {
        if !self.images.is_empty() {
            self.index = (self.index + 1) % self.images.len();
            self.load_current();
        }
    }

    fn prev(&mut self) {
        if !self.images.is_empty() {
            self.index = if self.index == 0 {
                self.images.len() - 1
            } else {
                self.index - 1
            };
            self.load_current();
        }
    }

    fn first(&mut self) {
        if !self.images.is_empty() {
            self.index = 0;
            self.load_current();
        }
    }

    fn last(&mut self) {
        if !self.images.is_empty() {
            self.index = self.images.len() - 1;
            self.load_current();
        }
    }

    fn zoom_in(&mut self) {
        self.zoom = (self.zoom * 1.25).min(10.0);
    }

    fn zoom_out(&mut self) {
        self.zoom = (self.zoom / 1.25).max(0.1);
    }

    fn zoom_reset(&mut self) {
        self.zoom = 1.0;
        self.pan_x = 0.0;
        self.pan_y = 0.0;
    }

    fn pan(&mut self, dx: f64, dy: f64) {
        self.pan_x += dx;
        self.pan_y += dy;
    }

    fn current_filename(&self) -> String {
        if self.images.is_empty() {
            return "(none)".into();
        }
        self.images[self.index]
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into()
    }

    fn image_info(&self) -> String {
        if let Some(ref img) = self.current_image {
            let (w, h) = img.dimensions();
            format!("{}x{}", w, h)
        } else {
            String::new()
        }
    }

    /// Get the image view, applying zoom and pan via cropping.
    fn get_view_image(&self) -> Option<DynamicImage> {
        let img = self.current_image.as_ref()?;

        if (self.zoom - 1.0).abs() < 0.01 && self.pan_x.abs() < 0.01 && self.pan_y.abs() < 0.01 {
            return Some(img.clone());
        }

        let (w, h) = img.dimensions();
        let view_w = ((w as f64 / self.zoom) as u32).max(1);
        let view_h = ((h as f64 / self.zoom) as u32).max(1);

        let center_x = (w as f64 / 2.0 + self.pan_x * w as f64).clamp(0.0, w as f64);
        let center_y = (h as f64 / 2.0 + self.pan_y * h as f64).clamp(0.0, h as f64);

        let x = (center_x - view_w as f64 / 2.0)
            .max(0.0)
            .min((w.saturating_sub(view_w.min(w))) as f64) as u32;
        let y = (center_y - view_h as f64 / 2.0)
            .max(0.0)
            .min((h.saturating_sub(view_h.min(h))) as f64) as u32;

        let crop_w = view_w.min(w - x);
        let crop_h = view_h.min(h - y);

        if crop_w == 0 || crop_h == 0 {
            return Some(img.clone());
        }

        Some(img.crop_imm(x, y, crop_w, crop_h))
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

fn draw(out: &mut impl Write, app: &App) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    let (cell_w, cell_h) = get_cell_size();

    // Clear screen and delete old kitty images
    queue!(out, terminal::Clear(ClearType::All))?;
    kitty_clear(out)?;

    let image_rows = rows.saturating_sub(1); // reserve 1 row for status bar

    // Draw image
    if let Some(view_img) = app.get_view_image() {
        kitty_display(out, &view_img, cols, image_rows, cell_w, cell_h)?;
    } else if let Some(ref err) = app.error_message {
        let err_row = rows / 2;
        let err_col = cols.saturating_sub(err.len() as u16) / 2;
        queue!(
            out,
            cursor::MoveTo(err_col, err_row),
            SetForegroundColor(style::Color::Red),
        )?;
        write!(out, "{}", err)?;
        queue!(out, SetForegroundColor(style::Color::Reset))?;
    }

    // Status bar
    let filename = app.current_filename();
    let counter = if app.images.is_empty() {
        "0/0".into()
    } else {
        format!("{}/{}", app.index + 1, app.images.len())
    };
    let info = app.image_info();
    let zoom_str = if (app.zoom - 1.0).abs() > 0.01 {
        format!(" {:.0}%", app.zoom * 100.0)
    } else {
        String::new()
    };

    let left = format!(" {} {} {}", filename, info, zoom_str);
    let right = format!("{} | q:quit ?:help ", counter);

    draw_status_bar(out, rows - 1, cols, &left, &right)?;

    // Help overlay
    if app.show_help {
        draw_help_overlay(out, cols, rows)?;
    }

    // Hide cursor
    queue!(out, cursor::Hide)?;

    out.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let browse_dir = if let Some(ref file) = args.file {
        if file.is_dir() {
            file.clone()
        } else {
            file.parent().unwrap_or(Path::new(".")).to_path_buf()
        }
    } else {
        args.directory.clone()
    };

    let browse_dir = std::fs::canonicalize(&browse_dir).unwrap_or(browse_dir);
    let images = collect_images(&browse_dir);

    let start_index = if let Some(ref file) = args.file {
        if file.is_file() {
            let canonical = std::fs::canonicalize(file).unwrap_or(file.clone());
            images
                .iter()
                .position(|p| std::fs::canonicalize(p).unwrap_or(p.clone()) == canonical)
                .unwrap_or(0)
        } else {
            0
        }
    } else {
        0
    };

    // Setup terminal
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(
        out,
        terminal::EnterAlternateScreen,
        cursor::Hide,
    )?;

    let mut app = App::new(images, start_index);

    // Initial draw
    draw(&mut out, &app)?;

    // Event loop
    loop {
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }

                    let mut needs_redraw = true;

                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('c')
                            if key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            break
                        }

                        KeyCode::Right | KeyCode::Char('l') => app.next(),
                        KeyCode::Left | KeyCode::Char('h') => app.prev(),
                        KeyCode::Home | KeyCode::Char('g') => app.first(),
                        KeyCode::End => app.last(),
                        KeyCode::Char('G') => app.last(),

                        KeyCode::Char('+') | KeyCode::Char('=') => app.zoom_in(),
                        KeyCode::Char('-') | KeyCode::Char('_') => app.zoom_out(),
                        KeyCode::Char('0') => app.zoom_reset(),

                        KeyCode::Char('w') => app.pan(0.0, -0.05),
                        KeyCode::Char('s') => app.pan(0.0, 0.05),
                        KeyCode::Char('a') => app.pan(-0.05, 0.0),
                        KeyCode::Char('d') => app.pan(0.05, 0.0),

                        KeyCode::Char('?') => app.show_help = !app.show_help,

                        _ => needs_redraw = false,
                    }

                    if needs_redraw {
                        draw(&mut out, &app)?;
                    }
                }
                Event::Resize(_, _) => {
                    draw(&mut out, &app)?;
                }
                _ => {}
            }
        }
    }

    // Cleanup: delete kitty images, restore terminal
    kitty_clear(&mut out)?;
    execute!(
        out,
        cursor::Show,
        terminal::LeaveAlternateScreen,
    )?;
    disable_raw_mode()?;

    Ok(())
}