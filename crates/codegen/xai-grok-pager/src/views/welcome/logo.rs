//! Bandicot logo component with braille and ASCII-safe variants.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::render::color::blend_color;
use crate::theme::Theme;

const LOGO: &str = include_str!("../../../assets/logo/logo11.txt");
const LOGO_SMALL: &str = include_str!("../../../assets/logo/logo06.txt");
const LOGO_NARROW: &str = include_str!("../../../assets/logo/logo_ascii.txt");

/// Height at or above which the compact braille logo is shown.
const SMALL_LOGO_MIN_HEIGHT: u16 = 19;
/// Height at or above which the full logo is shown.
const FULL_LOGO_MIN_HEIGHT: u16 = 24;
/// The fallback is short enough to remain useful in constrained terminals.
const NARROW_LOGO_MIN_HEIGHT: u16 = 4;

fn pick_logo(window_width: u16, window_height: u16) -> Option<&'static str> {
    pick_logo_for(window_width, window_height, braille_unsupported())
}

/// Pure tier selection so tests can drive the legacy-console flag directly.
/// Braille-capable terminals always get the dithered braille art (animated);
/// the plain ASCII stand-in is only for legacy Windows consoles whose raster
/// fonts lack the U+2800 braille block.
fn pick_logo_for(
    window_width: u16,
    window_height: u16,
    braille_unsupported: bool,
) -> Option<&'static str> {
    if braille_unsupported {
        return if window_height >= NARROW_LOGO_MIN_HEIGHT
            && window_width >= visual_width(LOGO_NARROW)
        {
            Some(LOGO_NARROW)
        } else {
            None
        };
    }
    if window_height < SMALL_LOGO_MIN_HEIGHT || window_width < visual_width(LOGO_SMALL) {
        None
    } else if window_height < FULL_LOGO_MIN_HEIGHT || window_width < visual_width(LOGO) {
        Some(LOGO_SMALL)
    } else {
        Some(LOGO)
    }
}

fn braille_unsupported() -> bool {
    crate::glyphs::is_legacy_windows_console()
}

fn non_empty_lines(logo: &str) -> impl Iterator<Item = &str> {
    logo.lines().filter(|l| !l.is_empty())
}

fn count_lines(logo: &str) -> u16 {
    non_empty_lines(logo).count() as u16
}

fn visual_width(logo: &str) -> u16 {
    non_empty_lines(logo)
        .map(unicode_width::UnicodeWidthStr::width)
        .max()
        .unwrap_or(24) as u16
}

/// Animation phase in seconds since the first render. Wall-clock based so the
/// shimmer speed is independent of the frame rate.
fn anim_phase_secs() -> f32 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// Shimmer redraw cadence in frames per second. The sweep is slow, so a few fps
/// looks smooth while sparing the long-lived welcome screen from full-rate
/// repaints.
const SHIMMER_FPS: f32 = 12.0;

fn ear_twitch_offset(row: usize, rows: usize, secs: f32) -> usize {
    let phase = secs % 5.0;
    let twitching = (3.60..3.72).contains(&phase) || (3.84..3.96).contains(&phase);
    usize::from(twitching && row < rows.div_ceil(4)) * 2
}

/// Quantized shimmer frame for the current wall-clock phase. The welcome screen
/// redraws only when this advances, throttling the animation to ~`SHIMMER_FPS`
/// rather than the full event-loop tick rate. Pinned to 0 when the logo is
/// using the static ASCII fallback.
pub fn shimmer_frame() -> u64 {
    if braille_unsupported() {
        return 0;
    }
    (anim_phase_secs() * SHIMMER_FPS) as u64
}

/// Per-glyph shine opacity in `[0, 1]` at normalized diagonal position `diag`
/// (0 = bottom-left .. 1 = top-right) and animation time `secs`. A raised-cosine
/// band sweeps bottom-left → top-right and parks off-screen between sweeps; a
/// gentle global pulse breathes underneath it. 0 keeps the resting gray, 1 is
/// full bright.
fn shine_opacity(diag: f32, secs: f32) -> f32 {
    const BAND: f32 = 0.38; // half-width of the shine band — wider = more gradual falloff
    const CYCLE: f32 = 4.0; // seconds per sweep + rest
    const SWEEP_FRAC: f32 = 0.32; // portion of the cycle spent sweeping (~1.3s glint, rest idles)
    const SHINE: f32 = 0.33; // peak shine strength
    const PULSE: f32 = 0.06; // global breathing amount
    const PULSE_SECS: f32 = 5.0; // breathing period

    let p = (secs % CYCLE) / CYCLE;
    let q = (p / SWEEP_FRAC).min(1.0); // parks the band off-screen during the rest
    let band_pos = -BAND + q * (1.0 + 2.0 * BAND);
    let pulse = PULSE * (0.5 - 0.5 * (std::f32::consts::TAU * secs / PULSE_SECS).cos());

    let d = (diag - band_pos).abs();
    let shine = if d < BAND {
        0.5 * (1.0 + (std::f32::consts::PI * d / BAND).cos())
    } else {
        0.0
    };
    (pulse + SHINE * shine).clamp(0.0, 1.0)
}

