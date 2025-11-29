//! PNG logo renderer for capable terminals.
//!
//! Goals:
//!   - Prefer the real PNG logo when the terminal supports inline images.
//!   - Respect the logical logo slot provided by the layout
//!     (`Rect` from `logo::draw_logo` – currently 7 rows high).
//!   - Preserve the logo’s aspect ratio (ask for fixed *cell height*,
//!     let the backend determine width).
//!   - Render at most once per geometry (no flicker, no per-tick PNG spam).
//!   - Fall back to ASCII logo for any failure or unsupported backend.
//!
//! Environment overrides:
//!   - `CHALYBS_TUI_IMAGE_BACKEND` = kitty | iterm | none | auto
//!   - `CHALYBS_TUI_LOGO_PNG`      = custom PNG path
//!   - `CHALYBS_TUI_LOGO_HEIGHT_SCALE` = float (default 1.0)

use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::VisualEffects;
use crate::config::LogoHaloProfile;
use crate::{logo, theme};

/// Runtime-detected backend for image rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageBackend {
    Kitty,
    ITerm,
    None,
}

/// Default height scale — tuned for your environment.
const DEFAULT_HEIGHT_SCALE: f32 = 1.0;

/// Logical maximum halo width in cells. This does *not* need to be the
/// entire status bar width; it is the width of the small band that hugs
/// the logo horizontally.
const HALO_MAX_WIDTH: u16 = 32;

/// All halo masks are 7 rows high.
const MASK_ROWS: usize = 7;
const MASK_COLS: usize = 7;

type Mask = [[u8; MASK_COLS]; MASK_ROWS];

/// ---------------------------------------------------------------------------
/// Set C masks (LEFT side; RIGHT is mirrored).
///
/// Intensities: 0 = none, 1 = light, 2 = medium, 3 = heavy.
/// ---------------------------------------------------------------------------

const C3_MASK: Mask = [
    // thinner center, balanced taper
    [3, 1, 0, 0, 0, 0, 0],
    [3, 2, 1, 0, 0, 0, 0],
    [3, 3, 1, 0, 0, 0, 0],
    [3, 3, 1, 0, 0, 0, 0],
    [3, 3, 1, 0, 0, 0, 0],
    [3, 2, 1, 0, 0, 0, 0],
    [3, 1, 0, 0, 0, 0, 0],
];

const C3_NARROW_MASK: Mask = [
    // thinnest wings
    [3, 0, 0, 0, 0, 0, 0],
    [3, 1, 0, 0, 0, 0, 0],
    [3, 1, 0, 0, 0, 0, 0],
    [3, 1, 0, 0, 0, 0, 0],
    [3, 1, 0, 0, 0, 0, 0],
    [3, 1, 0, 0, 0, 0, 0],
    [3, 0, 0, 0, 0, 0, 0],
];

const C3_WIDE_MASK: Mask = [
    // heavier, reaches further out
    [3, 3, 1, 0, 0, 0, 0],
    [3, 3, 2, 1, 0, 0, 0],
    [3, 3, 3, 1, 0, 0, 0],
    [3, 3, 3, 2, 1, 0, 0],
    [3, 3, 3, 2, 1, 0, 0],
    [3, 3, 2, 1, 0, 0, 0],
    [3, 3, 1, 0, 0, 0, 0],
];

const C3_XWIDE_MASK: Mask = [
    // heaviest wings; still keeps a center gap
    [3, 3, 3, 1, 0, 0, 0],
    [3, 3, 3, 2, 1, 0, 0],
    [3, 3, 3, 3, 1, 0, 0],
    [3, 3, 3, 3, 2, 1, 0],
    [3, 3, 3, 3, 2, 1, 0],
    [3, 3, 3, 2, 1, 0, 0],
    [3, 3, 3, 1, 0, 0, 0],
];

/// Cached PNG path.
static LOGO_PNG_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Tracks the last geometry we rendered, so we do not re-emit PNGs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RenderGeom {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    scaled_height: u16,
}

static LAST_RENDER_GEOM: OnceLock<Mutex<Option<RenderGeom>>> = OnceLock::new();

fn last_render_geom() -> &'static Mutex<Option<RenderGeom>> {
    LAST_RENDER_GEOM.get_or_init(|| Mutex::new(None))
}

