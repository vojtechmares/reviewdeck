//! Unified-diff parsing shared by every provider adapter and the diff viewer - a
//! port of src/shared/diff.ts.
//!
//! Providers hand us diffs in two shapes: one blob covering every file (Forgejo
//! `.diff`, Bitbucket `/diff`) or a per-file patch (GitHub `files[].patch`, GitLab
//! `changes[].diff`). Both funnel through the same hunk parser.
//!
//! The TypeScript hands line *objects* around and keys per-line state (syntax
//! tokens, inline threads, a range being picked) by their identity. Rust addresses a
//! line by where it sits instead: its index in [`DiffHunk::lines`], and the hunk's
//! index in the file's hunks ([`LineRef`]). Both are stable for as long as the patch
//! text is, which is exactly as long as object identity lasted in the renderer.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::model::{DiffFile, EdgeKind, FileStatus, LineRange, RangeEdge, Side};

/// Which side of a diff a line is read from. The same `'old' | 'new'` the
/// TypeScript spells out again here, so it is the model's [`Side`].
pub type DiffSide = Side;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffLineKind {
    Context,
    Add,
    Del,
    Meta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
    /// Set only on the sides the line exists on: both for context, one for a change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_line: Option<u32>,
    /// Where the line sits in each file as the diff counts it, set on every line but
    /// an annotation. Unlike the two above these never go missing: an added line
    /// carries the old-file number it was inserted ahead of, a removed line the
    /// new-file number that follows it. See [`RangeEdge`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_pos: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_pos: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    pub header: String,
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<DiffLine>,
}

/// Where a line sits in a file's hunks: the stable address the UI keys per-line
/// state by, in place of the object identity the TypeScript used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LineRef {
    /// Index into the file's hunks, as [`parse_patch`] returned them.
    pub hunk: usize,
    /// Index into that hunk's [`DiffHunk::lines`].
    pub line: usize,
}

/// The line a [`LineRef`] points at, or `None` when it points past the end.
pub fn line_at(hunks: &[DiffHunk], at: LineRef) -> Option<&DiffLine> {
    hunks.get(at.hunk)?.lines.get(at.line)
}

// `\d` is ASCII in JavaScript and Unicode in Rust, and `.` stops at `\r`, ` `
// and ` ` in JavaScript as well as at `\n`; both are spelled out so the two
// regexes accept exactly the same headers.
static HUNK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^@@+ -([0-9]+)(?:,([0-9]+))? \+([0-9]+)(?:,([0-9]+))? @@+([^\n\r\u{2028}\u{2029}]*)$",
    )
    .expect("the hunk header regex is valid")
});

static GIT_HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^diff --git ["']?a/([^\n\r\u{2028}\u{2029}]+?)["']? ["']?b/([^\n\r\u{2028}\u{2029}]+?)["']?$"#,
    )
    .expect("the git header regex is valid")
});

/// A hunk-header number. JavaScript's `Number` cannot fail on `\d+`; a count too big
/// for a `u32` saturates rather than failing the whole patch.
fn header_number(text: &str) -> u32 {
    text.parse::<u64>()
        .map_or(u32::MAX, |value| u32::try_from(value).unwrap_or(u32::MAX))
}

