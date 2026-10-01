//! Conservative, forward-only patch placement, with evidence for each edit.
use super::{
    changes::normalize_lf,
    patch::{Hunk, HunkLine},
    workspace::WorkspaceError,
};
use serde_json::{json, Value};

pub(super) fn apply(original: &str, hunks: &[Hunk]) -> Result<(String, Value), WorkspaceError> {
    let bom = original.starts_with('\u{feff}');
    let text = original.trim_start_matches('\u{feff}');
    let ending = if text.contains("\r\n") {
        "\r\n"
    } else if text.contains('\r') && !text.contains('\n') {
        "\r"
    } else {
        "\n"
    };
    let normalized = normalize_lf(text);
    let lines = normalized
        .split_terminator('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut placements = Vec::new();
    let mut cursor = 0;
    let mut quality = 0;
    let mut skipped = Vec::new();
    for (index, hunk) in hunks.iter().enumerate() {
        let anchor = hunk.lines.iter().find_map(|v| {
            if let HunkLine::Anchor(a) = v {
                Some(a)
            } else {
                None
            }
        });
        let eof = hunk.lines.iter().any(|v| matches!(v, HunkLine::EndOfFile));
        let old = hunk
            .lines
            .iter()
            .filter_map(|v| match v {
                HunkLine::Context(s) | HunkLine::Remove(s) => Some(s.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let new = hunk
            .lines
            .iter()
            .filter_map(|v| match v {
                HunkLine::Context(s) | HunkLine::Add(s) => Some(s.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut start = cursor;
        if let Some(anchor) = anchor {
            let mut found = None;
            for grade in 0..3 {
                if let Some(pos) =
                    positions(&lines, &[anchor.clone()], cursor, grade, false).first()
                {
                    found = Some(*pos + 1);
                    break;
                }
            }
            start = found.ok_or_else(|| {
                failure(
                    "PATCH_CONTEXT_NOT_FOUND",
                    "Patch anchor did not match",
                    index,
                    &lines,
                    cursor,
                    vec![],
                )
            })?;
        }
        if old.is_empty() {
            if !lines.is_empty() && !eof {
                return Err(failure(
                    "PATCH_CONTEXT_AMBIGUOUS",
                    "An insertion into an existing file needs unchanged context or an EOF anchor",
                    index,
                    &lines,
                    start,
                    vec![],
                ));
            }
            let pos = if eof { lines.len() } else { start };
            placements.push((pos, pos, index, new));
            cursor = pos;
            continue;
        }
        let mut chosen = None;
        for grade in 0..3 {
            let candidates = positions(&lines, &old, start, grade, eof);
            if candidates.len() > 1 {
                return Err(failure(
                    "PATCH_CONTEXT_AMBIGUOUS",
                    "Patch context matched multiple locations; add context",
                    index,
                    &lines,
                    start,
                    candidates,
                ));
            }
            if let Some(pos) = candidates.first() {
                chosen = Some((*pos, grade));
                break;
            }
        }
        let Some((pos, grade)) = chosen else {
            let evidence = hunk
                .lines
                .iter()
                .any(|v| matches!(v,HunkLine::Context(s) if !s.trim().is_empty()))
                || new.len() >= 2;
            if evidence && !new.is_empty() {
                let mut applied = None;
                for grade in 0..2 {
                    let candidates = positions(&lines, &new, start, grade, eof);
                    if candidates.len() == 1 {
                        applied = Some(candidates[0]);
                        break;
                    }
                    if candidates.len() > 1 {
                        break;
                    }
                }
                if let Some(pos) = applied {
                    skipped.push(index);
                    cursor = pos + new.len();
                    continue;
                }
            }
            return Err(failure(
                "PATCH_CONTEXT_NOT_FOUND",
                "Hunk context did not match file content",
                index,
                &lines,
                start,
                vec![],
            ));
        };
        quality = quality.max(grade);
        let shift = if grade == 2 {
            indent_delta(&lines[pos..pos + old.len()], &old)
        } else {
            None
        };
        let mut replacement = Vec::new();
        let mut offset = 0;
        for row in &hunk.lines {
            match row {
                HunkLine::Context(_) => {
                    replacement.push(lines[pos + offset].clone());
                    offset += 1;
                }
                HunkLine::Remove(_) => offset += 1,
                HunkLine::Add(s) => replacement.push(shift_indent(s, shift.as_ref())),
                _ => {}
            }
        }
        if lines[pos..pos + old.len()] == replacement[..] {
            skipped.push(index);
        } else {
            placements.push((pos, pos + old.len(), index, replacement));
        }
        cursor = pos + old.len();
    }
    placements.sort_by_key(|v| (v.0, v.1));
    for pair in placements.windows(2) {
        if pair[1].0 < pair[0].1 || (pair[0].0 == pair[0].1 && pair[1].0 == pair[0].0) {
            return Err(failure(
                "PATCH_HUNKS_OVERLAP",
                "Hunks overlap",
                pair[1].2,
                &lines,
                pair[1].0,
                vec![],
            ));
        }
    }
    let mut ranges = Vec::new();
    let mut delta = 0isize;
    for (a, b, index, replacement) in &placements {
        ranges.push(json!({"hunk_index":index,"old_start_line":a+1,"old_end_line":b,"new_start_line":*a as isize+delta+1,"new_end_line":*a as isize+delta+replacement.len() as isize}));
        delta += replacement.len() as isize - (*b - *a) as isize;
    }
    let mut updated = lines;
    for (a, b, _, replacement) in placements.iter().rev() {
        updated.splice(*a..*b, replacement.clone());
    }
    let mut output = updated.join(ending);
    if !updated.is_empty() && (normalized.ends_with('\n') || normalized.is_empty()) {
        output.push_str(ending);
    }
    if bom {
        output.insert(0, '\u{feff}');
    }
    let match_quality = ["exact", "trailing_ws", "indent"][quality];
    Ok((
        output,
        json!({"match_quality":match_quality,"changed_ranges":ranges,"applied_hunks":placements.len(),"already_applied_hunks":skipped}),
    ))
}

fn positions(
    lines: &[String],
    pattern: &[String],
    start: usize,
    grade: usize,
    eof: bool,
) -> Vec<usize> {
    if pattern.is_empty() || pattern.len() > lines.len() || start > lines.len() - pattern.len() {
        return vec![];
    }
    (start..=lines.len() - pattern.len())
        .filter(|&i| {
            (!eof || i + pattern.len() == lines.len())
                && lines[i..i + pattern.len()]
                    .iter()
                    .zip(pattern)
                    .all(|(a, b)| match grade {
                        0 => a == b,
                        1 => a.trim_end() == b.trim_end(),
                        _ => a.trim() == b.trim(),
                    })
        })
        .collect()
}
fn indent_delta(actual: &[String], expected: &[String]) -> Option<(String, usize)> {
    let mut result = None;
    for (a, b) in actual
        .iter()
        .zip(expected)
        .filter(|(a, b)| !a.trim().is_empty() && !b.trim().is_empty())
    {
        let ai = &a[..a.len() - a.trim_start().len()];
        let bi = &b[..b.len() - b.trim_start().len()];
        let candidate = if let Some(extra) = ai.strip_prefix(bi) {
            (extra.to_string(), 0)
        } else if bi.starts_with(ai) {
            (String::new(), bi.len() - ai.len())
        } else {
            return None;
        };
        if result.as_ref().is_some_and(|old| old != &candidate) {
            return None;
        }
        result = Some(candidate);
    }
    result
}
fn shift_indent(line: &str, shift: Option<&(String, usize)>) -> String {
    let Some((add, drop)) = shift else {
        return line.to_owned();
    };
    if line.trim().is_empty() {
        return line.to_owned();
    }
    let indent = &line[..line.len() - line.trim_start().len()];
    // drop counts UTF-8 bytes from a whitespace prefix; never split a code point.
    let mut cut = (*drop).min(indent.len());
    while !indent.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{add}{}{}", &indent[cut..], line.trim_start())
}
fn failure(
    code: &'static str,
    message: &str,
    index: usize,
    lines: &[String],
    start: usize,
    candidates: Vec<usize>,
) -> WorkspaceError {
    let nearby = lines
        .iter()
        .enumerate()
        .skip(start.saturating_sub(3))
        .take(10)
        .map(|(i, line)| json!({"line":i+1,"content":line.chars().take(512).collect::<String>()}))
        .collect::<Vec<_>>();
    WorkspaceError::ToolDetails {
        code,
        message: message.into(),
        category: "validation",
        retryable: true,
        details: json!({"hunk_index":index,"match_count":candidates.len(),"candidate_lines":candidates.iter().take(100).map(|i|i+1).collect::<Vec<_>>(),"nearby_lines":nearby,"retry_hint":"Read current numbered lines and regenerate the hunk with unique context"}),
    }
}