/// Called from `logo.rs`.
///
/// Returns `true` if the PNG path is active for this region, `false` if
/// the caller should fall back to ASCII.
///
/// Halo behaviour:
///   - Controlled by `effects.logo_halo`.
///   - We draw a **bow-tie wing** halo *behind* the PNG, not a solid bar.
///   - The halo is a narrow, centered band around the logo, not the entire
///     status bar.
pub fn draw_png_logo(f: &mut Frame, area: Rect, tick: u64, effects: &VisualEffects) -> bool {
    if area.width == 0 || area.height == 0 {
        return false;
    }

    let backend = detect_backend();
    if matches!(backend, ImageBackend::None) {
        return false;
    }

    let path = match resolve_logo_path() {
        Some(p) => p,
        None => return false,
    };

    let scale = resolve_height_scale();
    let scaled_height = (area.height as f32 * scale).round() as u16;

    let geom = RenderGeom {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height,
        scaled_height,
    };

    // 1) Draw halo *first*, using the same logical slot as the PNG,
    //    but as a centered, narrow band.
    draw_logo_halo(f, area, effects.logo_halo, tick);

    // 2) Emit PNG only when geometry changes; otherwise keep existing.
    {
        let mut guard = match last_render_geom().lock() {
            Ok(g) => g,
            Err(_) => return false,
        };

        if guard.as_ref() == Some(&geom) {
            // Geometry unchanged → halo updated, PNG reused.
            return true;
        }

        if let Err(_) = render_png_at(path, &geom, backend) {
            *guard = None;
            return false;
        }

        *guard = Some(geom);
    }

    true
}

/// Determine height scaling factor.
fn resolve_height_scale() -> f32 {
    match env::var("CHALYBS_TUI_LOGO_HEIGHT_SCALE") {
        Ok(v) => v.parse::<f32>().unwrap_or(DEFAULT_HEIGHT_SCALE),
        Err(_) => DEFAULT_HEIGHT_SCALE,
    }
}

/// Identify backend.
fn detect_backend() -> ImageBackend {
    if let Ok(val) = env::var("CHALYBS_TUI_IMAGE_BACKEND") {
        let v = val.to_ascii_lowercase();
        return match v.as_str() {
            "kitty" => ImageBackend::Kitty,
            "iterm" | "iterm2" => ImageBackend::ITerm,
            "none" | "off" | "disable" => ImageBackend::None,
            _ => detect_backend_auto(),
        };
    }

    detect_backend_auto()
}

fn detect_backend_auto() -> ImageBackend {
    if env::var("KITTY_WINDOW_ID").is_ok() {
        return ImageBackend::Kitty;
    }

    if let Ok(term) = env::var("TERM") {
        if term.to_ascii_lowercase().contains("xterm-kitty") {
            return ImageBackend::Kitty;
        }
    }

    if let Ok(tp) = env::var("TERM_PROGRAM") {
        if tp == "iTerm.app" {
            return ImageBackend::ITerm;
        }
    }

    ImageBackend::None
}

/// Locate PNG path (cached).
fn resolve_logo_path() -> Option<&'static Path> {
    LOGO_PNG_PATH
        .get_or_init(|| {
            if let Ok(path) = env::var("CHALYBS_TUI_LOGO_PNG") {
                let p = PathBuf::from(path);
                if p.is_file() {
                    return Some(p);
                }
            }

            let local = PathBuf::from("assets/chalybs.png");
            if local.is_file() {
                return Some(local);
            }

            let system = PathBuf::from("/usr/share/chalybs/chalybs.png");
            if system.is_file() {
                return Some(system);
            }

            None
        })
        .as_deref()
}

/// Emit PNG via viuer.
fn render_png_at(
    path: &Path,
    geom: &RenderGeom,
    backend: ImageBackend,
) -> Result<(), Box<dyn std::error::Error>> {
    use viuer::Config;

    let mut config = Config {
        x: geom.x as u16,
        y: geom.y as i16,
        height: Some(geom.scaled_height as u32),
        ..Default::default()
    };

    match backend {
        ImageBackend::Kitty => {
            config.use_kitty = true;
            config.use_iterm = false;
        }
        ImageBackend::ITerm => {
            config.use_kitty = false;
            config.use_iterm = true;
        }
        ImageBackend::None => {}
    }

    // PNG should have transparent background so the halo shows through.
    config.transparent = true;

    viuer::print_from_file(path, &config)?;
    Ok(())
}

/// ---------------------------------------------------------------------------
/// HALO IMPLEMENTATION (Set C wings, behind PNG)
/// ---------------------------------------------------------------------------

fn clamp01(v: f32) -> f32 {
    if v < 0.0 {
        0.0
    } else if v > 1.0 {
        1.0
    } else {
        v
    }
}

/// Blend between two RGB colors.
fn blend_color(a: Color, b: Color, t: f32) -> Color {
    let t = clamp01(t);
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            let fr = ar as f32 * (1.0 - t) + br as f32 * t;
            let fg = ag as f32 * (1.0 - t) + bg as f32 * t;
            let fb = ab as f32 * (1.0 - t) + bb as f32 * t;
            Color::Rgb(fr as u8, fg as u8, fb as u8)
        }
        _ => a,
    }
}

/// Compute a halo color influenced by the logo breathing curve.
fn halo_color_for_breath(breath: f32) -> Color {
    // Map breath ~0.88..1.12 into [0.0 .. 1.0] for teal↔pink blending.
    let t = clamp01((breath - 0.88) / (1.12 - 0.88));

    let base_teal = theme::palette::ACCENT_TEAL;
    let base_pink = theme::palette::ACCENT_PINK;
    let bg = theme::palette::BG;

    let accent = blend_color(base_teal, base_pink, t);
    let subtle = blend_color(bg, accent, 0.25);

    theme::adjust_brightness_soft(subtle, breath)
}

