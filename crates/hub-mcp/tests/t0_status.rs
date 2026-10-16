//! Plan 07, T0 (A4): `just mcp-t0-status` reads the results table of D91 and says NOT RUN for every cell a human has not
//! recorded. It never counts NOT RUN as a pass, it fails on a table it cannot read, and with `--strict` it exits 1 until every
//! cell is filled with a value that follows the outcome rules of the prompt. The tool is `examples/t0_status.rs`, included here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
#[path = "../examples/t0_status.rs"]
mod t0_status;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use t0_status::{Kind, NOT_RUN, ROWS, RowSpec, Setting, StatusError, parse, run};

const OTHER_DECISIONS: &str = "- **D90** (2026-10-13) The engine reads YAML with `granit-parser`.\n";

/// The text of D91 with `value(row, os)` in every cell, between two other decisions.
fn d91(value: &dyn Fn(&RowSpec, usize) -> String) -> String {
    let mut text = String::from(OTHER_DECISIONS);
    text.push_str("- **D91** (2026-10-15) **MCP compatibility test (T0).** Some prose.\n  - A sub-bullet that is not part of the table.\n\n");
    text.push_str("  | ID | What is recorded | Windows | macOS |\n  |---|---|---|---|\n");
    for row in ROWS {
        let _ = writeln!(
            text,
            "  | `{}` | what | {} | {} |",
            row.id,
            value(row, 0),
            value(row, 1)
        );
    }
    text.push_str("\n- **D92** (2026-10-16) A later decision.\n");
    text
}

fn all_not_run() -> String {
    d91(&|_, _| NOT_RUN.to_owned())
}

/// A value a human could have recorded for this row.
fn recorded(row: &RowSpec) -> String {
    match row.kind {
        Kind::CursorBuild => "3.22.12".to_owned(),
        Kind::Person => "A. Tester".to_owned(),
        Kind::Date => "2026-10-20".to_owned(),
        Kind::Check => "PASS".to_owned(),
        Kind::Observation => "observed as described".to_owned(),
        Kind::Decision(Setting::Auth) => "oauth".to_owned(),
        Kind::Decision(Setting::Confirm) => "elicitation".to_owned(),
        Kind::Decision(Setting::Content) => "summary".to_owned(),
    }
}

fn all_recorded() -> String {
    d91(&|row, _| recorded(row))
}

/// All recorded, except `id` on `os` (0 Windows, 1 macOS), which holds `value`.
fn with(id: &str, os: usize, value: &str) -> String {
    d91(&|row, o| {
        if row.id == id && o == os {
            value.to_owned()
        } else {
            recorded(row)
        }
    })
}

fn status(text: &str, strict: bool) -> (String, i32) {
    run(text, strict)
}

#[test]
fn every_cell_not_run_prints_not_run_per_os_and_exits_zero() {
    let (output, code) = status(&all_not_run(), false);
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("overall: NOT RUN"), "{output}");
    for os in ["Windows", "macOS"] {
        assert!(
            output.contains(&format!("{os}: 0 of {} cells recorded", ROWS.len())),
            "{output}"
        );
    }
    assert!(
        output.contains("PASS 0, FAIL 0"),
        "nothing was run, so nothing passed:\n{output}"
    );
    assert!(
        !output.contains("| PASS") && !output.contains("PASS |"),
        "no row shows a result:\n{output}"
    );
    assert!(output.contains("NOT RUN is not a pass"), "{output}");
    insta::assert_snapshot!("t0_status_all_not_run", output);
}

#[test]
fn strict_fails_while_any_cell_is_not_run() {
    let (output, code) = status(&all_not_run(), true);
    assert_eq!(code, 1, "{output}");
    assert!(output.contains("strict: FAILED"), "{output}");

    // One missing cell is enough, on either platform.
    for os in [0, 1] {
        let (output, code) = status(&with("C2.accept", os, NOT_RUN), true);
        assert_eq!(code, 1, "{output}");
        assert!(output.contains("overall: INCOMPLETE"), "{output}");
        assert!(output.contains("1 cell is NOT RUN"), "{output}");
    }
}

