use crate::protocol::{Align, Row, Tone};
use crate::view::{Cursor, Density, Pin, Section, Target, View};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Header,
    Group,
    Row,
    Separator,
    Blank,
}

const MIN_METER: usize = 3;
const SPINNER_ICON: &str = "spinner";
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A run of text drawn with one tone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub tone: Tone,
}

/// One screen line of the sidebar, already padded or truncated to the sidebar width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutLine {
    pub kind: LineKind,
    pub selected: bool,
    /// Part of a row that belongs to the tmux window this sidebar is shown in.
    pub current: bool,
    /// Header, group or row the line belongs to, as clicking it selects.
    pub target: Option<Cursor>,
    pub spans: Vec<Span>,
}

/// Size of the sidebar and per-client details that change how rows are drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutOptions {
    pub width: usize,
    pub height: usize,
    /// Frame of animated icons.
    pub spinner_frame: usize,
    /// tmux window the sidebar is shown in; its rows are marked current.
    pub current_window: Option<String>,
}

impl LayoutLine {
    pub fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }
}

/// Lays out `view` on a `width` x `height` sidebar: top sections stacked from the first line,
/// bottom-pinned sections on the last lines below a separator, every comfortable section but
/// the last followed by a blank line. When space runs out the bottom sections are kept and the top
/// ones are cut, scrolled so the selected line stays visible.
pub fn layout(view: &View, options: &LayoutOptions) -> Vec<LayoutLine> {
    let (width, height) = (options.width, options.height);
    let pinned = |pin: Pin| -> Vec<LayoutLine> {
        let mut lines = Vec::new();
        for (index, section) in view.sections.iter().enumerate() {
            if section.pin != pin {
                continue;
            }
            let section_lines = section_lines(view, index, section, options);
            let separated = section.header || lines.is_empty() && pin == Pin::Top;
            if section_lines.is_empty() {
                continue;
            }
            if !separated {
                lines.push(plain(LineKind::Separator, "─".repeat(width)));
            }
            lines.extend(section_lines);
            if section.density == Density::Comfortable {
                lines.push(plain(LineKind::Blank, " ".repeat(width)));
            }
        }
        lines
    };
    let mut bottom = pinned(Pin::Bottom);
    if bottom
        .last()
        .is_some_and(|line| line.kind == LineKind::Blank)
    {
        bottom.pop();
    }
    bottom.truncate(height);
    let mut lines = pinned(Pin::Top);
    let available = height - bottom.len();
    if let Some(last) = lines.iter().rposition(|line| line.selected)
        && last >= available
    {
        lines.drain(..last + 1 - available);
    }
    lines.truncate(available);
    while lines.len() + bottom.len() < height {
        lines.push(plain(LineKind::Blank, " ".repeat(width)));
    }
    lines.extend(bottom);
    lines
}

/// The help shown over the sidebar: the sidebar's own keys, then the actions of the selected
/// row with their labels.
pub fn help(view: &View, width: usize) -> Vec<LayoutLine> {
    let title_line = |title: &str| {
        let section = Section {
            counts: false,
            ..Section::new(title, vec![], None)
        };
        line(LineKind::Header, false, header(&section, width))
    };
    let entry = |key: &str, label: &str| {
        let mut row = Row::with_detail(key, label);
        row.tone = Tone::Accent;
        row.detail_tone = Tone::Plain;
        line(LineKind::Row, false, compose("", &row, width, 0))
    };
    let messages = view.language.messages();
    let keys = [
        ("j k", messages.move_selection),
        ("h l", messages.fold_unfold),
        ("space", messages.fold_or_unfold),
        ("enter", messages.run_or_show),
        ("o", messages.run_action),
        ("?", messages.close_help),
    ];
    let mut lines = vec![title_line(messages.keys)];
    lines.extend(keys.iter().map(|(key, label)| entry(key, label)));
    let actions: Vec<_> = view
        .selected_row()
        .map(|row| {
            row.actions
                .iter()
                .filter(|action| action.key != "enter")
                .collect()
        })
        .unwrap_or_default();
    if !actions.is_empty() {
        lines.push(plain(LineKind::Blank, " ".repeat(width)));
        lines.push(title_line(messages.this_row));
        lines.extend(
            actions
                .iter()
                .map(|action| entry(&action.key, action.label.as_deref().unwrap_or(""))),
        );
    }
    lines
}

/// Whether any row of `view` shows an animated icon.
pub fn has_spinner(view: &View) -> bool {
    view.sections
        .iter()
        .flat_map(|section| &section.rows)
        .any(|row| {
            row.icon.as_deref() == Some(SPINNER_ICON) || row.detail.as_deref() == Some(SPINNER_ICON)
        })
}

