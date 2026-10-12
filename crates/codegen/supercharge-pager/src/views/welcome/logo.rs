//! The logo is hidden entirely on legacy Windows consoles: the ConHost raster fonts do not cover the U+2800 braille block, so it renders as tofu.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::ascii_logo;

use crate::theme::Theme;

const WORDMARK: &str = "SUPERCHARGE";
const TAGLINE: &str = "AI coding workspace";
const LOGO_ART: &str = "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀\n⠀⠀⠀⠀⠀⠀⠀⢾⣶⣄⣀⣤⣶⣶⣦⣄⡀⠀⠀⠀\n⠀⠀⠀⠀⠀⠀⠀⣨⣿⣿⣿⣿⣿⣿⣿⣿⣿⣦⠀⠀\n⠀⠠⠤⠤⠄⢡⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡇⠀\n⠀⢀⣠⣭⣥⣼⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠃⠀\n⠐⠊⠉⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠃⠀⠀\n⠀⠀⠈⠠⠈⢻⣿⡿⣿⣿⣿⣿⣿⣿⣿⣽⣷⡀⠀⠀\n⠀⠀⠀⠀⠀⢐⣴⣿⡿⢻⣿⣿⠟⠈⠛⠛⠋⠀⠀⠀\n⠀⠀⠀⠀⠠⠬⣭⣅⣀⣘⣛⣋⠀⠀⠀⠀⠀⠀⠀⠀";
const COIN_FRAMES: [&str; 5] = [
    "      .-========-.      \n    .'      ⚡      `.    \n   /        ⚡        \\   \n  |         ⚡         |  \n   \\        ⚡        /   \n    `.              .'    \n      `-========-'      ",
    "       .-======-.       \n     .'    ⚡    `.     \n    /      ⚡      \\    \n   |       ⚡       |   \n    \\      ⚡      /    \n     `.          .'     \n       `-======-'       ",
    "          ║           \n          ║           \n          ║           \n          ⚡           \n          ║           \n          ║           \n          ║           ",
    "       .-======-.       \n     .'    ⚡    `.     \n    /      ⚡      \\    \n   |       ⚡       |   \n    \\      ⚡      /    \n     `.          .'     \n       `-======-'       ",
    "      .-========-.      \n    .'      ⚡      `.    \n   /        ⚡        \\   \n  |         ⚡         |  \n   \\        ⚡        /   \n    `.              .'    \n      `-========-'      ",
];
const LOGO: &str = "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀\n⠀⠀⠀⠀⠀⠀⠀⢾⣶⣄⣀⣤⣶⣶⣦⣄⡀⠀⠀⠀\n⠀⠀⠀⠀⠀⠀⠀⣨⣿⣿⣿⣿⣿⣿⣿⣿⣿⣦⠀⠀\n⠀⠠⠤⠤⠄⢡⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡇⠀\n⠀⢀⣠⣭⣥⣼⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠃⠀\n⠐⠊⠉⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠃⠀⠀\n⠀⠀⠈⠠⠈⢻⣿⡿⣿⣿⣿⣿⣿⣿⣿⣽⣷⡀⠀⠀\n⠀⠀⠀⠀⠀⢐⣴⣿⡿⢻⣿⣿⠟⠈⠛⠛⠋⠀⠀⠀\n⠀⠀⠀⠀⠠⠬⣭⣅⣀⣘⣛⣋⠀⠀⠀⠀⠀⠀⠀⠀\nSUPERCHARGE\nAI coding workspace\n";
const LOGO_SMALL: &str = "⠀⠀⠀⠀⢀⡀⠀⣀⣀⠀⠀⠀\n⠀⠀⠀⠀⣨⣿⣿⣿⣿⣿⣦⠀\n⠀⢉⣟⣸⣿⣿⣿⣿⣿⣿⣿⠁\n⠀⠉⠻⢿⣿⣿⣿⣿⣿⣿⡃⠀\n⠀⠀⠀⢨⡿⠟⣿⠟⠙⠛⠁⠀\n⠀⠀⠀⠉⠁⠀⠉⠀⠀⠀⠀⠀\nSUPERCHARGE\n";