/// Sample intensity from a mask given:
///   - row index 0..MASK_ROWS
///   - x position within the halo rect (0..width)
///
/// We treat the mask as radial bands away from the centerline, with a
/// central gap where intensity = 0.
fn sample_mask(mask: &Mask, row: usize, x: u16, width: u16) -> u8 {
    if width == 0 {
        return 0;
    }

    // Normalize x into [-1, 1] with 0 = centerline.
    let w = width as f32;
    let cx = (w - 1.0) * 0.5;
    let dx = (x as f32 - cx) / cx.max(1.0);
    let ax = dx.abs();

    // Center gap: keep PNG "core gap" clear.
    const GAP_FRACTION: f32 = 0.25;
    if ax < GAP_FRACTION {
        return 0;
    }

    // Map remaining [GAP..1] into [0..MASK_COLS).
    let t = ((ax - GAP_FRACTION) / (1.0 - GAP_FRACTION)).max(0.0);
    let mut idx = (t * MASK_COLS as f32).floor() as usize;
    if idx >= MASK_COLS {
        idx = MASK_COLS - 1;
    }

    // The masks are defined from **outer edge → toward centre**.
    // Our distance mapping is centre → outward, so flip the index
    // so the narrow/high-intensity part sits near the middle.
    let col = MASK_COLS - 1 - idx;
    mask[row][col]
}

/// Draw a breathing dual-wing halo band behind the logo.
///
/// Profiles:
///   - Off         → no halo at all.
///   - C3          → standard wings.
///   - C3Narrow    → thinner wings.
///   - C3Wide      → heavier wings.
///   - C3ExtraWide → heaviest wings.
fn draw_logo_halo(f: &mut Frame, area: Rect, profile: LogoHaloProfile, tick: u64) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    if matches!(profile, LogoHaloProfile::Off) {
        return;
    }

    let mask: &Mask = match profile {
        LogoHaloProfile::Off => return,
        LogoHaloProfile::C3 => &C3_MASK,
        LogoHaloProfile::C3Narrow => &C3_NARROW_MASK,
        LogoHaloProfile::C3Wide => &C3_WIDE_MASK,
        LogoHaloProfile::C3ExtraWide => &C3_XWIDE_MASK,
    };

    // Breathing colour.
    let breath = logo::logo_breath_factor(tick);
    let fg = halo_color_for_breath(breath);

    // --- Halo geometry -----------------------------------------------------
    //
    // The bug you’ve been fighting:
    //   - I was centering the halo inside the ENTIRE left pane
    //     instead of the logo slot → wings in the middle.
    //   - I was vertically centering → chopped bottoms.
    //
    // Fix:
    //   - Anchor the halo to the **top-left of `area`**, same place
    //     we tell viuer to start drawing the PNG.
    //   - Keep height at MASK_ROWS (or less if the slot is shorter).
    //   - Keep width a narrow band, starting at area.x.

    // Height: up to MASK_ROWS, aligned with the top of the logo slot.
    let halo_height = area.height.min(MASK_ROWS as u16).max(1);
    let halo_y = area.y;

    // Target width: 2 * MASK_COLS - 1 → left + right wings with a
    // conceptual centre gap in the sampling, but clamp to slot width.
    let desired_width: u16 = (MASK_COLS as u16)
        .saturating_mul(2)
        .saturating_sub(1)
        .min(HALO_MAX_WIDTH);

    let halo_width = area.width.min(desired_width).max(1);

    // ***Key change***: anchor to the LEFT edge of the logo slot,
    // not centered in the whole pane.
    let halo_x = area.x;

    let halo_rect = Rect {
        x: halo_x,
        y: halo_y,
        width: halo_width,
        height: halo_height,
    };

    // --- Build visual lines -----------------------------------------------

    let mut lines: Vec<Line> = Vec::with_capacity(halo_rect.height as usize);

    for row in 0..halo_rect.height {
        // Map visual row into mask row.
        let mask_row = ((row as usize * MASK_ROWS) / halo_rect.height as usize).min(MASK_ROWS - 1);

        let mut buf = String::with_capacity(halo_rect.width as usize);

        for x in 0..halo_rect.width {
            let intensity = sample_mask(mask, mask_row, x, halo_rect.width);
            let ch = match intensity {
                0 => ' ',
                1 => '░',
                2 => '▒',
                _ => '█',
            };
            buf.push(ch);
        }

        // ***Key change***: do NOT force a background colour here.
        // Let the pane's existing background show through instead of
        // painting a solid black block.
        let style = Style::default().fg(fg);
        lines.push(Line::from(Span::styled(buf, style)));
    }

    let block = Block::default().borders(Borders::NONE);
    let paragraph = Paragraph::new(lines).block(block);

    f.render_widget(paragraph, halo_rect);
}