fn section_lines(
    view: &View,
    index: usize,
    section: &Section,
    options: &LayoutOptions,
) -> Vec<LayoutLine> {
    let (width, frame) = (options.width, options.spinner_frame);
    let cursor = |target: Target| Cursor {
        section: index,
        target,
    };
    let is_selected = |target: Target| view.selected.as_ref() == Some(&cursor(target));
    let mut lines = Vec::new();
    if section.header {
        lines.push(LayoutLine {
            target: Some(cursor(Target::Header)),
            ..line(
                LineKind::Header,
                is_selected(Target::Header),
                header(section, width),
            )
        });
        if section.collapsed {
            return lines;
        }
    }
    let indent = "";
    let mut current_group = None;
    let mut previous_bar = None;
    for row in section.visible_rows() {
        let data = &section.rows[row];
        let group = data.group.as_deref();
        let collapsed = group.is_some_and(|group| section.is_group_collapsed(group));
        let starts_group = group != current_group && group.is_some();
        if group != current_group {
            current_group = group;
            if let Some(group) = group {
                let spaced = section.group_gap || section.density == Density::Comfortable;
                if spaced && lines.len() > usize::from(section.header) {
                    lines.push(line(
                        LineKind::Blank,
                        false,
                        vec![span(" ".repeat(width), Tone::Plain)],
                    ));
                }
                let size = section.counts.then(|| section.group_size(group));
                lines.push(LayoutLine {
                    target: Some(cursor(Target::Group(group.to_string()))),
                    ..line(
                        LineKind::Group,
                        is_selected(Target::Group(group.to_string())),
                        compose(indent, &counted(collapsed, group, size), width, frame),
                    )
                });
            }
        }
        if collapsed {
            continue;
        }
        let selected = is_selected(Target::Row(row));
        let current = data.window.is_some() && data.window == options.current_window;
        let gutter = match (data.bar_tone, group) {
            (Tone::Plain, Some(_)) => format!("{indent}  "),
            _ => indent.to_string(),
        };
        let bar = (data.bar_tone != Tone::Plain).then(|| span("▌".to_string(), data.bar_tone));
        if section.density == Density::Comfortable && !starts_group && previous_bar.is_some() {
            let mut spans = vec![span(gutter.clone(), Tone::Plain)];
            if bar.is_some() {
                let tone = match previous_bar {
                    Some(tone) if tone == data.bar_tone => tone,
                    _ => Tone::Muted,
                };
                spans.push(span("▌".to_string(), tone));
            }
            let used: usize = spans.iter().map(|span| columns(&span.text)).sum();
            spans.push(span(" ".repeat(width.saturating_sub(used)), Tone::Plain));
            lines.push(line(LineKind::Blank, false, spans));
        }
        previous_bar = Some(data.bar_tone);
        let mut first = compose(&gutter, data, width.saturating_sub(bar_width(&bar)), frame);
        insert_bar(&mut first, &bar);
        lines.push(LayoutLine {
            kind: LineKind::Row,
            selected,
            current,
            target: Some(cursor(Target::Row(row))),
            spans: first,
        });
        let icon_width = data.icon.as_ref().map_or(0, |_| 2);
        for extra in &data.lines {
            let mut extra_row = Row::new(extra.clone());
            extra_row.tone = Tone::Muted;
            let mut spans = compose(
                &gutter,
                &extra_row,
                width.saturating_sub(bar_width(&bar) + icon_width),
                frame,
            );
            spans.insert(1, span(" ".repeat(icon_width), Tone::Plain));
            insert_bar(&mut spans, &bar);
            lines.push(LayoutLine {
                kind: LineKind::Row,
                selected,
                current,
                target: Some(cursor(Target::Row(row))),
                spans,
            });
        }
    }
    lines
}

/// The section title centered on a separator line, with its row count when shown and a
/// leading `▸` and its summary when the section is collapsed, cut to 80% of the width.
fn header(section: &Section, width: usize) -> Vec<Span> {
    let (arrow, summary) = match (section.collapsed, &section.summary) {
        (true, Some(summary)) => ("▸ ", format!(" · {summary}")),
        (true, None) => ("▸ ", String::new()),
        (false, _) => ("", String::new()),
    };
    let count = match section.counts {
        true => format!(" · {}", section.rows.len()),
        false => String::new(),
    };
    let title = truncate(
        &format!("{arrow}{}{summary}{count}", section.title),
        (width * 8 / 10).min(width.saturating_sub(4)),
    );
    let title = format!(" {title} ");
    let fill = width.saturating_sub(columns(&title));
    let left = fill / 2;
    vec![
        span("─".repeat(left), Tone::Muted),
        span(title, Tone::Muted),
        span("─".repeat(fill - left), Tone::Muted),
    ]
}