/// Height at or above which the small logo is shown (below it, no logo).
const SMALL_LOGO_MIN_HEIGHT: u16 = 22;
/// Height at or above which the full logo is shown.
const FULL_LOGO_MIN_HEIGHT: u16 = 26;

/// Which logo art the stacked column shows.
/// The terminal height picks the tier; the stacked layout steps it down only while the column would not fit beside the draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogoTier {
    Ascii,
    Full,
    Compact,
    Hidden,
}

impl LogoTier {
    pub fn for_height(window_height: u16) -> Self {
        Self::for_height_and_hidden(window_height, logo_hidden())
    }

    /// Takes the legacy-console flag as a parameter so tests can drive it directly.
    fn for_height_and_hidden(window_height: u16, hidden: bool) -> Self {
        if hidden || window_height < SMALL_LOGO_MIN_HEIGHT {
            Self::Hidden
        } else if window_height < FULL_LOGO_MIN_HEIGHT {
            Self::Compact
        } else {
            Self::Full
        }
    }

    fn art(self) -> Option<&'static str> {
        match self {
            Self::Ascii => None,
            Self::Full => Some(LOGO),
            Self::Compact => Some(LOGO_SMALL),
            Self::Hidden => None,
        }
    }

    pub fn rows(self) -> u16 {
        if self == Self::Ascii {
            ascii_logo::ROWS
        } else {
            self.art().map_or(0, count_lines)
        }
    }

    /// The next smaller tier; `None` once hidden.
    pub fn step_down(self) -> Option<Self> {
        match self {
            Self::Ascii => Some(Self::Full),
            Self::Full => Some(Self::Compact),
            Self::Compact => Some(Self::Hidden),
            Self::Hidden => None,
        }
    }
}

fn pick_logo(window_height: u16) -> Option<&'static str> {
    pick_logo_for(window_height, logo_hidden())
}

fn pick_logo_for(window_height: u16, hidden: bool) -> Option<&'static str> {
    LogoTier::for_height_and_hidden(window_height, hidden).art()
}

