//! The handoff to the user's coding agent. A port of src/shared/agent-prompt.ts.
//!
//! Nothing here spawns anything: the app builds a command as text and puts it on
//! the clipboard for the user to run in the terminal they already have open in that
//! repository. That keeps authentication, billing, binary resolution and command
//! injection out of the app entirely - and because it is the user's own interactive
//! shell that runs it, a shell alias works as a command name, which it could not if
//! the app spawned the process itself.
//!
//! The payload is instruction text. It assumes the repository is the working
//! directory and deliberately does not carry the diff.

use crate::model::{
    Account, CommentThread, DEFAULT_AGENT_COMMAND, ProviderKind, ReviewItem, Settings,
};

/// A long bot comment must not turn a handoff into a wall of pasted text. Counted in
/// UTF-16 code units, as JavaScript's `String#length` counts.
const COMMENT_LIMIT: usize = 240;

/// How each host exposes a pull request as a ref under `origin`. Bitbucket publishes
/// no such ref, so it falls back to the source branch - as does any host whose ref
/// shape we do not know.
fn fetch_ref(provider: ProviderKind, number: u64) -> Option<String> {
    match provider {
        ProviderKind::Github | ProviderKind::Forgejo => Some(format!("pull/{number}/head")),
        ProviderKind::Gitlab => Some(format!("merge-requests/{number}/head")),
        ProviderKind::Bitbucket => None,
    }
}

/// `git fetch origin <ref>`, with the host's pull request ref or the source branch.
pub fn fetch_command(item: &ReviewItem) -> String {
    let target =
        fetch_ref(item.provider, item.number).unwrap_or_else(|| item.source_branch.clone());
    format!("git fetch origin {target}")
}

/// What JavaScript's `\s` and `String#trim` treat as white space: Unicode white
/// space plus the byte order mark.
fn is_js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{FEFF}'
}

/// The account's override when it has one, otherwise the setting, otherwise Claude
/// Code. A blank override must not shadow the setting, and a blank setting must not
/// produce a command that is only a quoted prompt.
pub fn agent_command_name(account: Option<&Account>, settings: &Settings) -> String {
    let override_ = account
        .and_then(|account| account.agent_command.as_deref())
        .map(|command| command.trim_matches(is_js_space))
        .filter(|command| !command.is_empty());
    let setting = Some(settings.agent_command.trim_matches(is_js_space))
        .filter(|command| !command.is_empty());
    override_
        .or(setting)
        .unwrap_or(DEFAULT_AGENT_COMMAND)
        .to_string()
}

/// POSIX single-quoting, so the whole prompt survives being pasted into a shell as
/// one argument. This produces text; it does not run it.
pub fn single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// A comment body on one line - every run of white space a single space - and cut
/// to [`COMMENT_LIMIT`] with an ellipsis when it runs over.
fn one_line(body: &str) -> String {
    let flat = body
        .split(is_js_space)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if flat.encode_utf16().count() <= COMMENT_LIMIT {
        return flat;
    }
    // `flat.slice(0, COMMENT_LIMIT - 1)`, except that a character straddling the cut
    // is dropped whole rather than split in half.
    let mut units = 0;
    let mut cut = 0;
    for (at, c) in flat.char_indices() {
        if units + c.len_utf16() > COMMENT_LIMIT - 1 {
            break;
        }
        units += c.len_utf16();
        cut = at + c.len_utf8();
    }
    format!("{}…", &flat[..cut])
}

fn describe_thread(thread: &CommentThread) -> Vec<String> {
    let where_ = match &thread.path {
        Some(path) if !path.is_empty() => match thread.line {
            Some(line) => format!("{path}:{line}"),
            None => path.clone(),
        },
        _ => "On the pull request itself".to_string(),
    };
    let mut lines = vec![format!("- {where_}")];
    lines.extend(
        thread
            .comments
            .iter()
            .map(|comment| format!("  {}: {}", comment.author.name, one_line(&comment.body))),
    );
    lines
}