#[test]
fn strict_passes_only_when_every_cell_is_recorded_and_follows_the_outcome_rules() {
    let (output, code) = status(&all_recorded(), true);
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("overall: RECORDED"), "{output}");
    assert!(output.contains("strict: ok"), "{output}");
    // Not a verdict on Cursor: the tool reports what was recorded, it does not call the compatibility a pass.
    assert!(
        !output.contains("T0 PASSED") && !output.contains("overall: PASS"),
        "{output}"
    );
    // Without --strict the same table exits 0 as well.
    assert_eq!(status(&all_recorded(), false).1, 0);
}

#[test]
fn partly_recorded_is_incomplete_and_counts_what_was_recorded() {
    let text = d91(&|row, os| {
        if os == 0 && matches!(row.kind, Kind::Check) {
            "PASS".to_owned()
        } else {
            NOT_RUN.to_owned()
        }
    });
    let checks = ROWS.iter().filter(|r| r.kind == Kind::Check).count();
    let (output, code) = status(&text, false);
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("overall: INCOMPLETE"), "{output}");
    assert!(
        output.contains(&format!(
            "Windows: {checks} of {} cells recorded (PASS {checks}, FAIL 0",
            ROWS.len()
        )),
        "{output}"
    );
    assert!(
        output.contains(&format!(
            "macOS: 0 of {} cells recorded (PASS 0, FAIL 0",
            ROWS.len()
        )),
        "{output}"
    );
}

#[test]
fn a_failed_check_is_counted_as_a_failure_never_a_pass() {
    let (output, _) = status(&with("C2.cancel", 1, "FAIL: the prompt stayed open"), false);
    assert!(output.contains("FAIL 1"), "{output}");
    assert!(output.contains("C2.cancel"), "{output}");
    // A FAIL is a recorded result: it selects the fallback mode, it does not make the table unreadable.
    let text = d91(&|row, os| match (row.id, os) {
        ("C2.cancel", 1) => "FAIL: the prompt stayed open".to_owned(),
        ("D.confirm", 1) => "draft".to_owned(),
        _ => recorded(row),
    });
    assert_eq!(status(&text, true).1, 0);
}

#[test]
fn the_mode_decision_must_follow_the_result() {
    // Prompt 07, T0: "If OAuth fails, use personal-token mode. If elicitation fails, use draft mode", per platform.
    for failing in ["C1.discovery", "C1.entra_accepts", "C1.tool_call"] {
        let (output, code) = status(&with(failing, 0, "FAIL: no endpoints found"), true);
        assert_eq!(code, 1, "{failing}: {output}");
        assert!(
            output.contains("Windows: OAuth failed, so D.auth must be personal_token"),
            "{output}"
        );
    }
    for failing in ["C2.prompt", "C2.accept", "C2.decline", "C2.cancel", "C2.no_hang"] {
        let (output, code) = status(&with(failing, 1, "FAIL: hangs"), true);
        assert_eq!(code, 1, "{failing}: {output}");
        assert!(
            output.contains("macOS: elicitation failed, so D.confirm must be draft"),
            "{output}"
        );
    }
    // Following the rule on one platform is not enough for the other.
    let text = d91(&|row, os| match (row.id, os) {
        ("C2.accept", 0) => "FAIL: hangs".to_owned(),
        ("D.confirm", 0 | 1) => "draft".to_owned(),
        _ => recorded(row),
    });
    assert_eq!(
        status(&text, true).1,
        0,
        "forcing draft mode on a platform that works is allowed"
    );
}

#[test]
fn neither_oauth_nor_headers_working_is_a_stop_and_escalate_not_a_mode() {
    // Prompt 07, "Stop and escalate if": T0 finds that neither OAuth nor headers work reliably in 3.22.12.
    let text = d91(&|row, os| match (row.id, os) {
        ("C1.discovery" | "C1.headers", 0) => "FAIL: nothing worked".to_owned(),
        ("D.auth", 0) => "personal_token".to_owned(),
        _ => recorded(row),
    });
    let (output, code) = status(&text, true);
    assert_eq!(code, 1, "{output}");
    assert!(
        output.contains("Windows: neither OAuth nor headers work, so stop and escalate"),
        "{output}"
    );
    // Headers working is what makes personal-token mode a way out.
    let text = d91(&|row, os| match (row.id, os) {
        ("C1.discovery", 0) => "FAIL: nothing worked".to_owned(),
        ("D.auth", 0) => "personal_token".to_owned(),
        _ => recorded(row),
    });
    assert_eq!(status(&text, true).1, 0);
}