/// Parse a single file's patch body into hunks.
pub fn parse_patch(patch: &str) -> Vec<DiffHunk> {
    let mut hunks: Vec<DiffHunk> = Vec::new();
    if patch.is_empty() {
        return hunks;
    }
    let mut old_line: u32 = 0;
    let mut new_line: u32 = 0;

    for raw in patch.split('\n') {
        if let Some(found) = HUNK_RE.captures(raw) {
            let number = |index: usize| found.get(index).map(|m| header_number(m.as_str()));
            let old_start = number(1).unwrap_or(0);
            let new_start = number(3).unwrap_or(0);
            hunks.push(DiffHunk {
                header: raw.to_string(),
                old_start,
                old_count: number(2).unwrap_or(1),
                new_start,
                new_count: number(4).unwrap_or(1),
                lines: Vec::new(),
            });
            old_line = old_start;
            new_line = new_start;
            continue;
        }
        let Some(current) = hunks.last_mut() else {
            continue;
        };

        // "\ No newline at end of file" annotates the previous line rather than being one.
        if let Some(rest) = raw.strip_prefix('\\') {
            current.lines.push(DiffLine {
                kind: DiffLineKind::Meta,
                content: rest.trim().to_string(),
                old_line: None,
                new_line: None,
                old_pos: None,
                new_pos: None,
            });
            continue;
        }

        if let Some(content) = raw.strip_prefix('+') {
            current.lines.push(DiffLine {
                kind: DiffLineKind::Add,
                content: content.to_string(),
                old_line: None,
                new_line: Some(new_line),
                old_pos: Some(old_line),
                new_pos: Some(new_line),
            });
            new_line = new_line.saturating_add(1);
        } else if let Some(content) = raw.strip_prefix('-') {
            current.lines.push(DiffLine {
                kind: DiffLineKind::Del,
                content: content.to_string(),
                old_line: Some(old_line),
                new_line: None,
                old_pos: Some(old_line),
                new_pos: Some(new_line),
            });
            old_line = old_line.saturating_add(1);
        } else if raw.starts_with(' ') || raw.is_empty() {
            // A fully empty line inside a hunk is a context line whose content is empty.
            current.lines.push(DiffLine {
                kind: DiffLineKind::Context,
                content: raw.get(1..).unwrap_or("").to_string(),
                old_line: Some(old_line),
                new_line: Some(new_line),
                old_pos: Some(old_line),
                new_pos: Some(new_line),
            });
            old_line = old_line.saturating_add(1);
            new_line = new_line.saturating_add(1);
        }
        // Anything else (stray git noise) is skipped rather than corrupting line numbers.
    }
    hunks
}

fn strip_prefix(path: &str) -> String {
    if path == "/dev/null" {
        return String::new();
    }
    // git uses a/ and b/ prefixes, but honours -p0 style diffs too.
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
        .to_string()
}

/// Split a multi-file unified diff blob into per-file entries.
pub fn parse_unified_diff(text: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    if text.is_empty() {
        return files;
    }

    let lines: Vec<&str> = text.split('\n').collect();
    let mut i = 0;

    while i < lines.len() {
        // `diff --git ` is a `diff ` line too; the TypeScript asks both.
        if !lines[i].starts_with("diff ") {
            i += 1;
            continue;
        }

        let header_line = lines[i];
        i += 1;

        let mut old_path = String::new();
        let mut new_path = String::new();
        let mut binary = false;
        let mut added = false;
        let mut removed = false;
        let mut renamed = false;
        let mut body: Vec<&str> = Vec::new();

        // Consume the extended header block that precedes the first hunk.
        while i < lines.len() && !lines[i].starts_with("@@") && !lines[i].starts_with("diff ") {
            let line = lines[i];
            if let Some(rest) = line.strip_prefix("--- ") {
                old_path = strip_prefix(rest.trim());
            } else if let Some(rest) = line.strip_prefix("+++ ") {
                new_path = strip_prefix(rest.trim());
            } else if line.starts_with("new file mode") {
                added = true;
            } else if line.starts_with("deleted file mode") {
                removed = true;
            } else if let Some(rest) = line.strip_prefix("rename from ") {
                renamed = true;
                old_path = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("rename to ") {
                renamed = true;
                new_path = rest.trim().to_string();
            } else if line.starts_with("Binary files") || line.starts_with("GIT binary patch") {
                binary = true;
            }
            i += 1;
        }

        // Collect the hunks until the next file header.
        while i < lines.len() && !lines[i].starts_with("diff ") {
            body.push(lines[i]);
            i += 1;
        }

        // Fall back to the `diff --git a/x b/y` line when there were no ---/+++ markers.
        if old_path.is_empty()
            && new_path.is_empty()
            && let Some(found) = GIT_HEADER_RE.captures(header_line)
        {
            old_path = found.get(1).map_or("", |m| m.as_str()).to_string();
            new_path = found.get(2).map_or("", |m| m.as_str()).to_string();
        }

        let path = if new_path.is_empty() {
            old_path.clone()
        } else {
            new_path.clone()
        };
        if path.is_empty() {
            continue;
        }

        let joined = body.join("\n");
        let patch = if joined.trim().is_empty() {
            None
        } else {
            Some(joined)
        };
        let counted = count_changes(patch.as_deref().unwrap_or(""));

        let status = if added || old_path.is_empty() {
            FileStatus::Added
        } else if removed || new_path.is_empty() {
            FileStatus::Removed
        } else if renamed {
            FileStatus::Renamed
        } else {
            FileStatus::Modified
        };

        files.push(DiffFile {
            old_path: if old_path.is_empty() {
                path.clone()
            } else {
                old_path
            },
            path,
            status,
            additions: counted.additions,
            deletions: counted.deletions,
            patch,
            binary,
        });
    }

    files
}

/// How many lines a patch adds and removes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeCount {
    pub additions: u32,
    pub deletions: u32,
}

