use serde_json::json;
use tmux_aside::i18n::Language;
use tmux_aside::protocol::Tone;
use tmux_aside_usage::{
    Window, claude_windows, codex_windows, format_remaining, rows, statusline_command,
    wrapped_statusline,
};

const NOW: u64 = 1_793_000_000;

#[test]
fn claude_windows_come_from_the_statusline_rate_limits() {
    let rate_limits = json!({
        "five_hour": {"used_percentage": 34.5, "resets_at": NOW + 3_600},
        "seven_day": {"used_percentage": 12, "resets_at": NOW + 86_400 * 3}
    });

    assert_eq!(
        claude_windows(&rate_limits, NOW, Language::En),
        [
            Window {
                label: "5 hours".to_string(),
                used_percent: 34.5,
                resets_at: NOW + 3_600
            },
            Window {
                label: "Weekly".to_string(),
                used_percent: 12.0,
                resets_at: NOW + 86_400 * 3
            },
        ]
    );
}

#[test]
fn claude_windows_that_already_reset_or_are_missing_are_skipped() {
    let rate_limits = json!({
        "five_hour": {"used_percentage": 90, "resets_at": NOW - 1},
    });

    assert!(claude_windows(&rate_limits, NOW, Language::En).is_empty());
}

#[test]
fn codex_windows_are_named_by_their_duration_not_their_slot() {
    let result = json!({
        "rateLimits": {
            "primary": {"usedPercent": 0, "windowDurationMins": 10080, "resetsAt": NOW + 600},
            "secondary": null
        }
    });

    assert_eq!(
        codex_windows(&result, NOW, Language::En),
        [Window {
            label: "Weekly".to_string(),
            used_percent: 0.0,
            resets_at: NOW + 600
        }]
    );

    let both = json!({
        "rateLimits": {
            "primary": {"usedPercent": 50, "windowDurationMins": 300, "resetsAt": NOW + 60},
            "secondary": {"usedPercent": 5, "windowDurationMins": 10080, "resetsAt": NOW + 600}
        }
    });
    let labels: Vec<String> = codex_windows(&both, NOW, Language::En)
        .into_iter()
        .map(|window| window.label)
        .collect();
    assert_eq!(labels, ["5 hours", "Weekly"]);
}

#[test]
fn rows_group_windows_by_tool_with_percent_and_time_to_reset() {
    let windows = vec![(
        "Claude".to_string(),
        vec![Window {
            label: "5 hours".to_string(),
            used_percent: 34.4,
            resets_at: NOW + 2 * 3_600 + 10 * 60,
        }],
    )];

    let rows = rows(&windows, NOW, Language::En);

    assert_eq!(rows[0].group.as_deref(), Some("Claude"));
    assert_eq!(rows[0].text, "5 hours");
    assert_eq!(rows[0].detail.as_deref(), Some("34% · 2h10m"));
}

#[test]
fn rows_carry_a_meter_and_line_up_across_tools() {
    let window = |label: &str, used_percent, hours: u64| Window {
        label: label.to_string(),
        used_percent,
        resets_at: NOW + hours * 3_600,
    };
    let tools = vec![
        (
            "Claude".to_string(),
            vec![window("5 hours", 38.4, 1), window("Weekly", 58.0, 89)],
        ),
        ("Codex".to_string(), vec![window("Weekly", 0.0, 168)]),
    ];

    let rows = rows(&tools, NOW, Language::En);

    let summary: Vec<(&str, &str, Option<u8>)> = rows
        .iter()
        .map(|row| (row.text.as_str(), row.detail.as_deref().unwrap(), row.meter))
        .collect();
    assert_eq!(
        summary,
        [
            ("5 hours", " 38% · 1h0m", Some(38)),
            ("Weekly ", "58% · 3d17h", Some(58)),
            ("Weekly ", "  0% · 7d0h", Some(0)),
        ]
    );
}

#[test]
fn usage_is_colored_by_how_close_it_is_to_the_limit() {
    let tone = |used_percent| {
        let windows = vec![(
            "Codex".to_string(),
            vec![Window {
                label: "Weekly".to_string(),
                used_percent,
                resets_at: NOW + 60,
            }],
        )];
        rows(&windows, NOW, Language::En).remove(0).meter_tone
    };

    assert_eq!(tone(10.0), Tone::Success);
    assert_eq!(tone(70.0), Tone::Warning);
    assert_eq!(tone(90.0), Tone::Danger);
}

#[test]
fn tools_without_data_say_so() {
    let rows = rows(&[("Codex".to_string(), vec![])], NOW, Language::En);

    assert_eq!(rows[0].text, "no data");
    assert_eq!(rows[0].tone, Tone::Muted);
}

#[test]
fn remaining_time_uses_minutes_hours_and_days() {
    assert_eq!(format_remaining(45 * 60), "45m");
    assert_eq!(format_remaining(2 * 3_600 + 10 * 60), "2h10m");
    assert_eq!(format_remaining(3 * 86_400 + 5 * 3_600), "3d5h");
}

#[test]
fn the_statusline_wrapper_keeps_the_original_command_recoverable() {
    let original = "POWERLINE_TIME=$(date '+%d/%m %H:%M:%S') node /x/claude-powerline";

    let wrapped = statusline_command("/bin/tmux-aside-usage", original);

    assert_eq!(wrapped_statusline(&wrapped).as_deref(), Some(original));
    assert_eq!(wrapped_statusline(original), None);
}

#[test]
fn only_the_meter_is_colored_and_the_detail_stays_muted() {
    let windows = vec![(
        "Claude".to_string(),
        vec![Window {
            label: "Weekly".to_string(),
            used_percent: 95.0,
            resets_at: NOW + 60,
        }],
    )];

    let row = rows(&windows, NOW, Language::En).remove(0);

    assert_eq!(
        (row.meter_tone, row.detail_tone),
        (Tone::Danger, Tone::Muted)
    );
}

#[test]
fn windows_and_missing_data_are_shown_in_spanish() {
    let rate_limits = json!({
        "five_hour": {"used_percentage": 10, "resets_at": NOW + 60},
        "seven_day": {"used_percentage": 20, "resets_at": NOW + 600}
    });
    let codex = json!({
        "rateLimits": {
            "primary": {"usedPercent": 5, "windowDurationMins": 4320, "resetsAt": NOW + 60}
        }
    });
    let tools = vec![
        (
            "Claude".to_string(),
            claude_windows(&rate_limits, NOW, Language::Es),
        ),
        (
            "Codex".to_string(),
            codex_windows(&codex, NOW, Language::Es),
        ),
        ("Agy".to_string(), vec![]),
    ];

    let texts: Vec<String> = rows(&tools, NOW, Language::Es)
        .into_iter()
        .map(|row| row.text)
        .collect();

    assert_eq!(texts, ["5 horas", "Semanal", "3 días ", "sin datos"]);
}