fn counted(collapsed: bool, title: &str, count: Option<usize>) -> Row {
    let arrow = if collapsed { "▸ " } else { "" };
    let mut row = Row::new(format!("{arrow}{title}"));
    row.tone = Tone::Muted;
    row.detail = count.map(|count| count.to_string());
    row.detail_tone = Tone::Muted;
    row
}

fn bar_width(bar: &Option<Span>) -> usize {
    bar.as_ref().map_or(0, |_| 2)
}

fn insert_bar(spans: &mut Vec<Span>, bar: &Option<Span>) {
    if let Some(bar) = bar {
        spans.insert(1, span(" ".to_string(), Tone::Plain));
        spans.insert(1, bar.clone());
    }
}

fn line(kind: LineKind, selected: bool, spans: Vec<Span>) -> LayoutLine {
    LayoutLine {
        kind,
        selected,
        current: false,
        target: None,
        spans,
    }
}

fn compose(indent: &str, row: &Row, width: usize, spinner_frame: usize) -> Vec<Span> {
    if row.align == Align::Center {
        let text = truncate(&row.text, width);
        let left = (width - columns(&text)) / 2;
        let right = width - left - columns(&text);
        return vec![
            span(" ".repeat(left), Tone::Plain),
            span(text, row.tone),
            span(" ".repeat(right), Tone::Plain),
        ];
    }
    let mut spans = vec![span(indent.to_string(), Tone::Plain)];
    let mut used = columns(indent);
    if let Some(icon) = &row.icon {
        let glyph = animated(icon, spinner_frame);
        used += columns(&glyph) + 1;
        spans.push(span(glyph, row.icon_tone));
        spans.push(span(" ".to_string(), Tone::Plain));
    }
    let detail = animated(row.detail.as_deref().unwrap_or(""), spinner_frame);
    let detail = detail.as_str();
    let detail_width = columns(detail);
    let gap = usize::from(detail_width > 0);
    let text = truncate(&row.text, width.saturating_sub(used + detail_width + gap));
    let padding = width.saturating_sub(used + columns(&text) + detail_width);
    spans.extend(diff_colored(&text, row.tone));
    match row.meter {
        Some(percent) if padding >= MIN_METER + 2 => {
            let size = padding - 2;
            let filled = (usize::from(percent.min(100)) * size + 50) / 100;
            spans.push(span(" ".to_string(), Tone::Plain));
            spans.push(span("━".repeat(filled), row.meter_tone));
            spans.push(span("─".repeat(size - filled), Tone::Muted));
            spans.push(span(" ".to_string(), Tone::Plain));
        }
        _ => spans.push(span(" ".repeat(padding), Tone::Plain)),
    }
    spans.push(span(detail.to_string(), row.detail_tone));
    spans
}

/// `text` in `tone`, except `+N -M` diff stats, drawn with `+N` in success and `-M` in danger.
fn diff_colored(text: &str, tone: Tone) -> Vec<Span> {
    let words: Vec<&str> = text.split(' ').collect();
    let is_count = |word: &str, sign: char| {
        word.strip_prefix(sign)
            .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
    };
    let mut spans: Vec<Span> = Vec::new();
    let mut plain = String::new();
    let mut index = 0;
    while index < words.len() {
        if index + 1 < words.len() && is_count(words[index], '+') && is_count(words[index + 1], '-')
        {
            if !plain.is_empty() {
                spans.push(span(std::mem::take(&mut plain), tone));
            }
            spans.push(span(words[index].to_string(), Tone::Success));
            spans.push(span(" ".to_string(), tone));
            spans.push(span(words[index + 1].to_string(), Tone::Danger));
            index += 2;
        } else {
            plain.push_str(words[index]);
            index += 1;
        }
        if index < words.len() {
            plain.push(' ');
        }
    }
    if !plain.is_empty() || spans.is_empty() {
        spans.push(span(plain, tone));
    }
    spans
}

fn animated(value: &str, spinner_frame: usize) -> String {
    match value {
        SPINNER_ICON => SPINNER[spinner_frame % SPINNER.len()].to_string(),
        _ => value.to_string(),
    }
}

fn truncate(text: &str, width: usize) -> String {
    if columns(text) <= width {
        return text.to_string();
    }
    let mut kept = String::new();
    let mut used = 0;
    for c in text.chars() {
        let size = c.width().unwrap_or(0);
        if used + size + 1 > width {
            break;
        }
        kept.push(c);
        used += size;
    }
    format!("{kept}…")
}

/// Terminal columns `text` takes, counting wide characters such as emoji as two.
fn columns(text: &str) -> usize {
    text.width()
}

fn span(text: String, tone: Tone) -> Span {
    Span { text, tone }
}

fn plain(kind: LineKind, text: String) -> LayoutLine {
    line(kind, false, vec![span(text, Tone::Plain)])
}