fn render_into(area: Rect, buf: &mut Buffer, theme: &Theme, logo: &str) {
    let lines: Vec<&str> = non_empty_lines(logo).collect();
    let rows = lines.len().max(1) as f32;
    let cols = lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let secs = anim_phase_secs();

    // Blend each glyph from the resting gray toward the bright text color by its
    // shine opacity, so a sheen sweeps across the braille art. Adjacent glyphs
    // that land on the same blended color share one Span to hold down the
    // per-frame allocation.
    let base = theme.gray;
    let hilite = theme.text_primary;
    let logo_lines: Vec<Line> = lines
        .iter()
        .enumerate()
        .map(|(row, line)| {
            let mut spans: Vec<Span> = Vec::new();
            let mut run = " ".repeat(ear_twitch_offset(row, lines.len(), secs));
            let mut run_color: Option<Color> = None;
            for (col, ch) in line.chars().enumerate() {
                // Sweep along the bottom-left → top-right diagonal: the
                // coordinate grows as col increases and row decreases.
                let diag = (col as f32 + (rows - 1.0 - row as f32)) / (cols + rows);
                let color = blend_color(base, hilite, shine_opacity(diag, secs)).unwrap_or(base);
                if run_color != Some(color) {
                    if let Some(prev) = run_color {
                        spans.push(Span::styled(
                            std::mem::take(&mut run),
                            Style::default().fg(prev),
                        ));
                    }
                    run_color = Some(color);
                }
                run.push(ch);
            }
            if let Some(prev) = run_color {
                spans.push(Span::styled(run, Style::default().fg(prev)));
            }
            Line::from(spans).alignment(Alignment::Center)
        })
        .collect();
    Paragraph::new(logo_lines).render(area, buf);
}

pub fn logo_line_count(window_width: u16, window_height: u16) -> u16 {
    pick_logo(window_width, window_height).map_or(0, count_lines)
}

pub fn logo_visual_width(window_width: u16, window_height: u16) -> u16 {
    pick_logo(window_width, window_height).map_or(0, visual_width)
}

pub fn render_logo(area: Rect, buf: &mut Buffer, theme: &Theme, window_height: u16) {
    if let Some(logo) = pick_logo(area.width, window_height) {
        render_into(area, buf, theme, logo);
    }
}

/// The hero box always shows the full logo: it is laid out beside the menu, so
/// it fits whenever the box does. These report and render that logo directly,
/// independent of the size-based [`pick_logo`] tiers used by the stacked
/// layout.
pub fn full_logo_line_count() -> u16 {
    count_lines(LOGO)
}

pub fn full_logo_visual_width() -> u16 {
    visual_width(LOGO)
}

pub fn render_full_logo(area: Rect, buf: &mut Buffer, theme: &Theme) {
    let logo = if braille_unsupported() {
        LOGO_NARROW
    } else {
        LOGO
    };
    render_into(area, buf, theme, logo);
}

/// Line count of the compact or narrow logo used in minimal's welcome card.
pub fn compact_logo_line_count(width: u16) -> u16 {
    compact_logo(width).map_or(0, count_lines)
}

fn compact_logo(width: u16) -> Option<&'static str> {
    if braille_unsupported() {
        return if width >= visual_width(LOGO_NARROW) {
            Some(LOGO_NARROW)
        } else {
            None
        };
    }
    if width < visual_width(LOGO_SMALL) {
        None
    } else {
        Some(LOGO_SMALL)
    }
}

