//! Runs a configured prompt headless with a coding agent (opencode or Claude Code) and
//! turns its JSON event stream into a `Run`: session id, tool activity and final answer.

use crate::config::{Harness, Prompt};
use crate::local::{expand, login_command, shell_quote};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    #[default]
    Running,
    Done,
    Error,
    Cancelled,
    /// The app quit while it was running.
    Interrupted,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Run {
    pub pr_id: String,
    pub repo: String,
    pub number: u64,
    pub prompt: String,
    pub harness: Harness,
    /// Whether its finished result was looked at; until then the sidebar lists it.
    pub seen: bool,
    pub status: RunStatus,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    /// PR head commit when the run started, to flag results as stale after new pushes.
    pub head_oid: Option<String>,
    /// Worktree the agent ran in.
    pub path: Option<String>,
    pub session_id: Option<String>,
    /// One line per tool call.
    pub activity: Vec<String>,
    /// Text of the latest agent message that had text: the answer once done.
    pub output: String,
    pub error: Option<String>,
    /// Existing .html/.svg/.png… files the answer mentions.
    pub files: Vec<String>,
    /// Findings the agent returned in a ```ghreviewer block, to pick and post from the app.
    pub review: Option<Review>,
    /// Message the current `output` comes from.
    #[serde(skip)]
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Review {
    /// Opening text of the GitHub review.
    pub summary: String,
    pub findings: Vec<Finding>,
    /// URL of the review once posted from the app.
    pub posted: Option<String>,
    /// GitHub rejected the inline comments, so every finding went into the review body.
    pub all_in_body: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Finding {
    pub severity: String,
    pub title: String,
    pub body: String,
    pub path: Option<String>,
    pub line: Option<u64>,
}

/// What `{review_format}` in a prompt expands to; `take_review` parses the result.
pub const REVIEW_FORMAT: &str = r#"End your final answer with your findings as a fenced code block tagged `ghreviewer` holding JSON:
```ghreviewer
{"summary": "optional opening line of the GitHub review",
 "findings": [{"severity": "blocker | should-fix | nit", "title": "one line",
   "body": "Markdown comment to the PR author, with the proposed fix",
   "path": "path/in/repo.rs", "line": 42}]}
```
`path` and `line` are optional. Give them only for a line of the new version of the file that the PR diff shows: it becomes an inline comment there. The user picks which findings to post from the app, so don't post anything on GitHub yourself."#;

const FENCE: &str = "```ghreviewer";

/// What `{findings}` in the fix prompt expands to.
pub fn findings_text(findings: &[Finding]) -> String {
    let one = |(i, f): (usize, &Finding)| {
        let mut head = format!("## {}. {}", i + 1, f.title);
        if !f.severity.is_empty() {
            head += &format!(" ({})", f.severity);
        }
        let place = match (&f.path, f.line) {
            (Some(path), Some(line)) => format!("\n`{path}:{line}`"),
            (Some(path), None) => format!("\n`{path}`"),
            _ => String::new(),
        };
        format!("{head}{place}\n\n{}", f.body.trim())
    };
    let all: Vec<String> = findings.iter().enumerate().map(one).collect();
    all.join("\n\n")
}

const MAX_ACTIVITY: usize = 500;

fn argv(spec: &Prompt, out: &Path, title: &str, base: &str, prompt: &str) -> Vec<String> {
    let mut args: Vec<String> = match spec.harness {
        // --agent: never prompts or opens a browser to sign in.
        Harness::Greptile => {
            let mut a = vec!["greptile", "review", "--json", "--agent", "-b", base];
            if !prompt.trim().is_empty() {
                a.extend(["--instructions", prompt.trim()]);
            }
            a
        }
        Harness::Opencode => vec!["opencode", "run", "--format", "json", "--title", title],
        Harness::Claude => vec![
            "claude",
            "-p",
            "--add-dir",
            out.to_str().unwrap_or_default(),
            "--output-format",
            "stream-json",
            "--verbose",
        ],
    }
    .into_iter()
    .map(Into::into)
    .collect();
    args.extend(spec.args.iter().cloned());
    args
}

/// Shell command line that reopens the run's session interactively.
pub fn resume_command(run: &Run) -> Option<String> {
    let (path, id) = (
        run.path.as_deref()?,
        shell_quote(run.session_id.as_deref()?),
    );
    let resume = match run.harness {
        Harness::Opencode => format!("opencode -s {id}"),
        Harness::Claude => format!("claude --resume {id}"),
        Harness::Greptile => return None,
    };
    Some(format!("cd {} && {resume}", shell_quote(path)))
}

/// Short description of a tool call, e.g. `shell git diff main`.
fn describe(tool: &str, input: &Value) -> String {
    let keys = [
        "command",
        "filePath",
        "file_path",
        "path",
        "pattern",
        "url",
        "query",
        "description",
        "id",
        "skill",
    ];
    let detail = keys
        .iter()
        .find_map(|k| input.get(k)?.as_str())
        .unwrap_or("");
    let line = format!("{tool} {}", detail.lines().next().unwrap_or(""));
    line.chars().take(160).collect()
}

impl Run {
    fn text(&mut self, message: &str, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if self.message != message {
            self.message = message.into();
            self.output.clear();
        }
        if !self.output.is_empty() {
            self.output.push_str("\n\n");
        }
        self.output.push_str(text);
    }

    /// Moves the last ```ghreviewer block of the answer into `review`. A broken block stays
    /// in the answer with a note, so the findings aren't lost.
    fn take_review(&mut self) {
        let Some(start) = self.output.rfind(FENCE) else {
            return;
        };
        let after = &self.output[start + FENCE.len()..];
        // JSON strings can't hold raw newlines, so a line starting with ``` closes the block.
        let (json, end) = match after.find("\n```") {
            Some(i) => (&after[..i], start + FENCE.len() + i + 4),
            None => (after, self.output.len()),
        };
        match serde_json::from_str::<Review>(json) {
            Ok(review) => {
                self.review = Some(Review {
                    posted: None,
                    all_in_body: false,
                    ..review
                });
                self.output.replace_range(start..end, "");
                self.output.truncate(self.output.trim_end().len());
            }
            Err(e) => self
                .output
                .push_str(&format!("\n\n> ghreviewer block ignored: {e}")),
        }
    }

    /// Turns the JSON `greptile review --json` printed into the run's review and answer.
    fn take_greptile(&mut self) -> Result<()> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Out {
            #[serde(default)]
            summary: String,
            confidence: Option<u8>,
            #[serde(default)]
            confidence_reasoning: String,
            security_summary: Option<String>,
            #[serde(default)]
            comments: Vec<Comment>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Comment {
            path: Option<String>,
            end_line: Option<u64>,
            side: Option<String>,
            #[serde(default)]
            severity: String,
            #[serde(default)]
            security_issue: bool,
            #[serde(default)]
            body: String,
            suggestion: Option<Value>,
        }
        if self.error.is_some() {
            return Ok(()); // failed: what it printed isn't a review
        }
        let out: Out = serde_json::from_str(&self.output).context("greptile's JSON")?;
        let findings = out.comments.into_iter().map(|c| {
            // The body opens with the title in bold; else its first line is the title.
            let body = c.body.trim();
            let bold = body.strip_prefix("**").and_then(|b| b.split_once("**"));
            let (title, rest) = bold.unwrap_or(body.split_once('\n').unwrap_or((body, "")));
            let (title, mut body) = (title.trim().to_string(), rest.trim().to_string());
            if let Some(Value::String(s)) = c.suggestion {
                body += &format!("\n\nSuggested change:\n```\n{}\n```", s.trim_end());
            }
            let severity = [
                c.severity.as_str(),
                if c.security_issue { "security" } else { "" },
            ];
            Finding {
                severity: severity.join(" ").trim().into(),
                title,
                body,
                // Only lines of the new version can carry an inline comment.
                line: c.end_line.filter(|_| c.side.as_deref() != Some("old")),
                path: c.path,
            }
        });
        self.review = Some(Review {
            summary: out.summary,
            findings: findings.collect(),
            ..Default::default()
        });
        let mut notes = Vec::new();
        if let Some(n) = out.confidence {
            notes.push(format!(
                "**Confidence {n}/5.** {}",
                out.confidence_reasoning
            ));
        }
        notes.extend(out.security_summary.map(|s| format!("**Security.** {s}")));
        self.output = notes.join("\n\n");
        Ok(())
    }

    fn tool(&mut self, line: String) {
        if self.activity.len() < MAX_ACTIVITY {
            self.activity.push(line);
        }
    }

    /// Applies one line of the harness's JSON output. Returns whether anything shown changed.
    pub fn apply(&mut self, line: &str) -> bool {
        // One JSON document over many lines, read once the review is done (`take_greptile`).
        if self.harness == Harness::Greptile {
            self.output.push_str(line);
            self.output.push('\n');
            return false;
        }
        let Ok(ev) = serde_json::from_str::<Value>(line) else {
            return false;
        };
        let str_at = |p: &str| ev.pointer(p).and_then(Value::as_str);
        let before = (self.activity.len(), self.output.len(), self.error.is_some());
        let session = str_at("/sessionID").or(str_at("/session_id"));
        if self.session_id.is_none() {
            self.session_id = session.map(Into::into);
        }
        match (self.harness, str_at("/type").unwrap_or("")) {
            (Harness::Opencode, "text") => {
                let msg = str_at("/part/messageID").unwrap_or("").to_string();
                self.text(&msg, str_at("/part/text").unwrap_or(""));
            }
            (Harness::Opencode, "tool_use") => {
                let input = ev.pointer("/part/state/input").cloned().unwrap_or_default();
                self.tool(describe(str_at("/part/tool").unwrap_or("?"), &input));
            }
            (Harness::Opencode, "error") => {
                let msg = str_at("/error/data/message").or(str_at("/error/message"));
                self.error = Some(msg.map_or_else(|| ev["error"].to_string(), Into::into));
            }
            (Harness::Claude, "assistant") => {
                let msg = str_at("/message/id").unwrap_or("").to_string();
                let blocks = ev.pointer("/message/content").and_then(Value::as_array);
                for block in blocks.into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => self.text(&msg, block["text"].as_str().unwrap_or("")),
                        Some("tool_use") => self.tool(describe(
                            block["name"].as_str().unwrap_or("?"),
                            &block["input"],
                        )),
                        _ => {}
                    }
                }
            }
            (Harness::Claude, "result") => {
                let result = str_at("/result").unwrap_or("");
                if ev["is_error"].as_bool() == Some(true) {
                    self.error = Some(result.into());
                } else if !result.trim().is_empty() {
                    self.output = result.into();
                }
                let denied = ev["permission_denials"].as_array().into_iter().flatten();
                for d in denied {
                    let tool = d["tool_name"].as_str().unwrap_or("?");
                    self.tool(format!("permission denied: {tool}"));
                }
            }
            _ => {}
        }
        before != (self.activity.len(), self.output.len(), self.error.is_some())
    }
}