/// The wordmark is safe for the terminals Supercharge supports.
fn logo_hidden() -> bool {
    false
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

static ANIMATION_START: OnceLock<Mutex<Instant>> = OnceLock::new();

fn animation_elapsed() -> Duration {
    ANIMATION_START
        .get_or_init(|| Mutex::new(Instant::now()))
        .lock()
        .unwrap()
        .elapsed()
}

pub fn restart_animation() {
    *ANIMATION_START
        .get_or_init(|| Mutex::new(Instant::now()))
        .lock()
        .unwrap() = Instant::now();
}

/// Low-frequency phase for the welcome logo.
pub fn shimmer_frame() -> u64 {
    (animation_elapsed().as_millis() / Duration::from_millis(83).as_millis()) as u64
}

fn animated_logo(frame: u64) -> String {
    let coin = COIN_FRAMES
        .get((frame as usize) % COIN_FRAMES.len())
        .copied()
        .unwrap_or_default();
    format!("{coin}\n{WORDMARK}\n{TAGLINE}\n")
}

fn animated_logo_layout() -> String {
    animated_logo(0)
}

/// Per-glyph shine opacity in `[0, 1]` at normalized diagonal position `diag` (0 is bottom-left, 1 is top-right) and animation time `secs`.
/// A raised-cosine band sweeps from bottom-left to top-right and parks off-screen between sweeps; a gentle global pulse breathes underneath it.
/// 0 keeps the resting gray, 1 is full bright.
fn shine_opacity(diag: f32, secs: f32) -> f32 {
    const BAND: f32 = 0.38; // half-width of the shine band; wider means a more gradual falloff
    const CYCLE: f32 = 4.0; // seconds for one sweep plus its rest
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
    let logo_lines: Vec<Line> = lines
        .iter()
        .map(|line| {
            if let Some(wordmark) = line.strip_prefix("⚡  ")
                && wordmark == WORDMARK
            {
                Line::from(vec![
                    Span::styled(
                        "⚡",
                        Style::default()
                            .fg(theme.accent_assistant)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        wordmark,
                        Style::default()
                            .fg(theme.text_primary)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
                .alignment(Alignment::Center)
            } else if *line == TAGLINE {
                Line::from(Span::styled(*line, Style::default().fg(theme.gray_bright)))
                    .alignment(Alignment::Center)
            } else {
                Line::from(Span::styled(*line, Style::default().fg(theme.gray)))
                    .alignment(Alignment::Center)
            }
        })
        .collect();
    Paragraph::new(logo_lines).render(area, buf);
}

pub fn logo_line_count(window_height: u16) -> u16 {
    pick_logo(window_height).map_or(0, count_lines)
}

pub fn render_logo(area: Rect, buf: &mut Buffer, theme: &Theme, window_height: u16) {
    if let Some(logo) = pick_logo(window_height) {
        render_into(area, buf, theme, logo);
    }
}

/// Paint the tier the layout reserved rows for, so the art can never outgrow its slot.
pub fn render_logo_tier(area: Rect, buf: &mut Buffer, theme: &Theme, tier: LogoTier) {
    if tier == LogoTier::Ascii {
        ascii_logo::render(area, buf, theme, animation_elapsed().as_secs_f64());
    } else if let Some(logo) = tier.art() {
        render_into(area, buf, theme, logo);
    }
}

/// The hero box always shows the full logo: it is laid out beside the menu, so it fits whenever the box does.
/// These report and render that logo directly, independent of the height-based [`pick_logo`] tiers used by the stacked layout.
/// When [`logo_hidden`], they report 0 and render nothing.
pub fn full_logo_line_count() -> u16 {
    full_logo_line_count_for(logo_hidden())
}

fn full_logo_line_count_for(hidden: bool) -> u16 {
    if hidden {
        0
    } else {
        count_lines(&animated_logo_layout())
    }
}

pub fn full_logo_visual_width() -> u16 {
    full_logo_visual_width_for(logo_hidden())
}

fn full_logo_visual_width_for(hidden: bool) -> u16 {
    if hidden {
        0
    } else {
        visual_width(&animated_logo_layout())
    }
}

pub fn render_full_logo(area: Rect, buf: &mut Buffer, theme: &Theme) {
    if !logo_hidden() {
        if area.width >= ascii_logo::COLS && area.height >= ascii_logo::ROWS {
            ascii_logo::render(area, buf, theme, animation_elapsed().as_secs_f64());
        } else {
            render_into(area, buf, theme, &animated_logo(shimmer_frame()));
        }
    }
}

pub fn render_hero_logo(area: Rect, buf: &mut Buffer, theme: &Theme) {
    if logo_hidden() {
        return;
    }
    if area.width >= ascii_logo::SMALL_COLS && area.height >= ascii_logo::SMALL_ROWS {
        ascii_logo::render_small(area, buf, theme, animation_elapsed().as_secs_f64());
    } else {
        render_full_logo(area, buf, theme);
    }
}

/// Line count of the small logo used in minimal's committed welcome card (0 on a legacy Windows console, where the braille art is suppressed).
pub fn compact_logo_line_count() -> u16 {
    if logo_hidden() {
        0
    } else {
        count_lines(LOGO_SMALL)
    }
}

/// Render the small braille logo (centered) into `area` for minimal's welcome card.
/// No-op when the logo is hidden.
pub fn render_compact_logo(area: Rect, buf: &mut Buffer, theme: &Theme) {
    if !logo_hidden() {
        render_into(area, buf, theme, LOGO_SMALL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_sizes_by_height() {
        assert!(pick_logo_for(SMALL_LOGO_MIN_HEIGHT - 1, false).is_none());
        assert_eq!(
            pick_logo_for(SMALL_LOGO_MIN_HEIGHT, false),
            Some(LOGO_SMALL)
        );
        assert_eq!(
            pick_logo_for(FULL_LOGO_MIN_HEIGHT - 1, false),
            Some(LOGO_SMALL)
        );
        assert_eq!(pick_logo_for(FULL_LOGO_MIN_HEIGHT, false), Some(LOGO));
    }

    // The braille art has no legacy-safe stand-in, so every height tier must collapse to no logo when the legacy-console flag is set
    #[test]
    fn logo_hidden_on_legacy_console_at_every_height() {
        for h in [0, SMALL_LOGO_MIN_HEIGHT, FULL_LOGO_MIN_HEIGHT, u16::MAX] {
            assert!(pick_logo_for(h, true).is_none(), "height {h}");
        }
    }

    #[test]
    fn full_logo_renders_ascii_art_in_its_reserved_area() {
        assert_eq!(LogoTier::Ascii.rows(), 28);
        let area = Rect::new(0, 0, 63, 28);
        let mut buf = Buffer::empty(area);
        render_full_logo(area, &mut buf, &Theme::tokyonight());
        let row: String = (0..63).map(|x| buf[(x, 1)].symbol()).collect();
        assert!(row.contains(".qq__             .__."), "{row}");
    }

    #[test]
    fn hero_box_always_uses_full_logo() {
        // The box renders the full logo regardless of height (it's laid out
        // beside the menu), and it's the large variant — never the small one.
        assert_eq!(
            full_logo_line_count_for(false),
            count_lines(&animated_logo_layout())
        );
        assert_eq!(
            full_logo_visual_width_for(false),
            visual_width(&animated_logo_layout())
        );
        assert!(full_logo_line_count_for(false) > count_lines(LOGO_SMALL));
        assert!(full_logo_visual_width_for(false) > visual_width(LOGO_SMALL));
    }

    #[test]
    fn full_logo_helpers_collapse_when_hidden() {
        assert_eq!(full_logo_line_count_for(true), 0);
        assert_eq!(full_logo_visual_width_for(true), 0);
    }

    #[test]
    fn compact_logo_line_count_matches_small_logo_when_visible() {
        // The minimal welcome card budgets exactly the small logo's rows
        if !logo_hidden() {
            assert_eq!(compact_logo_line_count(), count_lines(LOGO_SMALL));
            assert!(compact_logo_line_count() < count_lines(LOGO));
            assert!(compact_logo_line_count() > 0);
        } else {
            assert_eq!(compact_logo_line_count(), 0);
        }
    }

    #[test]
    fn full_wordmark_renders_without_the_legacy_lightning_art() {
        let area = Rect::new(0, 0, 48, 12);
        let mut buf = Buffer::empty(area);
        render_full_logo(area, &mut buf, &Theme::tokyonight());

        let mut rendered = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                rendered.push_str(buf[(x, y)].symbol());
            }
            rendered.push('\n');
        }

        assert!(rendered.contains(WORDMARK), "rendered:\n{rendered}");
        assert!(rendered.contains(TAGLINE), "rendered:\n{rendered}");
        assert!(!rendered.contains("/_"), "rendered:\n{rendered}");
    }

    #[test]
    fn coin_frames_are_stable_and_include_high_voltage() {
        let height = count_lines(COIN_FRAMES[0]);
        let max_width = visual_width(COIN_FRAMES[0]);
        for frame in COIN_FRAMES {
            assert_eq!(count_lines(frame), height);
            assert!(visual_width(frame) <= max_width);
        }
        assert!(COIN_FRAMES.iter().any(|frame| frame.contains('⚡')));
    }

    #[cfg(test)]
    #[test]
    fn shine_rests_dim_between_sweeps() {
        // During the rest phase the band is parked off-screen, so an interior glyph falls back to at most the gentle pulse, never full bright
        let op = shine_opacity(0.5, 6.0); // secs % 4.0 = 2.0, past SWEEP_FRAC, in the rest phase
        assert!(op < 0.2, "resting opacity {op} should stay dim");
    }
}