/// Render the compact logo centered in minimal's welcome card.
pub fn render_compact_logo(area: Rect, buf: &mut Buffer, theme: &Theme) {
    if let Some(logo) = compact_logo(area.width) {
        render_into(area, buf, theme, logo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_sizes_by_height() {
        let wide = visual_width(LOGO);
        assert_eq!(pick_logo_for(wide, SMALL_LOGO_MIN_HEIGHT - 1, false), None);
        assert_eq!(
            pick_logo_for(wide, SMALL_LOGO_MIN_HEIGHT, false),
            Some(LOGO_SMALL)
        );
        assert_eq!(
            pick_logo_for(wide, FULL_LOGO_MIN_HEIGHT - 1, false),
            Some(LOGO_SMALL)
        );
        assert_eq!(pick_logo_for(wide, FULL_LOGO_MIN_HEIGHT, false), Some(LOGO));
    }

    #[test]
    fn legacy_console_uses_ascii_fallback() {
        assert_eq!(
            pick_logo_for(visual_width(LOGO_NARROW), FULL_LOGO_MIN_HEIGHT, true),
            Some(LOGO_NARROW)
        );
        assert_eq!(
            pick_logo_for(visual_width(LOGO_NARROW) - 1, FULL_LOGO_MIN_HEIGHT, true),
            None
        );
    }

    #[test]
    fn logo_sizes_by_width() {
        assert!(pick_logo_for(visual_width(LOGO_SMALL) - 1, u16::MAX, false).is_none());
        assert_eq!(
            pick_logo_for(visual_width(LOGO) - 1, u16::MAX, false),
            Some(LOGO_SMALL)
        );
        assert_eq!(
            pick_logo_for(visual_width(LOGO), u16::MAX, false),
            Some(LOGO)
        );
    }

    #[test]
    fn hero_box_uses_large_logo_dimensions() {
        assert_eq!(full_logo_line_count(), count_lines(LOGO));
        assert_eq!(full_logo_visual_width(), visual_width(LOGO));
        assert!(full_logo_line_count() > count_lines(LOGO_SMALL));
        assert!(full_logo_visual_width() > visual_width(LOGO_SMALL));
    }

    #[test]
    fn logo_assets_fit_their_reported_widths() {
        for logo in [LOGO, LOGO_SMALL, LOGO_NARROW] {
            assert!(non_empty_lines(logo).all(|line| {
                unicode_width::UnicodeWidthStr::width(line) <= visual_width(logo) as usize
            }));
        }
        assert!(LOGO_NARROW.is_ascii());
    }

    #[test]
    fn selected_logo_never_exceeds_terminal_width() {
        for width in 0..=visual_width(LOGO) {
            if let Some(logo) = pick_logo_for(width, u16::MAX, false) {
                assert!(visual_width(logo) <= width, "width {width}");
            }
        }
    }

    #[test]
    fn compact_logo_shows_dither_art_when_it_fits() {
        assert_eq!(compact_logo_line_count(visual_width(LOGO_SMALL) - 1), 0);
        assert_eq!(
            compact_logo_line_count(visual_width(LOGO_SMALL)),
            count_lines(LOGO_SMALL)
        );
    }

    #[test]
    fn shine_opacity_stays_in_unit_range() {
        let mut secs = 0.0;
        while secs < 10.0 {
            for i in 0..=20 {
                let diag = i as f32 / 20.0;
                let op = shine_opacity(diag, secs);
                assert!(
                    (0.0..=1.0).contains(&op),
                    "opacity {op} out of range at diag {diag}, secs {secs}"
                );
            }
            secs += 0.13;
        }
    }

    #[test]
    fn shine_band_sweeps_across() {
        // The brightest point along the diagonal advances left → right as the
        // sweep progresses through its active phase.
        let brightest = |secs: f32| -> f32 {
            (0..=100)
                .map(|i| i as f32 / 100.0)
                .max_by(|a, b| {
                    shine_opacity(*a, secs)
                        .partial_cmp(&shine_opacity(*b, secs))
                        .unwrap()
                })
                .unwrap()
        };
        let early = brightest(0.1);
        let mid = brightest(0.4);
        let late = brightest(0.7);
        assert!(early < mid, "early {early} should precede mid {mid}");
        assert!(mid < late, "mid {mid} should precede late {late}");
    }

    #[test]
    fn shine_rests_dim_between_sweeps() {
        // During the rest phase the band is parked off-screen, so an interior
        // glyph falls back to at most the gentle pulse — never full bright.
        let op = shine_opacity(0.5, 6.0); // secs % 4.0 = 2.0 → past SWEEP_FRAC, in the rest phase
        assert!(op < 0.2, "resting opacity {op} should stay dim");
    }

    #[test]
    fn ears_twitch_twice_without_moving_the_body() {
        let rows = count_lines(LOGO) as usize;
        assert_eq!(ear_twitch_offset(0, rows, 3.65), 2);
        assert_eq!(ear_twitch_offset(0, rows, 3.90), 2);
        assert_eq!(ear_twitch_offset(0, rows, 4.10), 0);
        assert_eq!(ear_twitch_offset(rows / 2, rows, 3.65), 0);
    }
}