/// Runs `prompt` with the harness in `cwd`, calling `on_update` whenever the run changes.
/// Sets `output`, `session_id`, `activity`, `error` and `files`; the caller sets the status.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    spec: &Prompt,
    cwd: &Path,
    out: &Path,
    title: &str,
    base: &str,
    prompt: &str,
    run: &mut Run,
    on_update: impl FnMut(&Run),
) -> Result<()> {
    let argv = argv(spec, out, title, base, prompt);
    if spec.harness == Harness::Greptile {
        run_argv(&argv, cwd, "", run, on_update).await?;
        return run.take_greptile();
    }
    run_argv(&argv, cwd, prompt, run, on_update).await?;
    run.take_review();
    run.files = files_in(out);
    for f in mentioned_files(&run.output, cwd) {
        if !run.files.contains(&f) {
            run.files.push(f);
        }
    }
    Ok(())
}

fn shown(path: &str) -> bool {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_lowercase());
    ext.is_some_and(|e| SHOWN.contains(&e.as_str()))
}

/// Viewable files directly in `dir`, by name.
fn files_in(dir: &Path) -> Vec<String> {
    let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
    let mut files: Vec<String> = entries
        .map(|e| e.path().display().to_string())
        .filter(|p| shown(p) && Path::new(p).is_file())
        .collect();
    files.sort();
    files
}