pub fn count_changes(patch: &str) -> ChangeCount {
    let mut counted = ChangeCount::default();
    for line in patch.split('\n') {
        if line.starts_with('+') && !line.starts_with("+++") {
            counted.additions += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            counted.deletions += 1;
        }
    }
    counted
}

/// One row of the side-by-side layout, as indices into [`DiffHunk::lines`]. A
/// context line is the same index in both columns, as it was the same object in
/// both columns in the TypeScript.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SplitRow {
    pub left: Option<usize>,
    pub right: Option<usize>,
}

/// Lay a hunk out for side-by-side viewing: context lines occupy both columns,
/// and consecutive runs of removals/additions are zipped so a rewritten line
/// lines up with its replacement.
pub fn to_split_rows(hunk: &DiffHunk) -> Vec<SplitRow> {
    let mut rows = Vec::new();
    let mut dels: Vec<usize> = Vec::new();
    let mut adds: Vec<usize> = Vec::new();

    let flush = |rows: &mut Vec<SplitRow>, dels: &mut Vec<usize>, adds: &mut Vec<usize>| {
        for i in 0..dels.len().max(adds.len()) {
            rows.push(SplitRow {
                left: dels.get(i).copied(),
                right: adds.get(i).copied(),
            });
        }
        dels.clear();
        adds.clear();
    };

    for (index, line) in hunk.lines.iter().enumerate() {
        match line.kind {
            DiffLineKind::Del => dels.push(index),
            DiffLineKind::Add => adds.push(index),
            DiffLineKind::Meta => {}
            DiffLineKind::Context => {
                flush(&mut rows, &mut dels, &mut adds);
                rows.push(SplitRow {
                    left: Some(index),
                    right: Some(index),
                });
            }
        }
    }
    flush(&mut rows, &mut dels, &mut adds);
    rows
}

/// Where a line comment lands: the file, the line on whichever side it is on, and
/// the lines above it that it also covers, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentTarget {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<LineRange>,
}

/// The number a line has on one side, or nothing when it is not on that side.
pub fn line_on(line: &DiffLine, side: DiffSide) -> Option<u32> {
    match side {
        Side::New => line.new_line,
        Side::Old => line.old_line,
    }
}

fn edge(line: &DiffLine) -> Option<RangeEdge> {
    let kind = match line.kind {
        DiffLineKind::Add => EdgeKind::Add,
        DiffLineKind::Del => EdgeKind::Del,
        DiffLineKind::Context => EdgeKind::Context,
        DiffLineKind::Meta => return None,
    };
    Some(RangeEdge {
        kind,
        old_pos: line.old_pos?,
        new_pos: line.new_pos?,
    })
}

