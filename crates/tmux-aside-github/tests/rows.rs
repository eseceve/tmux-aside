use jiff::Timestamp;
use tmux_aside::protocol::Tone;
use tmux_aside_github::{Part, RowOptions, State, format_age, merge_info, parse_search, rows};

fn search(nodes: &str) -> String {
    format!(r#"{{"data": {{"search": {{"issueCount": 0, "nodes": [{nodes}]}}}}}}"#)
}

fn node(repo: &str, number: u64, draft: bool, review: &str, checks: &str, updated: &str) -> String {
    let review = if review.is_empty() {
        "null".to_string()
    } else {
        format!(r#""{review}""#)
    };
    let rollup = if checks.is_empty() {
        "null".to_string()
    } else {
        format!(r#"{{"state": "{checks}"}}"#)
    };
    format!(
        r#"{{"number": {number}, "title": "Title {number}", "url": "https://github.com/org/{repo}/pull/{number}",
            "isDraft": {draft}, "reviewDecision": {review}, "createdAt": "2026-10-18T10:00:00Z",
            "author": {{"login": "dev"}}, "updatedAt": "{updated}", "mergeable": "MERGEABLE",
            "reviewThreads": {{"nodes": [{{"isResolved": false}}, {{"isResolved": true}}]}},
            "latestOpinionatedReviews": {{"nodes": [{{"state": "APPROVED"}}, {{"state": "COMMENTED"}}]}},
            "baseRef": {{"branchProtectionRule": {{"requiredApprovingReviewCount": 2}}}},
            "repository": {{"name": "{repo}", "nameWithOwner": "org/{repo}"}},
            "commits": {{"nodes": [{{"commit": {{"statusCheckRollup": {rollup}}}}}]}}}}"#
    )
}

#[test]
fn each_pull_request_is_a_row_grouped_by_repository_with_its_number_on_the_right() {
    let output = search(&node(
        "shop",
        345,
        false,
        "APPROVED",
        "SUCCESS",
        "2026-10-20T10:00:00Z",
    ));

    let rows = rows(
        &parse_search(&output).unwrap(),
        &RowOptions {
            colored: vec![Part::Icon],
            ..Default::default()
        },
    );

    let row = &rows[0];
    assert_eq!(row.text, "Title 345");
    assert_eq!(row.group.as_deref(), Some("shop"));
    assert_eq!(row.detail.as_deref(), Some("#345"));
    assert_eq!(
        row.action("enter"),
        Some(
            &[
                "gh".to_string(),
                "pr".to_string(),
                "view".to_string(),
                "--web".to_string(),
                "https://github.com/org/shop/pull/345".to_string()
            ][..]
        )
    );
}

fn pull_request(draft: bool, review: &str, checks: &str) -> tmux_aside_github::PullRequest {
    let output = search(&node(
        "api",
        1,
        draft,
        review,
        checks,
        "2026-10-20T10:00:00Z",
    ));
    parse_search(&output).unwrap().remove(0)
}

#[test]
fn the_state_puts_failing_checks_before_reviews_and_drafts_before_everything() {
    let state = |draft, review, checks| pull_request(draft, review, checks).state();

    assert_eq!(state(true, "CHANGES_REQUESTED", "FAILURE"), State::Draft);
    assert_eq!(state(false, "APPROVED", "FAILURE"), State::Failing);
    assert_eq!(state(false, "", "ERROR"), State::Failing);
    assert_eq!(
        state(false, "CHANGES_REQUESTED", "PENDING"),
        State::ChangesRequested
    );
    assert_eq!(state(false, "APPROVED", "PENDING"), State::Pending);
    assert_eq!(state(false, "", "EXPECTED"), State::Pending);
    assert_eq!(state(false, "APPROVED", "SUCCESS"), State::Ready);
    assert_eq!(state(false, "APPROVED", ""), State::Ready);
    assert_eq!(state(false, "REVIEW_REQUIRED", "SUCCESS"), State::Review);
    assert_eq!(state(false, "", ""), State::Review);
}

#[test]
fn colored_parts_take_the_state_tone() {
    let row = |colored: &[Part]| {
        rows(
            &[pull_request(false, "", "FAILURE")],
            &RowOptions {
                colored: colored.to_vec(),
                ..Default::default()
            },
        )
        .remove(0)
    };

    let icon = row(&[Part::Icon]);
    assert_eq!(icon.icon.as_deref(), Some("×"));
    assert_eq!(icon.icon_tone, Tone::Danger);
    assert_eq!((icon.tone, icon.detail_tone), (Tone::Plain, Tone::Muted));

    let number = row(&[Part::Number]);
    assert_eq!(number.icon, None);
    assert_eq!(
        (number.tone, number.detail_tone),
        (Tone::Plain, Tone::Danger)
    );

    let title = row(&[Part::Title]);
    assert_eq!((title.tone, title.detail_tone), (Tone::Danger, Tone::Muted));
}

#[test]
fn each_state_has_its_own_icon_and_tone() {
    let icon = |draft, review, checks| {
        let row = rows(
            &[pull_request(draft, review, checks)],
            &RowOptions {
                colored: vec![Part::Icon],
                ..Default::default()
            },
        )
        .remove(0);
        (row.icon.unwrap(), row.icon_tone)
    };

    assert_eq!(icon(false, "", "FAILURE"), ("×".to_string(), Tone::Danger));
    assert_eq!(
        icon(false, "CHANGES_REQUESTED", ""),
        ("◆".to_string(), Tone::Warning)
    );
    assert_eq!(icon(false, "", "PENDING"), ("◷".to_string(), Tone::Accent));
    assert_eq!(
        icon(false, "APPROVED", ""),
        ("✓".to_string(), Tone::Success)
    );
    assert_eq!(icon(false, "", ""), ("○".to_string(), Tone::Muted));
    assert_eq!(icon(true, "", ""), ("◌".to_string(), Tone::Muted));
}

#[test]
fn every_row_draws_a_muted_bar_along_its_repository() {
    let rows = rows(
        &[pull_request(false, "APPROVED", "")],
        &RowOptions {
            colored: vec![Part::Number],
            ..Default::default()
        },
    );

    assert_eq!(rows[0].bar_tone, Tone::Muted);
}

#[test]
fn repositories_are_alphabetical_and_recently_updated_pull_requests_come_first() {
    let output = search(
        &[
            node("web", 1, false, "", "", "2026-10-21T00:00:00Z"),
            node("api", 2, false, "", "", "2026-10-19T00:00:00Z"),
            node("api", 3, false, "", "", "2026-10-22T00:00:00Z"),
        ]
        .join(","),
    );

    let order: Vec<String> = rows(
        &parse_search(&output).unwrap(),
        &RowOptions {
            colored: vec![],
            ..Default::default()
        },
    )
    .into_iter()
    .filter_map(|row| row.detail)
    .collect();

    assert_eq!(order, ["#3", "#2", "#1"]);
}

#[test]
fn search_results_that_are_not_pull_requests_are_skipped() {
    let output = search(&format!(
        "{{}},{}",
        node("api", 9, false, "", "", "2026-10-20T10:00:00Z")
    ));

    assert_eq!(parse_search(&output).unwrap().len(), 1);
}

#[test]
fn graphql_errors_are_reported() {
    let error = parse_search(r#"{"errors": [{"message": "Bad credentials"}]}"#)
        .unwrap_err()
        .to_string();

    assert!(error.contains("Bad credentials"), "{error}");
}

#[test]
fn bylines_show_the_author_and_the_age_of_the_pull_request() {
    let now: Timestamp = "2026-10-20T10:00:00Z".parse().unwrap();

    let rows = rows(
        &[pull_request(false, "", "")],
        &RowOptions {
            colored: vec![],
            byline_at: Some(now),
            ..Default::default()
        },
    );

    assert_eq!(rows[0].lines, ["@dev · 2d"]);
}

#[test]
fn ages_use_minutes_then_hours_then_days() {
    assert_eq!(format_age(59), "0m");
    assert_eq!(format_age(45 * 60), "45m");
    assert_eq!(format_age(3 * 3_600 + 59 * 60), "3h");
    assert_eq!(format_age(2 * 86_400), "2d");
}

#[test]
fn a_colored_bar_takes_the_state_tone() {
    let rows = rows(
        &[pull_request(false, "", "FAILURE")],
        &RowOptions {
            colored: vec![Part::Bar],
            ..Default::default()
        },
    );

    assert_eq!(rows[0].bar_tone, Tone::Danger);
}

#[test]
fn search_results_carry_merge_state_threads_and_approvals() {
    let pr = pull_request(false, "", "");

    assert!(!pr.conflicting);
    assert_eq!(pr.unresolved_threads, 1);
    assert_eq!(pr.approvals, 1);
    assert_eq!(pr.required_approvals, Some(2));
}

#[test]
fn conflicts_come_right_after_failing_checks() {
    let conflicting = |checks, review| {
        let mut pr = pull_request(false, review, checks);
        pr.conflicting = true;
        pr.state()
    };

    assert_eq!(conflicting("FAILURE", ""), State::Failing);
    assert_eq!(conflicting("", "CHANGES_REQUESTED"), State::Conflicting);
    assert_eq!(conflicting("SUCCESS", "APPROVED"), State::Conflicting);
}

#[test]
fn merge_info_lists_unresolved_threads_and_approvals() {
    let mut pr = pull_request(false, "", "");
    assert_eq!(merge_info(&pr), "1 unresolved · 1/2 approvals");

    pr.unresolved_threads = 0;
    pr.required_approvals = Some(0);
    pr.approvals = 0;
    assert_eq!(merge_info(&pr), "0 approvals");

    pr.required_approvals = None;
    pr.approvals = 1;
    assert_eq!(merge_info(&pr), "1 approval");

    let rows = rows(
        &[pr],
        &RowOptions {
            merge_info: true,
            ..Default::default()
        },
    );
    assert_eq!(rows[0].lines, ["1 approval"]);
}

#[test]
fn unknown_merge_state_is_not_a_conflict() {
    let output = search(
        &node("api", 1, false, "", "", "2026-10-20T10:00:00Z").replace("MERGEABLE", "UNKNOWN"),
    );

    assert!(!parse_search(&output).unwrap()[0].conflicting);
}

#[test]
fn c_copies_the_pull_request_url() {
    let rows = rows(&[pull_request(false, "", "")], &RowOptions::default());

    let copy = rows[0].find_action("c").unwrap();
    assert_eq!(
        copy.command.last().map(String::as_str),
        Some("https://github.com/org/api/pull/1")
    );
    assert!(copy.label.is_some());
}