/// The instruction text itself. Findings are asked for one strict
/// path-and-line-prefixed line at a time so the output is already machine-readable
/// the day an importer is added.
pub fn agent_prompt(item: &ReviewItem, threads: &[CommentThread]) -> String {
    let open: Vec<&CommentThread> = threads.iter().filter(|thread| !thread.resolved).collect();

    let mut lines: Vec<String> = vec![
        format!(
            "Review the pull request \"{}\" ({} #{}).",
            item.title, item.repo, item.number
        ),
        String::new(),
        "Fetch it into the repository you are in:".into(),
        String::new(),
        format!("  {}", fetch_command(item)),
        String::new(),
        format!(
            "The change is then at FETCH_HEAD and targets {}, so the diff to review is:",
            item.target_branch
        ),
        String::new(),
        format!("  git diff origin/{}...FETCH_HEAD", item.target_branch),
        String::new(),
    ];

    if open.is_empty() {
        lines.push("No open threads on it yet.".into());
        lines.push(String::new());
    } else {
        lines.push("Open threads, so you do not repeat what has already been said:".into());
        lines.push(String::new());
        for thread in open {
            lines.extend(describe_thread(thread));
        }
        lines.push(String::new());
    }

    lines.extend(
        [
            "Report every finding as exactly one line in this format, and output nothing else:",
            "",
            "  path/to/file.ext:LINE: what is wrong, and what to do about it",
            "",
            "If you find nothing worth raising, output exactly:",
            "",
            "  no findings",
        ]
        .map(String::from),
    );

    lines.join("\n")
}