async fn run_argv(
    argv: &[String],
    cwd: &Path,
    prompt: &str,
    run: &mut Run,
    mut on_update: impl FnMut(&Run),
) -> Result<()> {
    let program = &argv[0];
    let mut child = login_command(argv)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running {program}"))?;
    let mut stdin = child.stdin.take().context("no stdin")?;
    stdin.write_all(prompt.as_bytes()).await?;
    drop(stdin);
    let mut stderr = child.stderr.take().context("no stderr")?;
    let mut lines = BufReader::new(child.stdout.take().context("no stdout")?).lines();
    let read_err = async {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        s
    };
    let read_out = async {
        while let Some(line) = lines.next_line().await? {
            if run.apply(&line) {
                on_update(run);
            }
        }
        anyhow::Ok(())
    };
    let (stderr, res) = tokio::join!(read_err, read_out);
    res?;
    let status = child.wait().await?;
    if !status.success() && run.error.is_none() {
        let tail: Vec<&str> = stderr.trim().lines().rev().take(5).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        run.error = Some(format!(
            "{program} exited with {status}: {}",
            tail.join("\n")
        ));
    }
    Ok(())
}

const SHOWN: [&str; 8] = ["html", "htm", "svg", "png", "jpg", "jpeg", "gif", "webp"];

