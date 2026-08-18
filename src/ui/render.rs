use crate::preview::Line as PreviewLine;
use crate::ui::App;
use crate::ui::table::Row;
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, Padding, Paragraph, Wrap};
use std::time::SystemTime;

const QUEUE_CAP: usize = 18;
const STATUS_CAP: usize = 22;
const JOB_CAP: usize = 32;
const AGE_WIDTH: usize = 5;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [body, footer] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Min(24)]).areas(body);

    draw_list(frame, app, left);
    draw_preview(frame, app, right);
    frame.render_widget(footer_line(app, footer.width as usize), footer);

    if app.help {
        draw_overlay(frame, body, "Keys", help_lines(app), app.help_scroll);
    } else if let Some(prompt) = &app.prompt {
        let mut lines = vec![Line::from(prompt.question.clone()), Line::from("")];
        lines.extend(
            prompt
                .detail
                .iter()
                .map(|line| Line::from(Span::styled(line.clone(), app.theme.muted()))),
        );
        if !prompt.detail.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            "y to go ahead, anything else to cancel",
            app.theme.muted(),
        )));
        draw_overlay(frame, body, "Confirm", lines, 0);
    }
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let widths = Widths::of(&app.rows, area.width.saturating_sub(3) as usize);
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| ListItem::new(list_line(app, row, &widths, index == app.cursor)))
        .collect();

    let files = app.file_count();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(app.theme.border())
        .title(Span::styled(
            format!(" {} ", app.root_name()),
            app.theme.title(),
        ))
        .title_bottom(
            Line::from(Span::styled(
                format!(" {files} file{} ", if files == 1 { "" } else { "s" }),
                app.theme.muted(),
            ))
            .right_aligned(),
        );

    app.list_area = block.inner(area);
    app.list_state.select(Some(app.cursor));
    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(app.theme.selected()),
        area,
        &mut app.list_state,
    );
}

const MARKER: &str = "\u{2588}";

fn list_line<'a>(app: &App, row: &'a Row, widths: &Widths, selected: bool) -> Line<'a> {
    let mut spans = vec![match selected {
        true => Span::styled(MARKER, app.theme.marker()),
        false => Span::raw(" "),
    }];
    spans.extend(coloured_line(app, row, widths).spans);

    let line = Line::from(spans);
    match selected || matches!(row, Row::Columns) {
        true => line,
        false => line.style(app.theme.resting()),
    }
}

fn coloured_line<'a>(app: &App, row: &'a Row, widths: &Widths) -> Line<'a> {
    match row {
        Row::Columns => Line::from(Span::styled(
            format!(
                " {:queue$} {:status$} {:job$} {:>age$}",
                "QUEUE",
                "STATUS",
                "JOB",
                "AGE",
                queue = widths.queue,
                status = widths.status,
                job = widths.job,
                age = AGE_WIDTH
            ),
            app.theme.columns(),
        )),
        Row::Empty(queue) => Line::from(vec![
            Span::styled(
                format!(" {:width$} ", queue, width = widths.queue),
                app.theme.queue(),
            ),
            Span::styled("empty", app.theme.muted()),
        ]),
        Row::File(entry) => {
            let job = match &entry.detail {
                Some(detail) => format!("{} {}", entry.label, detail),
                None => entry.label.clone(),
            };
            Line::from(vec![
                Span::styled(
                    format!(
                        " {:width$} ",
                        truncated(&entry.queue, widths.queue),
                        width = widths.queue
                    ),
                    app.theme.queue(),
                ),
                Span::styled(
                    entry.status.clone(),
                    app.theme.status(app.color_of(&entry.status)),
                ),
                Span::styled(
                    entry
                        .badge
                        .as_ref()
                        .map(|badge| format!(" {badge}"))
                        .unwrap_or_default(),
                    app.theme.badge(),
                ),
                Span::raw(" ".repeat(status_padding(entry, widths.status))),
                Span::styled(
                    format!(
                        "{:width$} ",
                        truncated(&job, widths.job),
                        width = widths.job
                    ),
                    app.theme.job(),
                ),
                Span::styled(
                    format!(
                        "{:>width$}",
                        age(entry.modified, app.now),
                        width = AGE_WIDTH
                    ),
                    app.theme.muted(),
                ),
            ])
        }
    }
}

fn status_padding(entry: &crate::scan::Entry, width: usize) -> usize {
    let badge = entry
        .badge
        .as_ref()
        .map_or(0, |badge| badge.chars().count() + 1);
    width.saturating_sub(entry.status.chars().count() + badge) + 1
}

fn draw_preview(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(app.theme.border())
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" {} ", app.preview_title()),
            app.theme.title(),
        ));

    app.preview_area = block.inner(area);
    let lines: Vec<Line> = app
        .preview
        .iter()
        .map(|line| preview_line(app, line))
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((app.preview_scroll, 0)),
        area,
    );
}

fn preview_line<'a>(app: &App, line: &'a PreviewLine) -> Line<'a> {
    match line {
        PreviewLine::Blank => Line::from(""),
        PreviewLine::Text(text) => Line::from(text.as_str()),
        PreviewLine::Notice(text) => Line::from(Span::styled(text.as_str(), app.theme.notice())),
        PreviewLine::Field { label, value } => Line::from(vec![
            Span::styled(format!("{label:<9} "), app.theme.label()),
            Span::raw(value.as_str()),
        ]),
    }
}