/// The comment covering every line from one to another on one side of a hunk, in
/// whichever order the two were picked - or nothing when the pair cannot make one.
///
/// Both lines have to be on the side asked for, and both in the same hunk: the
/// lines between two hunks are not in the diff, and a host asked to cover them
/// refuses the whole comment. The same line twice is an ordinary single-line
/// comment, which is what a drag that never left its row should produce.
///
/// The TypeScript took the hunk and two line objects and checked the hunk contains
/// both; here the two picks are addresses into the file's hunks, and "in the same
/// hunk" is their hunk indices agreeing.
pub fn range_target(
    path: &str,
    hunks: &[DiffHunk],
    side: DiffSide,
    first: LineRef,
    second: LineRef,
) -> Option<CommentTarget> {
    if first.hunk != second.hunk {
        return None;
    }
    let first_line = line_at(hunks, first)?;
    let second_line = line_at(hunks, second)?;
    let a = line_on(first_line, side)?;
    let b = line_on(second_line, side)?;

    let ((from, from_line), (to, to_line)) = if a <= b {
        ((first, first_line), (second, second_line))
    } else {
        ((second, second_line), (first, first_line))
    };
    let last = side_target(path, to_line, side)?;
    if from == to {
        return Some(last);
    }

    let start = edge(from_line)?;
    let end = edge(to_line)?;
    Some(CommentTarget {
        range: Some(LineRange {
            start_line: line_on(from_line, side)?,
            start,
            end,
        }),
        ..last
    })
}

/// The comment this line takes from one gutter, or nothing when it has no line there.
pub fn side_target(path: &str, line: &DiffLine, side: DiffSide) -> Option<CommentTarget> {
    comment_targets(path, line)
        .into_iter()
        .find(|target| match side {
            Side::Old => target.old_line.is_some(),
            Side::New => target.new_line.is_some(),
        })
}

/// Where a comment stands, for [`covers_line`]: the side, its own line, and the
/// first line of the range it reaches back to, if it covers several.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineAnchor {
    pub side: DiffSide,
    pub line: u32,
    pub start_line: Option<u32>,
}

/// Whether a comment standing on `line` and reaching back to `start_line` covers a
/// line - by number on that side, which is all a range is once it has left the
/// diff it was drawn on.
pub fn covers_line(anchor: LineAnchor, line: &DiffLine) -> bool {
    let Some(number) = line_on(line, anchor.side) else {
        return false;
    };
    number <= anchor.line && number >= anchor.start_line.unwrap_or(anchor.line)
}