/// Existing files with a viewable extension mentioned in `text`, absolute or relative to `base`.
/// Paths with spaces are only found inside backticks or quotes.
fn mentioned_files(text: &str, base: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let words = text.split(|c: char| c.is_whitespace() || "`'\"()[]<>*|".contains(c));
    let quoted = text.split('`').chain(text.split('"'));
    for token in words.chain(quoted) {
        let token = token
            .trim()
            .trim_end_matches(['.', ',', ':', ';', '!', '?']);
        let token = token.strip_prefix("file://").unwrap_or(token);
        if token.contains('\n') || !shown(token) {
            continue;
        }
        let path = base.join(expand(token)); // a no-op join for absolute paths
        let path = path.display().to_string();
        if Path::new(&path).is_file() && !found.contains(&path) {
            found.push(path);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(harness: Harness) -> Prompt {
        Prompt {
            name: "review".into(),
            harness,
            prompt: "x".into(),
            args: vec!["--model".into(), "m".into()],
        }
    }

    #[test]
    fn findings_for_the_fix_prompt() {
        let f = |title: &str, path: Option<&str>, line: Option<u64>| Finding {
            severity: if line.is_some() {
                "P1".into()
            } else {
                String::new()
            },
            title: title.into(),
            body: " Do it.\n".into(),
            path: path.map(Into::into),
            line,
        };
        let text = findings_text(&[f("Leak", Some("a.rs"), Some(3)), f("Docs", None, None)]);
        assert_eq!(
            text,
            "## 1. Leak (P1)\n`a.rs:3`\n\nDo it.\n\n## 2. Docs\n\nDo it."
        );
    }

    #[test]
    fn opencode_events() {
        let mut run = Run::default();
        let lines = [
            r#"{"type":"step_start","sessionID":"ses_1","part":{"messageID":"m1"}}"#,
            r#"{"type":"text","sessionID":"ses_1","part":{"messageID":"m1","text":"Let me look."}}"#,
            r#"{"type":"tool_use","sessionID":"ses_1","part":{"messageID":"m1","tool":"shell","state":{"input":{"command":"git diff main\nmore"}}}}"#,
            r#"{"type":"text","sessionID":"ses_1","part":{"messageID":"m2","text":"Review"}}"#,
            r#"{"type":"text","sessionID":"ses_1","part":{"messageID":"m2","text":"LGTM"}}"#,
            "not json",
        ];
        let changed: Vec<bool> = lines.iter().map(|l| run.apply(l)).collect();
        assert_eq!(changed, [false, true, true, true, true, false]);
        assert_eq!(run.session_id.as_deref(), Some("ses_1"));
        assert_eq!(run.activity, ["shell git diff main"]);
        assert_eq!(run.output, "Review\n\nLGTM");
        run.apply(r#"{"type":"error","error":{"name":"X","data":{"message":"boom"}}}"#);
        assert_eq!(run.error.as_deref(), Some("boom"));
    }

    #[test]
    fn claude_events() {
        let mut run = Run {
            harness: Harness::Claude,
            ..Default::default()
        };
        for l in [
            r#"{"type":"system","subtype":"init","session_id":"abc"}"#,
            r#"{"type":"assistant","session_id":"abc","message":{"id":"m1","content":[{"type":"text","text":"Looking"},{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"assistant","session_id":"abc","message":{"id":"m2","content":[{"type":"text","text":"Draft"}]}}"#,
            r#"{"type":"result","session_id":"abc","is_error":false,"result":"Final","permission_denials":[{"tool_name":"Write"}]}"#,
        ] {
            run.apply(l);
        }
        assert_eq!(run.session_id.as_deref(), Some("abc"));
        assert_eq!(run.activity, ["Bash ls", "permission denied: Write"]);
        assert_eq!(run.output, "Final");
        run.apply(r#"{"type":"result","is_error":true,"result":"over quota"}"#);
        assert_eq!(run.error.as_deref(), Some("over quota"));
    }

    #[test]
    fn takes_the_review_block() {
        let block = r#"```ghreviewer
{"summary": "s", "posted": "x", "findings": [
  {"severity": "blocker", "title": "t", "body": "use\n```rust\nfoo()\n```", "path": "a.rs", "line": 3},
  {"title": "u", "body": "b"}]}
```"#;
        let mut run = Run {
            output: format!("Review\n\n```ghreviewer\nold\n```\n\n{block}\n"),
            ..Default::default()
        };
        run.take_review();
        assert_eq!(run.output, "Review\n\n```ghreviewer\nold\n```");
        let review = run.review.unwrap();
        assert_eq!((review.summary.as_str(), review.posted), ("s", None));
        let f = &review.findings;
        assert_eq!(f[0].body, "use\n```rust\nfoo()\n```");
        assert_eq!((f[0].path.as_deref(), f[0].line), (Some("a.rs"), Some(3)));
        assert_eq!((f[1].path.as_deref(), f[1].severity.as_str()), (None, ""));

        let mut broken = Run {
            output: "Review\n```ghreviewer\n{nope\n```".into(),
            ..Default::default()
        };
        broken.take_review();
        assert!(broken.review.is_none());
        assert!(
            broken
                .output
                .contains("{nope\n```\n\n> ghreviewer block ignored")
        );

        let mut none = Run {
            output: "LGTM".into(),
            ..Default::default()
        };
        none.take_review();
        assert!(none.review.is_none() && none.output == "LGTM");
    }

    #[test]
    fn greptile_review() {
        let mut run = Run {
            harness: Harness::Greptile,
            ..Default::default()
        };
        let json = r#"{"summary":"Moves secrets.","confidence":4,"confidenceReasoning":"Safe.",
            "securitySummary":null,"comments":[
            {"path":"a.mjs","startLine":124,"endLine":150,"side":"new","severity":"P2",
             "securityIssue":true,"body":"**Lacks tests**\n\nAdd some.","suggestion":"x = 1"},
            {"path":"b.rs","endLine":3,"side":"old","severity":"P1","body":"No title\nhere"}]}"#;
        for line in json.lines() {
            assert!(!run.apply(line));
        }
        run.take_greptile().unwrap();
        let review = run.review.unwrap();
        assert_eq!(review.summary, "Moves secrets.");
        let f = &review.findings;
        assert_eq!(
            (f[0].severity.as_str(), f[0].title.as_str()),
            ("P2 security", "Lacks tests")
        );
        assert_eq!(f[0].body, "Add some.\n\nSuggested change:\n```\nx = 1\n```");
        assert_eq!(
            (f[0].path.as_deref(), f[0].line),
            (Some("a.mjs"), Some(150))
        );
        assert_eq!(
            (f[1].title.as_str(), f[1].body.as_str()),
            ("No title", "here")
        );
        assert_eq!(f[1].line, None);
        assert_eq!(run.output, "**Confidence 4/5.** Safe.");
    }

    #[test]
    fn harness_argv_and_resume() {
        let out = Path::new("/o");
        assert_eq!(
            argv(&spec(Harness::Opencode), out, "t", "main", "x"),
            [
                "opencode", "run", "--format", "json", "--title", "t", "--model", "m"
            ]
        );
        assert_eq!(
            argv(&spec(Harness::Claude), out, "t", "main", "x")[..4],
            ["claude", "-p", "--add-dir", "/o"]
        );
        assert_eq!(
            argv(&spec(Harness::Greptile), out, "t", "dev", " be brief ")[4..],
            ["-b", "dev", "--instructions", "be brief", "--model", "m"]
        );
        let mut run = Run {
            path: Some("/w/it's".into()),
            session_id: Some("ses_1".into()),
            ..Default::default()
        };
        assert_eq!(
            resume_command(&run).unwrap(),
            r"cd '/w/it'\''s' && opencode -s 'ses_1'"
        );
        run.session_id = None;
        assert!(resume_command(&run).is_none());
    }

    #[test]
    fn finds_mentioned_files() {
        let dir = std::env::temp_dir().join(format!("ghreviewer-files-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        for f in ["docs/d.html", "a.svg", "notes.md", "with space.png"] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        let abs = dir.join("a.svg").display().to_string();
        let spaced = dir.join("with space.png").display().to_string();
        let text = format!(
            "Wrote `docs/d.html` and [svg]({abs}). See file://{abs}, notes.md, missing.png.\n\
             - `{spaced}`: an image"
        );
        let rel = dir.join("docs/d.html").display().to_string();
        assert_eq!(
            mentioned_files(&text, &dir),
            [rel, abs.clone(), spaced.clone()]
        );
        assert_eq!(files_in(&dir), [abs, spaced]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn runs_a_process_and_reports_failure() {
        let dir = std::env::temp_dir();
        let script = r#"read p; echo "{\"type\":\"text\",\"sessionID\":\"s\",\"part\":{\"messageID\":\"m\",\"text\":\"got $p\"}}"; echo oops >&2; exit 2"#;
        let argv = ["sh", "-c", script].map(String::from);
        let mut run = Run::default();
        let mut updates = 0;
        run_argv(&argv, &dir, "hello\n", &mut run, |_| updates += 1)
            .await
            .unwrap();
        assert_eq!((run.output.as_str(), updates), ("got hello", 1));
        let err = run.error.unwrap();
        assert!(err.contains("oops") && err.contains("sh exited"), "{err}");
    }
}