#[test]
fn a_build_other_than_3_22_12_does_not_count_in_strict_mode() {
    for build in ["3.22.11", "3.23.0", "latest", "3.22.120"] {
        let (output, code) = status(&with("META.cursor_build", 0, build), true);
        assert_eq!(code, 1, "{build}: {output}");
        assert!(
            output.contains("Windows: META.cursor_build is not 3.22.12"),
            "{output}"
        );
        assert_eq!(
            status(&with("META.cursor_build", 0, build), false).1,
            0,
            "without --strict it is only reported"
        );
    }
    for build in ["3.22.12", "3.22.12 (user setup)"] {
        assert_eq!(status(&with("META.cursor_build", 0, build), true).1, 0, "{build}");
    }
}

#[test]
fn a_table_that_cannot_be_read_is_an_error_in_both_modes() {
    let text = all_not_run();
    let cases: Vec<(String, StatusError)> = vec![
        (text.replace("- **D91**", "- **D911**"), StatusError::NoD91),
        (text.replacen("  | `C2.accept` | what | NOT RUN | NOT RUN |\n", "", 1), StatusError::MissingRow("C2.accept".to_owned())),
        (
            text.replacen("  | `C2.accept` | what | NOT RUN | NOT RUN |\n", "  | `C2.accept` | what | NOT RUN | NOT RUN |\n  | `C2.accept` | what | NOT RUN | NOT RUN |\n", 1),
            StatusError::DuplicateRow("C2.accept".to_owned()),
        ),
        (text.replacen("`C2.accept`", "`C2.acept`", 1), StatusError::UnknownRow("C2.acept".to_owned())),
    ];
    for (text, expected) in cases {
        for strict in [false, true] {
            assert_eq!(parse(&text).unwrap_err(), expected);
            let (output, code) = status(&text, strict);
            assert_eq!(code, 1, "{output}");
            assert!(output.contains("cannot read D91"), "{output}");
            assert!(
                !output.contains("overall:"),
                "no status is printed for a table that cannot be read:\n{output}"
            );
        }
    }
}

#[test]
fn a_cell_with_an_unknown_value_is_an_error_not_a_pass() {
    // A check cell is NOT RUN, PASS, FAIL or `FAIL: reason`. "pass", "ok", "yes" and an empty cell are not results.
    for bad in [
        "pass",
        "Pass",
        "OK",
        "yes",
        "true",
        "",
        " ",
        "FAIL:",
        "FAIL:  ",
        "PASS (mostly)",
        "NOT  RUN",
        "not run",
    ] {
        let text = with("C1.discovery", 0, bad);
        let error = parse(&text).unwrap_err();
        assert!(
            matches!(error, StatusError::BadCell { ref row, .. } if row == "C1.discovery"),
            "{bad:?}: {error}"
        );
        assert_eq!(status(&text, false).1, 1, "{bad:?}");
    }
    // Free-text rows may say anything but a misspelling of NOT RUN, which would count as "recorded".
    for (row, bad) in [
        ("O.protocol_version", "not run"),
        ("O.refresh", "Not Run"),
        ("C1.mirror_needed", "NOT  RUN"),
        ("META.recorded_by", "not  run"),
        ("META.cursor_build", "Not run"),
    ] {
        assert!(
            matches!(
                parse(&with(row, 1, bad)).unwrap_err(),
                StatusError::BadCell { .. }
            ),
            "{row}={bad}"
        );
    }
    // Decisions are exactly a documented mode of the setting.
    for (row, bad) in [
        ("D.auth", "basic"),
        ("D.auth", "oauth,personal_token"),
        ("D.confirm", "Draft"),
        ("D.content", "raw"),
        ("D.auth", "elicitation"),
    ] {
        assert!(
            matches!(
                parse(&with(row, 0, bad)).unwrap_err(),
                StatusError::BadCell { .. }
            ),
            "{row}={bad}"
        );
    }
    // The date is a date.
    for bad in ["20 Oct 2026", "2026-13-01", "2026-10-32", "yesterday", "2026-1-5"] {
        assert!(
            matches!(
                parse(&with("META.date", 0, bad)).unwrap_err(),
                StatusError::BadCell { .. }
            ),
            "{bad}"
        );
    }
    // An observation is free text, but bounded.
    assert!(parse(&with("O.protocol_version", 0, &"x".repeat(201))).is_err());
    assert!(parse(&with("O.protocol_version", 0, &"x".repeat(200))).is_ok());
}