/// The comments a diff line can take, one per side it exists on.
///
/// Every host addresses a line by which side's number is given: an added line
/// carries only a new line number, a removed line only an old one, and a context
/// line has both - so it can be commented on from either side.
pub fn comment_targets(path: &str, line: &DiffLine) -> Vec<CommentTarget> {
    let target = |old_line: Option<u32>, new_line: Option<u32>| CommentTarget {
        path: path.to_string(),
        new_line,
        old_line,
        range: None,
    };
    match line.kind {
        DiffLineKind::Add => vec![target(None, line.new_line)],
        DiffLineKind::Del => vec![target(line.old_line, None)],
        DiffLineKind::Context => vec![target(line.old_line, None), target(None, line.new_line)],
        DiffLineKind::Meta => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "@@ -1,6 +1,7 @@
 const a = 1
-const b = 2
+const b = 3
+const c = 4
 const d = 5
 const e = 6
 const f = 7";

    // --- test/diff.test.ts ---

    #[test]
    fn parse_patch_numbers_lines_on_both_sides() {
        let hunks = parse_patch(PATCH);
        let hunk = &hunks[0];
        assert_eq!(hunk.old_start, 1);
        assert_eq!(hunk.old_count, 6);
        assert_eq!(hunk.new_start, 1);
        assert_eq!(hunk.new_count, 7);

        let kinds: Vec<_> = hunk.lines.iter().map(|line| line.kind).collect();
        use DiffLineKind::*;
        assert_eq!(kinds, [Context, Del, Add, Add, Context, Context, Context]);

        let removed = &hunk.lines[1];
        assert_eq!(removed.old_line, Some(2));
        assert_eq!(removed.new_line, None);

        let added = &hunk.lines[2];
        assert_eq!(added.new_line, Some(2));
        assert_eq!(added.old_line, None);

        // The context line after two additions must resume at old 3 / new 4.
        let after = &hunk.lines[4];
        assert_eq!(after.old_line, Some(3));
        assert_eq!(after.new_line, Some(4));
    }

    #[test]
    fn parse_patch_handles_a_hunk_header_without_counts() {
        let hunks = parse_patch("@@ -7 +7 @@\n-old\n+new");
        assert_eq!(hunks[0].old_start, 7);
        assert_eq!(hunks[0].old_count, 1);
        assert_eq!(hunks[0].new_count, 1);
    }

    #[test]
    fn parse_patch_keeps_blank_context_lines_aligned() {
        let hunks = parse_patch("@@ -1,3 +1,3 @@\n a\n\n-b\n+c");
        let kinds: Vec<_> = hunks[0].lines.iter().map(|line| line.kind).collect();
        use DiffLineKind::*;
        assert_eq!(kinds, [Context, Context, Del, Add]);
        assert_eq!(hunks[0].lines[2].old_line, Some(3));
    }

    #[test]
    fn parse_patch_treats_the_no_newline_marker_as_metadata() {
        let hunks = parse_patch("@@ -1,1 +1,1 @@\n-a\n\\ No newline at end of file\n+b");
        assert_eq!(hunks[0].lines[1].kind, DiffLineKind::Meta);
        assert_eq!(hunks[0].lines[1].content, "No newline at end of file");
        // The marker must not consume a line number.
        assert_eq!(hunks[0].lines[2].new_line, Some(1));
    }

    #[test]
    fn to_split_rows_zips_replacement_runs_and_shares_context_lines() {
        let hunks = parse_patch(PATCH);
        let hunk = &hunks[0];
        let rows = to_split_rows(hunk);
        let content = |index: Option<usize>| index.map(|i| hunk.lines[i].content.as_str());

        assert_eq!(rows[0].left, rows[0].right);
        assert!(rows[0].left.is_some());

        // One deletion against two additions: pair the first, leave the second alone.
        assert_eq!(content(rows[1].left), Some("const b = 2"));
        assert_eq!(content(rows[1].right), Some("const b = 3"));
        assert_eq!(rows[2].left, None);
        assert_eq!(content(rows[2].right), Some("const c = 4"));
    }

    const MULTI: &str = "diff --git a/src/app.ts b/src/app.ts
index 1111111..2222222 100644
--- a/src/app.ts
+++ b/src/app.ts
@@ -1,2 +1,3 @@
 import x
+import y
 export default x
diff --git a/README.md b/README.md
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/README.md
@@ -0,0 +1,2 @@
+# Title
+Body
diff --git a/old.txt b/renamed.txt
similarity index 100%
rename from old.txt
rename to renamed.txt
diff --git a/logo.png b/logo.png
index 4444444..5555555 100644
Binary files a/logo.png and b/logo.png differ
";

    #[test]
    fn parse_unified_diff_splits_a_multi_file_blob() {
        let files = parse_unified_diff(MULTI);
        assert_eq!(files.len(), 4);

        let (app, readme, renamed, binary) = (&files[0], &files[1], &files[2], &files[3]);

        assert_eq!(app.path, "src/app.ts");
        assert_eq!(app.status, FileStatus::Modified);
        assert_eq!(app.additions, 1);
        assert_eq!(app.deletions, 0);
        let app_patch = app.patch.as_deref().unwrap_or("");
        assert!(
            app_patch
                .lines()
                .any(|line| line.starts_with("@@ -1,2 +1,3 @@")),
            "{app_patch}"
        );
        // A file's patch must not bleed into the next file's.
        assert!(!app_patch.contains("# Title"));

        assert_eq!(readme.path, "README.md");
        assert_eq!(readme.status, FileStatus::Added);
        assert_eq!(readme.additions, 2);

        assert_eq!(renamed.status, FileStatus::Renamed);
        assert_eq!(renamed.old_path, "old.txt");
        assert_eq!(renamed.path, "renamed.txt");

        assert!(binary.binary);
        assert_eq!(binary.path, "logo.png");
    }

    #[test]
    fn parse_unified_diff_copes_with_paths_containing_spaces() {
        let files = parse_unified_diff(
            "diff --git a/my dir/a b.ts b/my dir/a b.ts\n--- a/my dir/a b.ts\n+++ b/my dir/a b.ts\n@@ -1 +1 @@\n-x\n+y\n",
        );
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "my dir/a b.ts");
    }

    #[test]
    fn parse_unified_diff_returns_nothing_for_empty_input() {
        assert_eq!(parse_unified_diff(""), Vec::<DiffFile>::new());
        assert_eq!(parse_patch(""), Vec::<DiffHunk>::new());
    }

    #[test]
    fn count_changes_ignores_the_file_headers() {
        let counted = count_changes("--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n+c");
        assert_eq!(
            counted,
            ChangeCount {
                additions: 2,
                deletions: 1
            }
        );
    }

    // --- Rust-specific parsing details ---

    #[test]
    fn parse_unified_diff_falls_back_to_the_git_header_for_a_mode_change() {
        let files =
            parse_unified_diff("diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "run.sh");
        assert_eq!(files[0].old_path, "run.sh");
        assert_eq!(files[0].status, FileStatus::Modified);
        assert_eq!(files[0].patch, None);
    }

    #[test]
    fn parse_unified_diff_marks_a_deleted_file_removed() {
        let files = parse_unified_diff(
            "diff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n",
        );
        assert_eq!(files[0].status, FileStatus::Removed);
        assert_eq!(files[0].path, "gone.txt");
        assert_eq!(files[0].deletions, 1);
    }

    #[test]
    fn a_crlf_hunk_header_is_not_a_header_as_in_javascript() {
        // JavaScript's `.` stops at `\r`, so a header with a trailing carriage
        // return never matched there; it must not match here either.
        assert_eq!(parse_patch("@@ -1 +1 @@\r\n-a\r\n+b\r"), Vec::new());
        // A carriage return inside the content is just content.
        let hunks = parse_patch("@@ -1 +1 @@\n-a\r\n+b\r");
        assert_eq!(hunks[0].lines[0].content, "a\r");
    }

    #[test]
    fn non_ascii_content_and_unicode_digits_are_handled() {
        // A Unicode digit is not `\d` in JavaScript, so this is not a header.
        assert_eq!(parse_patch("@@ -١ +1 @@\n-a"), Vec::new());
        let hunks = parse_patch("@@ -1 +1 @@\n-žluťoučký\n+kůň 🐎\n\\ Žádný nový řádek");
        assert_eq!(hunks[0].lines[0].content, "žluťoučký");
        assert_eq!(hunks[0].lines[1].content, "kůň 🐎");
        assert_eq!(hunks[0].lines[2].content, "Žádný nový řádek");
    }

    #[test]
    fn a_trailing_newline_leaves_an_empty_context_line_as_in_the_typescript() {
        let hunks = parse_patch("@@ -1 +1 @@\n-a\n+b\n");
        assert_eq!(hunks[0].lines.len(), 3);
        assert_eq!(hunks[0].lines[2].kind, DiffLineKind::Context);
        assert_eq!(hunks[0].lines[2].content, "");
    }

    #[test]
    fn lines_serialise_with_the_typescript_field_names() {
        let hunks = parse_patch("@@ -3 +3 @@\n+x");
        let json = serde_json::to_value(&hunks[0]).unwrap_or_default();
        assert_eq!(
            json,
            serde_json::json!({
                "header": "@@ -3 +3 @@",
                "oldStart": 3, "oldCount": 1, "newStart": 3, "newCount": 1,
                "lines": [{ "kind": "add", "content": "x", "newLine": 3, "oldPos": 3, "newPos": 3 }],
            })
        );
    }

    // --- test/comment-targets.test.ts ---

    const TARGET_PATCH: &str = "@@ -1,3 +1,3 @@
 const a = 1
-const b = 2
+const b = 3
 const d = 5";

    fn target(old_line: Option<u32>, new_line: Option<u32>) -> CommentTarget {
        CommentTarget {
            path: "src/a.ts".into(),
            new_line,
            old_line,
            range: None,
        }
    }

    #[test]
    fn an_added_line_can_be_commented_on_addressed_by_its_new_line_number() {
        let hunks = parse_patch(TARGET_PATCH);
        assert_eq!(
            comment_targets("src/a.ts", &hunks[0].lines[2]),
            [target(None, Some(2))]
        );
    }

    #[test]
    fn a_removed_line_can_be_commented_on_addressed_by_its_old_line_number() {
        let hunks = parse_patch(TARGET_PATCH);
        assert_eq!(
            comment_targets("src/a.ts", &hunks[0].lines[1]),
            [target(Some(2), None)]
        );
    }

    #[test]
    fn a_context_line_can_be_commented_on_from_either_side() {
        let hunks = parse_patch(TARGET_PATCH);
        assert_eq!(
            comment_targets("src/a.ts", &hunks[0].lines[0]),
            [target(Some(1), None), target(None, Some(1))]
        );
    }

    #[test]
    fn a_no_newline_annotation_is_not_a_line_and_takes_no_comment() {
        let hunks = parse_patch("@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+a\n");
        assert_eq!(comment_targets("src/a.ts", &hunks[0].lines[1]), []);
    }

    // --- ranges ---

    /// Where each line sits in both files as the diff counts it, which is what
    /// GitLab's line codes are built from. An added line takes the old-file number
    /// it was inserted ahead of, a removed line the new-file number that follows it.
    #[test]
    fn every_line_records_its_position_in_both_files() {
        let hunks = parse_patch("@@ -1,3 +1,4 @@\n a\n-b\n+b2\n+c\n d");
        let positions: Vec<_> = hunks[0]
            .lines
            .iter()
            .map(|line| (line.kind, line.old_pos, line.new_pos))
            .collect();
        use DiffLineKind::*;
        assert_eq!(
            positions,
            [
                (Context, Some(1), Some(1)),
                (Del, Some(2), Some(2)),
                (Add, Some(3), Some(2)),
                (Add, Some(3), Some(3)),
                (Context, Some(3), Some(4)),
            ]
        );
    }

    const RANGE_PATCH: &str = "@@ -10,4 +10,5 @@
 keep
-gone
+here
+more
 tail
@@ -30,2 +31,2 @@
 far
+away";

    const KEEP: LineRef = LineRef { hunk: 0, line: 0 };
    const GONE: LineRef = LineRef { hunk: 0, line: 1 };
    const HERE: LineRef = LineRef { hunk: 0, line: 2 };
    const MORE: LineRef = LineRef { hunk: 0, line: 3 };
    const TAIL: LineRef = LineRef { hunk: 0, line: 4 };
    const FAR: LineRef = LineRef { hunk: 1, line: 0 };

    fn edge_of(kind: EdgeKind, old_pos: u32, new_pos: u32) -> RangeEdge {
        RangeEdge {
            kind,
            old_pos,
            new_pos,
        }
    }

    #[test]
    fn a_range_covers_every_line_between_two_picks_whichever_was_picked_first() {
        let hunks = parse_patch(RANGE_PATCH);
        let forward = range_target("src/a.ts", &hunks, Side::New, KEEP, MORE);
        assert_eq!(
            forward,
            Some(CommentTarget {
                path: "src/a.ts".into(),
                new_line: Some(12),
                old_line: None,
                range: Some(LineRange {
                    start_line: 10,
                    start: edge_of(EdgeKind::Context, 10, 10),
                    // The old position is what a removed line above it moved the counter to.
                    end: edge_of(EdgeKind::Add, 12, 12),
                }),
            })
        );
        assert_eq!(
            range_target("src/a.ts", &hunks, Side::New, MORE, KEEP),
            forward
        );
    }

    #[test]
    fn a_range_on_the_old_side_is_addressed_by_old_line_numbers() {
        let hunks = parse_patch(RANGE_PATCH);
        assert_eq!(
            range_target("src/a.ts", &hunks, Side::Old, GONE, TAIL),
            Some(CommentTarget {
                path: "src/a.ts".into(),
                new_line: None,
                old_line: Some(12),
                range: Some(LineRange {
                    start_line: 11,
                    start: edge_of(EdgeKind::Del, 11, 11),
                    end: edge_of(EdgeKind::Context, 12, 13),
                }),
            })
        );
    }

    #[test]
    fn a_pick_that_never_left_its_line_is_an_ordinary_single_line_comment() {
        let hunks = parse_patch(RANGE_PATCH);
        assert_eq!(
            range_target("src/a.ts", &hunks, Side::New, HERE, HERE),
            Some(target(None, Some(11)))
        );
    }

    #[test]
    fn a_range_cannot_take_in_a_line_that_is_not_on_its_side() {
        let hunks = parse_patch(RANGE_PATCH);
        // The removed line has no new number, so a range on the new side cannot end there.
        assert_eq!(
            range_target("src/a.ts", &hunks, Side::New, KEEP, GONE),
            None
        );
        assert_eq!(
            range_target("src/a.ts", &hunks, Side::Old, HERE, TAIL),
            None
        );
    }

    #[test]
    fn a_range_cannot_reach_into_another_hunk() {
        let hunks = parse_patch(RANGE_PATCH);
        assert_eq!(range_target("src/a.ts", &hunks, Side::New, KEEP, FAR), None);
        assert_eq!(range_target("src/a.ts", &hunks, Side::New, FAR, KEEP), None);
        // Nor point past the lines there are.
        let missing = LineRef { hunk: 0, line: 99 };
        assert_eq!(
            range_target("src/a.ts", &hunks, Side::New, KEEP, missing),
            None
        );
    }

    #[test]
    fn covers_line_reads_a_range_by_number_and_a_single_line_as_itself() {
        let hunks = parse_patch(RANGE_PATCH);
        let first = &hunks[0];
        let anchor = LineAnchor {
            side: Side::New,
            line: 12,
            start_line: Some(10),
        };
        let covered: Vec<_> = first
            .lines
            .iter()
            .map(|line| covers_line(anchor, line))
            .collect();
        assert_eq!(covered, [true, false, true, true, false]);
        // A removed line is not on the new side at all, wherever the range is.
        assert!(!covers_line(anchor, &first.lines[1]));

        let single = LineAnchor {
            side: Side::Old,
            line: 11,
            start_line: None,
        };
        let covered: Vec<_> = first
            .lines
            .iter()
            .map(|line| covers_line(single, line))
            .collect();
        assert_eq!(covered, [false, true, false, false, false]);
    }

    #[test]
    fn comment_targets_serialise_like_the_typescript_objects() {
        let hunks = parse_patch(RANGE_PATCH);
        let found = range_target("src/a.ts", &hunks, Side::New, KEEP, MORE);
        assert_eq!(
            serde_json::to_value(found).unwrap_or_default(),
            serde_json::json!({
                "path": "src/a.ts",
                "newLine": 12,
                "range": {
                    "startLine": 10,
                    "start": { "kind": "context", "oldPos": 10, "newPos": 10 },
                    "end": { "kind": "add", "oldPos": 12, "newPos": 12 },
                },
            })
        );
    }
}
