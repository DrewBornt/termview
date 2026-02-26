use std::io::{self, stdout};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, Rgba};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Terminal,
};

/// A terminal-based image viewer that renders images using Unicode half-blocks.
/// Works over SSH, no GUI required.
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

/// Collect and sort all image files in a directory.
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

/// Render a DynamicImage into a vector of ratatui Lines using half-block characters.
/// Each terminal row encodes two pixel rows using the upper-half-block character (▀),
/// with the foreground color as the top pixel and background color as the bottom pixel.
fn render_image_to_lines(img: &DynamicImage, width: u16, height: u16) -> Vec<Line<'static>> {
    // Each terminal cell encodes 2 vertical pixels
    let pixel_rows = height as u32 * 2;
    let pixel_cols = width as u32;

    if pixel_cols == 0 || pixel_rows == 0 {
        return vec![];
    }

    // Determine scaling to fit the image within the terminal area while maintaining aspect ratio
    let (img_w, img_h) = img.dimensions();
    let scale_x = pixel_cols as f64 / img_w as f64;
    let scale_y = pixel_rows as f64 / img_h as f64;
    let scale = scale_x.min(scale_y);

    let new_w = ((img_w as f64 * scale) as u32).max(1).min(pixel_cols);
    let new_h = ((img_h as f64 * scale) as u32).max(1).min(pixel_rows);

    let resized = img.resize_exact(new_w, new_h, FilterType::Triangle);

    // Calculate centering offsets
    let offset_x = (pixel_cols.saturating_sub(new_w)) / 2;
    let offset_y = (pixel_rows.saturating_sub(new_h)) / 2;

    let get_pixel = |px: u32, py: u32| -> Option<Rgba<u8>> {
        if px >= offset_x && py >= offset_y {
            let ix = px - offset_x;
            let iy = py - offset_y;
            if ix < new_w && iy < new_h {
                return Some(resized.get_pixel(ix, iy));
            }
        }
        None
    };

    let mut lines = Vec::with_capacity(height as usize);

    for row in 0..height as u32 {
        let top_y = row * 2;
        let bot_y = row * 2 + 1;

        let mut spans = Vec::with_capacity(pixel_cols as usize);

        for col in 0..pixel_cols {
            let top_pixel = get_pixel(col, top_y);
            let bot_pixel = get_pixel(col, bot_y);

            match (top_pixel, bot_pixel) {
                (Some(tp), Some(bp)) => {
                    let fg = Color::Rgb(tp[0], tp[1], tp[2]);
                    let bg = Color::Rgb(bp[0], bp[1], bp[2]);
                    spans.push(Span::styled("▀", Style::default().fg(fg).bg(bg)));
                }
                (Some(tp), None) => {
                    let fg = Color::Rgb(tp[0], tp[1], tp[2]);
                    spans.push(Span::styled("▀", Style::default().fg(fg).bg(Color::Black)));
                }
                (None, Some(bp)) => {
                    let bg = Color::Rgb(bp[0], bp[1], bp[2]);
                    spans.push(Span::styled("▀", Style::default().fg(Color::Black).bg(bg)));
                }
                (None, None) => {
                    spans.push(Span::styled(" ", Style::default()));
                }
            }
        }

        lines.push(Line::from(spans));
    }

    lines
}

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

    /// Get a cropped/zoomed view of the current image for rendering.
    fn get_view_image(&self) -> Option<DynamicImage> {
        let img = self.current_image.as_ref()?;

        if (self.zoom - 1.0).abs() < 0.01 && self.pan_x.abs() < 0.01 && self.pan_y.abs() < 0.01 {
            return Some(img.clone());
        }

        let (w, h) = img.dimensions();
        let view_w = (w as f64 / self.zoom) as u32;
        let view_h = (h as f64 / self.zoom) as u32;

        let center_x = (w as f64 / 2.0 + self.pan_x * w as f64).clamp(0.0, w as f64);
        let center_y = (h as f64 / 2.0 + self.pan_y * h as f64).clamp(0.0, h as f64);

        let x = (center_x - view_w as f64 / 2.0)
            .max(0.0)
            .min((w - view_w.min(w)) as f64) as u32;
        let y = (center_y - view_h as f64 / 2.0)
            .max(0.0)
            .min((h - view_h.min(h)) as f64) as u32;

        let crop_w = view_w.min(w - x);
        let crop_h = view_h.min(h - y);

        if crop_w == 0 || crop_h == 0 {
            return Some(img.clone());
        }

        Some(img.crop_imm(x, y, crop_w, crop_h))
    }
}

