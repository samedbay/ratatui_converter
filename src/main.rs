use std::{
    error::Error,
    fs::File,
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

use ratatui::{
    backend::{Backend, CrosstermBackend},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};

use image::codecs::gif::GifDecoder;
use image::{imageops, AnimationDecoder, ImageBuffer, Rgba, RgbaImage};

/// Holds the braille + color lines for a single frame (no per‐frame delay).
struct BrailleFrame<'a> {
    lines: Vec<Line<'a>>,
}

fn main() -> Result<(), Box<dyn Error>> {
    // 1) Parse CLI argument: path to GIF
    let mut args = std::env::args().skip(1);
    let gif_path = match args.next() {
        Some(path) => PathBuf::from(path),
        None => {
            eprintln!("Usage: gif_braille_tui <path_to_gif>");
            std::process::exit(1);
        }
    };

    // 2) Decode + convert all frames into braille/color lines
    let frames = load_and_convert_gif(&gif_path)?;
    if frames.is_empty() {
        eprintln!("No frames found or failed to decode GIF.");
        std::process::exit(1);
    }

    // 3) Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 4) Run the TUI loop to display frames at ~60 fps
    let res = run_app(&mut terminal, &frames);

    // 5) Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {err:?}");
    }
    Ok(())
}

/// Reads a GIF from disk, merges partial frames, converts each to braille lines without
/// distorting the original aspect ratio, using a **higher‐quality Lanczos3** filter.
fn load_and_convert_gif(path: &PathBuf) -> Result<Vec<BrailleFrame<'static>>, Box<dyn Error>> {
    let file_in = File::open(path)?;
    let decoder = GifDecoder::new(file_in)?;
    let frames_iter = decoder.into_frames().collect_frames()?;

    // Query terminal size, compute max braille cells => max pixel dims
    let (term_cols, term_rows) = crossterm::terminal::size()?;
    let max_braille_cols = (term_cols as u32).saturating_sub(2);
    let max_braille_rows = (term_rows as u32).saturating_sub(2);
    let max_width_px = max_braille_cols * 2;
    let max_height_px = max_braille_rows * 4;

    let mut out_frames = Vec::with_capacity(frames_iter.len());

    for frame in frames_iter {
        let rgba = frame.buffer();
        let width = rgba.width();
        let height = rgba.height();
        let Some(image) = RgbaImage::from_raw(width, height, rgba.clone().into_raw()) else {
            continue;
        };

        // -- Keep aspect ratio --
        let (new_width, new_height) = compute_scaled_dims(width, height, max_width_px, max_height_px);

        let resized = if new_width > 0 && new_height > 0 {
            imageops::resize(
                &image,
                new_width,
                new_height,
                // Higher‐quality filter for smoother downscaling
                imageops::FilterType::Lanczos3,
            )
        } else {
            ImageBuffer::<Rgba<u8>, _>::new(1, 1)
        };

        // Convert to braille + color lines
        let braille_lines = rgba_to_braille_colored(resized);
        out_frames.push(BrailleFrame { lines: braille_lines });
    }

    Ok(out_frames)
}

/// Compute new dimensions for the image, preserving aspect ratio,
/// so it fits within (max_w, max_h).
fn compute_scaled_dims(
    orig_w: u32,
    orig_h: u32,
    max_w: u32,
    max_h: u32,
) -> (u32, u32) {
    if max_w == 0 || max_h == 0 || orig_w == 0 || orig_h == 0 {
        return (0, 0);
    }

    let orig_w_f = orig_w as f32;
    let orig_h_f = orig_h as f32;
    let max_w_f = max_w as f32;
    let max_h_f = max_h as f32;

    // scale factor to fit width
    let scale_w = max_w_f / orig_w_f;
    // scale factor to fit height
    let scale_h = max_h_f / orig_h_f;
    let scale = scale_w.min(scale_h);

    // if the image is smaller already, skip upscaling
    let scale = scale.min(1.0);

    let new_w = (orig_w_f * scale).round().max(1.0) as u32;
    let new_h = (orig_h_f * scale).round().max(1.0) as u32;
    (new_w, new_h)
}

