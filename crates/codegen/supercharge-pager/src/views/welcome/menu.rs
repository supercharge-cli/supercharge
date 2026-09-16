use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::theme::Theme;

fn cols(text: &str) -> u16 {
    unicode_width::UnicodeWidthStr::width(text) as u16
}

const ROW_MARKER: &str = "› ";
const ROW_MARKER_WIDTH: u16 = 2;
const SHORTCUT_GAP: u16 = 2;

/// Render the welcome menu rows as `label … shortcut`, padded within each row.
/// Returns the Rect for each item row (for hit-testing clicks and hover).
pub fn render_menu(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    items: &[(&str, &str)],
    selected: Option<usize>,
    mouse_pos: Option<(u16, u16)>,
    min_width_hint: u16,
) -> Vec<Rect> {
    let label_style = Style::default().fg(theme.text_secondary);
    let label_selected_style = Style::default()
        .fg(theme.text_primary)
        .bg(theme.bg_highlight)
        .add_modifier(Modifier::BOLD);
    let key_style = Style::default().fg(theme.accent_user);
    let key_selected_style = Style::default()
        .fg(theme.gray_bright)
        .bg(theme.bg_highlight);

    // Width: label + gap + key
    // Keep a 4-col gap between label and key for readability
    let content_min: u16 = items
        .iter()
        .map(|(key, label)| {
            ROW_MARKER_WIDTH
                + cols(label)
                + if key.is_empty() {
                    0
                } else {
                    SHORTCUT_GAP + cols(key)
                }
        })
        .max()
        .unwrap_or(0);
    let menu_width = area.width.min(30.max(content_min).max(min_width_hint));

    let [_, menu_centered, _] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(menu_width),
        Constraint::Min(0),
    ])
    .flex(Flex::Center)
    .areas(area);

    let mut rects = Vec::with_capacity(items.len());
    let mut y = menu_centered.y;
    for (i, (key, label)) in items.iter().enumerate() {
        if y >= menu_centered.y + menu_centered.height {
            break;
        }

        let is_selected = selected == Some(i);
        let key_width = cols(key);
        let label_width = menu_centered.width.saturating_sub(
            ROW_MARKER_WIDTH
                + if key.is_empty() {
                    0
                } else {
                    SHORTCUT_GAP + key_width
                },
        );
        // The shortcut sits at the right edge; leave a modest, fixed gap so
        // actions remain easy to scan without looking like a split table.
        let label = crate::render::line_utils::truncate_str(label, label_width as usize);
        let label_len = cols(&label);

        let row_rect = Rect {
            x: menu_centered.x,
            y,
            width: menu_centered.width,
            height: 1,
        };
        rects.push(row_rect);

        // Fill row background when selected/hovered
        if is_selected {
            let hover_bg = Style::default().bg(theme.bg_highlight);
            for x in menu_centered.x..menu_centered.x + menu_centered.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_style(hover_bg);
                }
            }
        }

        let marker_style = if is_selected {
            Style::default()
                .fg(theme.accent_user)
                .bg(theme.bg_highlight)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.accent_assistant)
        };
        buf.set_span(
            menu_centered.x,
            y,
            &Span::styled(ROW_MARKER, marker_style),
            ROW_MARKER_WIDTH,
        );

        // Label, just after the action marker.
        let lstyle = if is_selected {
            label_selected_style
        } else {
            label_style
        };
        buf.set_span(
            menu_centered.x + ROW_MARKER_WIDTH,
            y,
            &Span::styled(&*label, lstyle),
            label_len,
        );

        if !key.is_empty() {
            // Key shortcut flush with the right edge of the compact palette.
            let kstyle = if is_selected {
                key_selected_style
            } else {
                key_style
            };
            buf.set_span(
                menu_centered.x + menu_centered.width - key_width,
                y,
                &Span::styled(*key, kstyle),
                key_width,
            );
        }

        // [x] dismiss affordance restyling (for the import row)
        if let Some(x_offset) = key.rfind("[x]") {
            let key_x_start = menu_centered.x + menu_centered.width - key_width;
            let dismiss_start = key_x_start + x_offset as u16;
            let dismiss_end = dismiss_start + 3;
            let mouse_on_dismiss = mouse_pos
                .is_some_and(|(mx, my)| my == y && mx >= dismiss_start && mx < dismiss_end);
            let dismiss_color = if mouse_on_dismiss {
                theme.text_primary
            } else {
                theme.gray_bright
            };
            let dismiss_style = if is_selected {
                Style::default()
                    .fg(dismiss_color)
                    .bg(theme.bg_highlight)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(dismiss_color)
                    .add_modifier(Modifier::BOLD)
            };
            for (offset, ch) in "[x]".chars().enumerate() {
                let col = dismiss_start + offset as u16;
                if let Some(cell) = buf.cell_mut((col, y)) {
                    cell.set_char(ch);
                    cell.set_style(dismiss_style);
                }
            }
        }

        // Terminal theme (Reset band slots): the selection/hover cue is
        // reverse video; the row is all default-fg text so it inverts
        // uniformly. RGB themes keep their bg_highlight band.
        if is_selected && theme.is_bandless() {
            buf.set_style(row_rect, Style::default().add_modifier(Modifier::REVERSED));
        }

        y += 1;
    }

    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_palette_is_compact_and_keeps_shortcuts_visible() {
        let area = Rect::new(0, 0, 100, 3);
        let mut buf = Buffer::empty(area);
        let items = [
            ("ctrl+w", "New worktree"),
            ("ctrl+s", "Resume session"),
            ("", "Changelog"),
        ];

        let rows = render_menu(area, &mut buf, &Theme::tokyonight(), &items, None, None, 44);

        assert_eq!(rows.len(), items.len());
        assert_eq!(rows[0].width, 44);
        let first_row: String = (rows[0].x..rows[0].x + rows[0].width)
            .flat_map(|x| buf[(x, rows[0].y)].symbol().chars())
            .collect();
        assert!(first_row.contains("› New worktree"), "row: {first_row:?}");
        assert!(first_row.contains("ctrl+w"), "row: {first_row:?}");
    }
}