#[test]
fn only_the_rows_of_d91_count() {
    // The same ids in another decision are ignored, and so is a table that follows D91 in a later entry.
    let mut text = all_not_run();
    text.push_str("  | `C2.accept` | decoy | PASS | PASS |\n");
    text.push_str("\n- **D93** later\n\n  | `C2.accept` | decoy | PASS | PASS |\n");
    let (output, code) = status(&text, false);
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("overall: NOT RUN"), "{output}");
    let before = format!(
        "- **D90** earlier\n\n  | `C2.accept` | decoy | PASS | PASS |\n\n{}",
        all_not_run()
    );
    assert!(status(&before, false).0.contains("overall: NOT RUN"));
}

#[test]
fn a_filled_table_that_a_human_edited_by_hand_still_parses() {
    // Extra spaces, no indentation, and cells with pipes escaped as `\|` are what a person produces.
    let text = all_not_run().replace(
        "  | `C1.discovery` | what | NOT RUN | NOT RUN |",
        "|`C1.discovery`|what|   PASS   |FAIL: no \\| endpoint|",
    );
    let table = parse(&text).unwrap();
    assert_eq!(table.cell("C1.discovery", 0), Some("PASS"));
    assert_eq!(table.cell("C1.discovery", 1), Some("FAIL: no | endpoint"));
}

#[test]
fn the_row_list_has_unique_ids_and_a_documented_procedure_each() {
    let mut seen = std::collections::BTreeSet::new();
    for row in ROWS {
        assert!(seen.insert(row.id), "duplicate row id {}", row.id);
    }
    // The runbook explains how to record every row, so a row cannot be added without saying how to fill it in.
    let readme = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("t0/README.md")).unwrap();
    for row in ROWS {
        assert!(
            readme.contains(&format!("`{}`", row.id)),
            "t0/README.md does not mention `{}`",
            row.id
        );
    }
}

#[test]
fn the_committed_d91_can_be_read_and_never_claims_more_than_it_records() {
    let decisions =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/decisions.md"))
            .unwrap();
    let table = parse(&decisions).expect("D91 in docs/decisions.md must be a readable table");
    let (output, code) = status(&decisions, false);
    assert_eq!(code, 0, "{output}");
    let filled = ROWS
        .iter()
        .flat_map(|r| [0, 1].map(|os| table.cell(r.id, os).unwrap_or("?")))
        .filter(|c| *c != NOT_RUN)
        .count();
    // Whatever a human has recorded, the tool agrees with the table about how much that is, and strict mode agrees with it too.
    assert!(
        output.contains(&format!(
            "overall: {}",
            if filled == 0 { "NOT RUN" } else { "INCOMPLETE" }
        )) || filled == ROWS.len() * 2,
        "{output}"
    );
    let strict = status(&decisions, true).1;
    assert_eq!(
        strict == 0,
        filled == ROWS.len() * 2 && !output.contains("problem"),
        "strict and the table disagree:\n{output}"
    );
}

#[test]
fn the_status_covers_the_three_cursor_checks_and_the_observations_the_design_needs() {
    let ids: Vec<&str> = ROWS.iter().map(|r| r.id).collect();
    // Check 1 (Entra OAuth), check 2 (elicitation), check 3 (the tool allowlist) and the five observations of plan A4.
    for needed in [
        "C1.discovery",
        "C1.entra_accepts",
        "C1.tool_call",
        "C1.headers",
        "C2.prompt",
        "C2.accept",
        "C2.decline",
        "C2.cancel",
        "C2.no_hang",
        "C3.read_runs",
        "C3.write_prompts",
        "O.protocol_version",
        "O.structured_content",
        "O.error_vs_output_schema",
        "O.session_404",
        "O.origin_header",
        "D.auth",
        "D.confirm",
        "D.content",
    ] {
        assert!(ids.contains(&needed), "no row {needed}");
    }
    let by_id: BTreeMap<&str, &RowSpec> = ROWS.iter().map(|r| (r.id, r)).collect();
    assert_eq!(by_id["C2.cancel"].kind, Kind::Check);
    assert_eq!(by_id["D.confirm"].kind, Kind::Decision(Setting::Confirm));
}