/// Convert an RGBA image into multi‐line braille cells with 24‐bit color.
fn rgba_to_braille_colored(img: RgbaImage) -> Vec<Line<'static>> {
    let width = img.width();
    let height = img.height();

    // Each braille cell is 2 px wide, 4 px tall
    let cell_cols = (width + 1) / 2;
    let cell_rows = (height + 3) / 4;

    let mut lines = Vec::with_capacity(cell_rows as usize);

    for row in 0..cell_rows {
        let mut span_vec = Vec::with_capacity(cell_cols as usize);

        for col in 0..cell_cols {
            let mut r_sum: u32 = 0;
            let mut g_sum: u32 = 0;
            let mut b_sum: u32 = 0;
            let mut count: u32 = 0;
            let mut dots: u8 = 0;

            for sub_row in 0..4 {
                for sub_col in 0..2 {
                    let px_x = col * 2 + sub_col;
                    let px_y = row * 4 + sub_row;

                    if px_x < width && px_y < height {
                        let Rgba([r, g, b, a]) = *img.get_pixel(px_x, px_y);

                        // Map (sub_col, sub_row) => braille bit
                        let bit_index = match (sub_col, sub_row) {
                            (0, 0) => 0, // dot1
                            (0, 1) => 1, // dot2
                            (0, 2) => 2, // dot3
                            (0, 3) => 3, // dot7
                            (1, 0) => 4, // dot4
                            (1, 1) => 5, // dot5
                            (1, 2) => 6, // dot6
                            (1, 3) => 7, // dot8
                            _ => 0,
                        };

                        // Simple brightness threshold
                        let lum = 0.2126 * (r as f32)
                            + 0.7152 * (g as f32)
                            + 0.0722 * (b as f32);
                        if a > 50 && lum > 20.0 {
                            dots |= 1 << bit_index;
                        }

                        r_sum += r as u32;
                        g_sum += g as u32;
                        b_sum += b as u32;
                        count += 1;
                    }
                }
            }

            let braille_char = char::from_u32(0x2800 + dots as u32).unwrap_or(' ');
            let (avg_r, avg_g, avg_b) = if count > 0 {
                ((r_sum / count) as u8, (g_sum / count) as u8, (b_sum / count) as u8)
            } else {
                (0, 0, 0)
            };

            // “Leak” the single‐char string to get 'static lifetime
            let content: &'static str = Box::leak(braille_char.to_string().into_boxed_str());

            // Create a colored span
            let span = Span::styled(
                content,
                Style::default()
                    .fg(Color::Rgb(avg_r, avg_g, avg_b))
                    .add_modifier(Modifier::BOLD),
            );
            span_vec.push(span);
        }

        lines.push(Line::from(span_vec));
    }

    lines
}

/// Runs the TUI loop with ~60 fps. Press `q` to quit.
fn run_app<B: Backend>(terminal: &mut Terminal<B>, frames: &[BrailleFrame<'static>]) -> io::Result<()> {
    // ~16 ms per frame => ~60 fps
    let frame_delay = Duration::from_millis(96);
    let mut frame_index = 0;
    let mut frame_start = Instant::now();

    loop {
        // 1) Draw current frame
        terminal.draw(|f| {
            let size = f.area(); // use .area() over .size()
            let block = Block::default().borders(Borders::ALL).title("GIF - Braille (Hi-Qual)");
            let current_frame = &frames[frame_index];

            let paragraph = Paragraph::new(current_frame.lines.clone()).block(block);
            f.render_widget(paragraph, size);
        })?;

        // 2) Check for user input
        let elapsed = frame_start.elapsed();
        let time_left = frame_delay.saturating_sub(elapsed);

        if event::poll(time_left)? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') {
                    return Ok(());
                }
            }
        }

        // 3) Next frame if we've passed ~16 ms
        if frame_start.elapsed() >= frame_delay {
            frame_index = (frame_index + 1) % frames.len();
            frame_start = Instant::now();
        }
    }
}