fn draw(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &App) -> io::Result<()> {
    terminal.draw(|frame| {
        let size = frame.size();

        // Layout: image area + status bar
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),    // image area
                Constraint::Length(1), // status bar
            ])
            .split(size);

        let image_area = chunks[0];
        let status_area = chunks[1];

        // Render image
        if let Some(view_img) = app.get_view_image() {
            let lines = render_image_to_lines(&view_img, image_area.width, image_area.height);
            let paragraph = Paragraph::new(lines);
            frame.render_widget(paragraph, image_area);
        } else if let Some(ref err) = app.error_message {
            let err_text = Paragraph::new(err.as_str())
                .style(Style::default().fg(Color::Red))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false });
            // Center vertically
            let vert = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(40),
                    Constraint::Min(3),
                    Constraint::Percentage(40),
                ])
                .split(image_area);
            frame.render_widget(err_text, vert[1]);
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

        let status_width = status_area.width as usize;
        let pad = status_width.saturating_sub(left.len() + right.len());

        let status_line = Line::from(vec![
            Span::styled(&left, Style::default().fg(Color::White).bg(Color::DarkGray)),
            Span::styled(
                " ".repeat(pad),
                Style::default().bg(Color::DarkGray),
            ),
            Span::styled(&right, Style::default().fg(Color::Yellow).bg(Color::DarkGray)),
        ]);

        let status = Paragraph::new(status_line);
        frame.render_widget(status, status_area);

        // Help overlay
        if app.show_help {
            let help_text = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  termview — Keyboard Shortcuts  ",
                    Style::default().fg(Color::Cyan),
                )),
                Line::from(""),
                Line::from("  ← / h       Previous image"),
                Line::from("  → / l       Next image"),
                Line::from("  Home / g    First image"),
                Line::from("  End / G     Last image"),
                Line::from("  + / =       Zoom in"),
                Line::from("  - / _       Zoom out"),
                Line::from("  0           Reset zoom"),
                Line::from("  w/a/s/d     Pan (when zoomed)"),
                Line::from("  ?           Toggle help"),
                Line::from("  q / Esc     Quit"),
                Line::from(""),
            ];

            let help_height = help_text.len() as u16;
            let help_width: u16 = 40;

            let x = (size.width.saturating_sub(help_width)) / 2;
            let y = (size.height.saturating_sub(help_height)) / 2;

            let help_rect = Rect::new(x, y, help_width, help_height);

            // Clear the area behind the popup
            let clear = Paragraph::new(
                std::iter::repeat(Line::from(" ".repeat(help_width as usize)))
                    .take(help_height as usize)
                    .collect::<Vec<_>>(),
            )
            .style(Style::default().bg(Color::Black));
            frame.render_widget(clear, help_rect);

            let help = Paragraph::new(help_text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Help ")
                        .style(Style::default().fg(Color::White).bg(Color::Black)),
                )
                .style(Style::default().fg(Color::White).bg(Color::Black));
            frame.render_widget(help, help_rect);
        }
    })?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Determine the browsing directory and collect images
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

    // Find starting index
    let start_index = if let Some(ref file) = args.file {
        if file.is_file() {
            let canonical = std::fs::canonicalize(file).unwrap_or(file.clone());
            images
                .iter()
                .position(|p| {
                    std::fs::canonicalize(p).unwrap_or(p.clone()) == canonical
                })
                .unwrap_or(0)
        } else {
            0
        }
    } else {
        0
    };

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut app = App::new(images, start_index);

    // Main event loop
    loop {
        draw(&mut terminal, &app)?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match key.code {
                    // Quit
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,

                    // Navigation
                    KeyCode::Right | KeyCode::Char('l') => app.next(),
                    KeyCode::Left | KeyCode::Char('h') => app.prev(),
                    KeyCode::Home | KeyCode::Char('g') => app.first(),
                    KeyCode::End => app.last(),
                    KeyCode::Char('G') => app.last(),

                    // Zoom
                    KeyCode::Char('+') | KeyCode::Char('=') => app.zoom_in(),
                    KeyCode::Char('-') | KeyCode::Char('_') => app.zoom_out(),
                    KeyCode::Char('0') => app.zoom_reset(),

                    // Pan (when zoomed)
                    KeyCode::Char('w') => app.pan(0.0, -0.05),
                    KeyCode::Char('s') => app.pan(0.0, 0.05),
                    KeyCode::Char('a') => app.pan(-0.05, 0.0),
                    KeyCode::Char('d') => app.pan(0.05, 0.0),

                    // Help
                    KeyCode::Char('?') => app.show_help = !app.show_help,

                    _ => {}
                }
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}