/// The prompt with the resolved command name in front of it, ready to paste.
pub fn agent_command(
    item: &ReviewItem,
    threads: &[CommentThread],
    account: Option<&Account>,
    settings: &Settings,
) -> String {
    format!(
        "{} {}",
        agent_command_name(account, settings),
        single_quote(&agent_prompt(item, threads))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ApprovalOutcome, ApprovalSummary, CheckStatus, CheckSummary, MyReviewState, PullComment,
        User,
    };

    fn item(provider: ProviderKind) -> ReviewItem {
        ReviewItem {
            id: "acc:repo:88".into(),
            account_id: "acc".into(),
            provider,
            repo_key: "acme/design-tokens".into(),
            repo: "acme/design-tokens".into(),
            number: 88,
            title: "Regenerate the dark palette".into(),
            url: "https://example.test/pull/88".into(),
            author: User {
                name: "lpeters".into(),
                avatar_url: String::new(),
            },
            created_at: "2026-08-01T10:00:00Z".into(),
            updated_at: "2026-08-01T10:00:00Z".into(),
            draft: false,
            source_branch: "design/contrast-pass".into(),
            target_branch: "main".into(),
            labels: vec![],
            my_review_state: MyReviewState::Pending,
            approvals: ApprovalSummary {
                given: 0,
                required: None,
                outcome: ApprovalOutcome::NoneRequired,
            },
            checks: CheckSummary {
                status: CheckStatus::Unknown,
                passed: 0,
                failed: 0,
                running: 0,
                total: 0,
                runs: vec![],
            },
            additions: None,
            deletions: None,
            changed_files: None,
        }
    }

    fn comment(id: &str, author: &str, body: &str, created_at: &str) -> PullComment {
        PullComment {
            id: id.into(),
            author: User {
                name: author.into(),
                avatar_url: String::new(),
            },
            body: body.into(),
            created_at: created_at.into(),
        }
    }

    fn thread() -> CommentThread {
        CommentThread {
            id: "t1".into(),
            comments: vec![comment(
                "c1",
                "mnovotna",
                "Can the timeout come from config?",
                "2026-08-01T11:00:00Z",
            )],
            resolved: false,
            outdated: false,
            path: None,
            line: None,
            start_line: None,
            side: None,
            can_reply: false,
            can_resolve: false,
        }
    }

    fn account(agent_command: Option<&str>) -> Account {
        Account {
            id: "acc".into(),
            kind: ProviderKind::Github,
            label: "Work GitHub".into(),
            base_url: "https://api.github.com".into(),
            web_url: "https://github.com".into(),
            username: "lpeters".into(),
            display_name: "L Peters".into(),
            avatar_url: String::new(),
            added_at: "2026-08-01T10:00:00Z".into(),
            agent_command: agent_command.map(String::from),
        }
    }

    fn settings(agent_command: &str) -> Settings {
        Settings {
            agent_command: agent_command.into(),
            ..Settings::default()
        }
    }

    #[test]
    fn fetch_command_follows_each_host_ref_shape() {
        assert_eq!(
            fetch_command(&item(ProviderKind::Github)),
            "git fetch origin pull/88/head"
        );
        assert_eq!(
            fetch_command(&item(ProviderKind::Forgejo)),
            "git fetch origin pull/88/head"
        );
        assert_eq!(
            fetch_command(&item(ProviderKind::Gitlab)),
            "git fetch origin merge-requests/88/head"
        );
    }

    #[test]
    fn fetch_command_falls_back_to_the_source_branch_where_no_ref_shape_exists() {
        // Bitbucket publishes no ref for a pull request, so the branch name is all there is.
        assert_eq!(
            fetch_command(&item(ProviderKind::Bitbucket)),
            "git fetch origin design/contrast-pass"
        );
        assert_eq!(
            fetch_command(&ReviewItem {
                source_branch: "feature/x".into(),
                ..item(ProviderKind::Bitbucket)
            }),
            "git fetch origin feature/x"
        );
    }

    #[test]
    fn agent_command_name_prefers_the_account_override() {
        assert_eq!(
            agent_command_name(Some(&account(Some("claude-acme"))), &settings("claude")),
            "claude-acme"
        );
    }

    #[test]
    fn agent_command_name_falls_back_to_the_setting_then_to_claude_code() {
        assert_eq!(
            agent_command_name(None, &settings("my-claude")),
            "my-claude"
        );
        assert_eq!(
            agent_command_name(Some(&account(None)), &settings("my-claude")),
            "my-claude"
        );

        // A blank override must not shadow the setting, and a blank setting must not
        // produce a command that is only a quoted prompt.
        assert_eq!(
            agent_command_name(Some(&account(Some("   "))), &settings("my-claude")),
            "my-claude"
        );
        assert_eq!(
            agent_command_name(None, &settings("")),
            DEFAULT_AGENT_COMMAND
        );
        assert_eq!(Settings::default().agent_command, DEFAULT_AGENT_COMMAND);
    }

    #[test]
    fn agent_command_name_trims_what_it_keeps() {
        assert_eq!(
            agent_command_name(Some(&account(Some("  claude-acme \n"))), &settings("x")),
            "claude-acme"
        );
    }

    #[test]
    fn the_prompt_carries_the_title_the_base_branch_and_the_fetch_command() {
        let prompt = agent_prompt(&item(ProviderKind::Gitlab), &[]);

        assert!(prompt.contains("Regenerate the dark palette"));
        assert!(prompt.contains("acme/design-tokens #88"));
        assert!(prompt.contains("git fetch origin merge-requests/88/head"));
        assert!(prompt.contains("targets main"));
        assert!(prompt.contains("git diff origin/main...FETCH_HEAD"));
        assert!(prompt.contains("No open threads"));
    }

    #[test]
    fn the_prompt_lists_open_threads_and_leaves_resolved_ones_out() {
        let prompt = agent_prompt(
            &item(ProviderKind::Github),
            &[
                CommentThread {
                    id: "open".into(),
                    path: Some("src/theme.ts".into()),
                    line: Some(42),
                    ..thread()
                },
                CommentThread {
                    id: "done".into(),
                    resolved: true,
                    comments: vec![comment(
                        "c9",
                        "hkramer",
                        "Already dealt with.",
                        "2026-08-01T12:00:00Z",
                    )],
                    ..thread()
                },
            ],
        );

        assert!(prompt.contains("- src/theme.ts:42"));
        assert!(prompt.contains("mnovotna: Can the timeout come from config?"));
        assert!(!prompt.contains("Already dealt with."));
    }

    #[test]
    fn the_prompt_names_an_unanchored_thread_rather_than_pretending_it_has_a_line() {
        let prompt = agent_prompt(&item(ProviderKind::Github), &[thread()]);
        assert!(prompt.contains("- On the pull request itself"));
    }

    #[test]
    fn a_thread_with_a_path_and_no_line_is_named_by_its_path_alone() {
        let prompt = agent_prompt(
            &item(ProviderKind::Github),
            &[CommentThread {
                path: Some("src/theme.ts".into()),
                ..thread()
            }],
        );
        assert!(prompt.lines().any(|line| line == "- src/theme.ts"));
    }

    #[test]
    fn the_prompt_flattens_and_caps_a_long_comment_so_a_bot_report_cannot_swamp_it() {
        let body = format!("line one\nline two{}", " padding".repeat(200));
        let prompt = agent_prompt(
            &item(ProviderKind::Github),
            &[CommentThread {
                comments: vec![comment("c1", "dependabot", &body, "2026-08-01T11:00:00Z")],
                ..thread()
            }],
        );

        let line = prompt
            .split('\n')
            .find(|entry| entry.contains("dependabot:"))
            .expect("the comment should appear");
        let length = line.encode_utf16().count();
        assert!(length < 300, "the comment line was {length} characters");
        assert!(line.contains("dependabot: line one line two"));
    }

    #[test]
    fn the_cap_counts_like_javascript_and_never_splits_a_character() {
        // 240 units exactly fits; one more is cut to 239 and an ellipsis.
        let fits = "a".repeat(240);
        assert_eq!(one_line(&fits), fits);
        let over = "a".repeat(241);
        assert_eq!(one_line(&over), format!("{}…", "a".repeat(239)));
        // An emoji is two UTF-16 units; one straddling the cut goes whole.
        let emoji = format!("{}😀😀", "a".repeat(238));
        assert_eq!(one_line(&emoji), format!("{}…", "a".repeat(238)));
        // Multi-byte text is cut on a character boundary.
        let accented = "é".repeat(300);
        assert_eq!(one_line(&accented), format!("{}…", "é".repeat(239)));
    }

    #[test]
    fn the_prompt_asks_for_findings_one_path_and_line_prefixed_line_at_a_time() {
        let prompt = agent_prompt(&item(ProviderKind::Github), &[]);
        assert!(prompt.contains("exactly one line"));
        assert!(prompt.contains("path/to/file.ext:LINE:"));
        assert!(prompt.contains("output nothing else"));
        assert!(prompt.contains("no findings"));
    }

    #[test]
    fn single_quote_survives_an_apostrophe_in_the_prompt() {
        assert_eq!(single_quote("plain"), "'plain'");
        assert_eq!(single_quote("don't"), r"'don'\''t'");
    }

    #[test]
    fn agent_command_is_the_resolved_name_applied_to_the_quoted_prompt() {
        let command = agent_command(
            &ReviewItem {
                title: "Don't drop the retry".into(),
                ..item(ProviderKind::Github)
            },
            &[],
            Some(&account(Some("claude-acme"))),
            &settings("claude"),
        );

        assert!(command.starts_with("claude-acme '"), "{}", &command[..40]);
        assert!(command.ends_with('\''));
        // The apostrophe in the title must not close the quoting early.
        assert!(command.contains(r"Don'\''t drop the retry"));
    }
}