fn footer_line(app: &App, width: usize) -> Line<'static> {
    if let Some(message) = &app.message {
        return Line::from(Span::styled(format!(" {message}"), app.theme.alert()));
    }

    let keys = &app.profile.keys;
    let head = format!(" {} move", pair(&keys.down, &keys.up));
    let tail = format!(
        "{} help  {} quit",
        first_of(&keys.help),
        first_of(&keys.quit)
    );

    let mut optional: Vec<String> = app
        .profile
        .action
        .iter()
        .map(|action| format!("{} {}", action.key, app.labelled(action)))
        .collect();
    optional.push(format!("{} rescan", first_of(&keys.rescan)));
    optional.push(format!(
        "{} sort:{}",
        first_of(&keys.sort),
        app.order.label()
    ));

    let mut used = head.chars().count() + 2 + tail.chars().count();
    let mut parts = vec![head];
    let mut hidden = false;

    for part in optional {
        let cost = part.chars().count() + 2;
        match used + cost <= width {
            true => {
                used += cost;
                parts.push(part);
            }
            false => hidden = true,
        }
    }
    if hidden && used + 3 <= width {
        parts.push("\u{2026}".to_string());
    }
    parts.push(tail);

    Line::from(Span::styled(parts.join("  "), app.theme.muted()))
}

fn first_of(bindings: &[crate::keys::Binding]) -> String {
    bindings
        .first()
        .map(|binding| binding.written())
        .unwrap_or_default()
}

fn pair(left: &[crate::keys::Binding], right: &[crate::keys::Binding]) -> String {
    format!("{}/{}", first_of(left), first_of(right))
}

fn help_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = app
        .profile
        .action
        .iter()
        .map(|action| key_line(app, &action.key, &app.labelled(action)))
        .collect();
    lines.extend(
        app.profile
            .keys
            .described()
            .into_iter()
            .map(|(keys, meaning)| key_line(app, &keys, meaning)),
    );
    lines
}

fn key_line(app: &App, key: &str, meaning: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<20}"), app.theme.label()),
        Span::raw(meaning.to_string()),
    ])
}

fn draw_overlay(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    scroll: u16,
) {
    let width = area
        .width
        .saturating_sub(area.width / 4)
        .max(30)
        .min(area.width);
    let inner = width.saturating_sub(4).max(1) as usize;
    let wrapped: usize = lines
        .iter()
        .map(|line| line.width().div_ceil(inner).max(1))
        .sum();
    let height = (wrapped as u16 + 2).min(area.height);

    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };

    let hidden = (wrapped as u16 + 2).saturating_sub(area.height);
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .padding(Padding::horizontal(1))
        .title(format!(" {title} "));
    if hidden > 0 {
        block = block
            .title_bottom(Line::from(format!(" {} more, j/k to scroll ", hidden)).right_aligned());
    }

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        popup,
    );
}

struct Widths {
    queue: usize,
    status: usize,
    job: usize,
}

impl Widths {
    fn of(rows: &[Row], available: usize) -> Self {
        let mut widths = Self {
            queue: 5,
            status: 6,
            job: 3,
        };
        for entry in rows.iter().filter_map(Row::entry) {
            widths.queue = widths.queue.max(entry.queue.len());
            let badge = entry.badge.as_ref().map_or(0, |badge| badge.len() + 1);
            widths.status = widths.status.max(entry.status.len() + badge);
            let detail = entry.detail.as_ref().map_or(0, |detail| detail.len() + 1);
            widths.job = widths.job.max(entry.label.len() + detail);
        }
        widths.queue = widths.queue.min(QUEUE_CAP);
        widths.status = widths.status.min(STATUS_CAP);

        let spent = 5 + widths.queue + widths.status + AGE_WIDTH;
        widths.job = available.saturating_sub(spent).clamp(
            3,
            widths.job.min(JOB_CAP).max(available.saturating_sub(spent)),
        );
        widths
    }
}

fn truncated(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

pub fn age(modified: SystemTime, now: SystemTime) -> String {
    let seconds = now
        .duration_since(modified)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);

    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ago(seconds: u64) -> String {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        age(now - Duration::from_secs(seconds), now)
    }

    #[test]
    fn shortens_an_age_to_its_coarsest_useful_unit() {
        assert_eq!(ago(5), "5s");
        assert_eq!(ago(90), "1m");
        assert_eq!(ago(3600 * 5), "5h");
        assert_eq!(ago(86400 * 10), "10d");
    }

    #[test]
    fn treats_a_file_from_the_future_as_brand_new() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(age(now + Duration::from_secs(500), now), "0s");
    }

    #[test]
    fn truncates_with_an_ellipsis_only_when_it_has_to() {
        assert_eq!(truncated("short", 10), "short");
        assert_eq!(truncated("exactlyten", 10), "exactlyten");
        assert_eq!(truncated("far too long to fit", 10), "far too l\u{2026}");
    }
}
