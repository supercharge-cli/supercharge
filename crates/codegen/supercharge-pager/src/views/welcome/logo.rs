//! Supercharge wordmark for the welcome screen.
//!
//! This deliberately stays compact and text-first. A terminal welcome screen
//! should identify the product without consuming the workspace with ASCII art
//! or an idle animation.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::theme::Theme;

const WORDMARK: &str = "SUPERCHARGE";
const TAGLINE: &str = "AI coding workspace";
const LOGO: &str = "✦  SUPERCHARGE\nAI coding workspace\n";
const LOGO_SMALL: &str = "✦  SUPERCHARGE\n";

/// Height at or above which the small logo is shown (below it, no logo).
const SMALL_LOGO_MIN_HEIGHT: u16 = 22;
/// Height at or above which the full logo is shown.
const FULL_LOGO_MIN_HEIGHT: u16 = 26;

fn pick_logo(window_height: u16) -> Option<&'static str> {
    pick_logo_for(window_height, logo_hidden())
}

/// Pure tier selection so tests can drive the visibility flag directly.
fn pick_logo_for(window_height: u16, hidden: bool) -> Option<&'static str> {
    if hidden || window_height < SMALL_LOGO_MIN_HEIGHT {
        None
    } else if window_height < FULL_LOGO_MIN_HEIGHT {
        Some(LOGO_SMALL)
    } else {
        Some(LOGO)
    }
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

/// Compatibility hook for the welcome renderer's low-frequency animation
/// scheduler. The wordmark is intentionally static, so it never requests an
/// extra redraw while the user is deciding what to do.
pub fn shimmer_frame() -> u64 {
    0
}

fn render_into(area: Rect, buf: &mut Buffer, theme: &Theme, logo: &str) {
    let lines: Vec<&str> = non_empty_lines(logo).collect();
    let logo_lines: Vec<Line> = lines
        .iter()
        .map(|line| {
            if let Some(wordmark) = line.strip_prefix("✦  ")
                && wordmark == WORDMARK
            {
                Line::from(vec![
                    Span::styled(
                        "✦",
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

/// The hero box always shows the full logo: it is laid out beside the menu, so
/// it fits whenever the box does. These report and render that logo directly,
/// independent of the height-based [`pick_logo`] tiers used by the stacked
/// layout. When [`logo_hidden`], they report 0 and render nothing.
pub fn full_logo_line_count() -> u16 {
    full_logo_line_count_for(logo_hidden())
}

fn full_logo_line_count_for(hidden: bool) -> u16 {
    if hidden { 0 } else { count_lines(LOGO) }
}

pub fn full_logo_visual_width() -> u16 {
    full_logo_visual_width_for(logo_hidden())
}

fn full_logo_visual_width_for(hidden: bool) -> u16 {
    if hidden { 0 } else { visual_width(LOGO) }
}

pub fn render_full_logo(area: Rect, buf: &mut Buffer, theme: &Theme) {
    if !logo_hidden() {
        render_into(area, buf, theme, LOGO);
    }
}

/// Line count of the small logo used in minimal's committed welcome card.
pub fn compact_logo_line_count() -> u16 {
    if logo_hidden() {
        0
    } else {
        count_lines(LOGO_SMALL)
    }
}

/// Render the compact Supercharge wordmark (centered) into `area` for
/// minimal's welcome card.
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

    // The explicit hidden parameter remains covered for layout callers that
    // intentionally suppress the logo.
    #[test]
    fn logo_hidden_on_legacy_console_at_every_height() {
        for h in [0, SMALL_LOGO_MIN_HEIGHT, FULL_LOGO_MIN_HEIGHT, u16::MAX] {
            assert!(pick_logo_for(h, true).is_none(), "height {h}");
        }
    }

    #[test]
    fn hero_box_always_uses_full_logo() {
        // The box renders the full logo regardless of height (it's laid out
        // beside the menu), and it's the large variant — never the small one.
        assert_eq!(full_logo_line_count_for(false), count_lines(LOGO));
        assert_eq!(full_logo_visual_width_for(false), visual_width(LOGO));
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
        // The minimal welcome card budgets exactly the small logo's rows. When
        // the logo isn't hidden, the count equals the small art's line count and
        // is strictly shorter than the full logo.
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
        let area = Rect::new(0, 0, 48, 4);
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
    fn static_wordmark_does_not_schedule_idle_redraws() {
        assert_eq!(shimmer_frame(), 0);
    }
}
