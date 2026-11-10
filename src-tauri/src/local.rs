//! Local clones and worktrees: finds a repo's clone under the configured roots,
//! looks up the worktree checked out on a PR branch, and creates one with worktrunk.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    /// Main clone of the repo, if one was found under the clone roots.
    pub clone: Option<String>,
    /// Worktree checked out on the PR branch, or mid-rebase of it (may be the main clone itself).
    pub path: Option<String>,
    /// Whether worktrunk (`wt`) is installed.
    pub wt: bool,
}

/// GUI apps on macOS start with a minimal PATH; add the usual install locations.
pub fn command(program: &str) -> Command {
    let home = dirs::home_dir().unwrap_or_default();
    let extra = [
        "/opt/homebrew/bin".into(),
        "/usr/local/bin".into(),
        home.join(".local/bin"),
        home.join(".cargo/bin"),
    ];
    let current = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(extra.into_iter().chain(std::env::split_paths(&current)))
        .unwrap_or(current);
    let mut cmd = Command::new(program);
    cmd.env("PATH", path).kill_on_drop(true);
    cmd
}

pub fn expand(root: &str) -> PathBuf {
    match root.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(root),
    }
}

/// `owner/repo` from a GitHub remote URL (ssh, scp-like or https).
fn repo_from_url(url: &str) -> Option<String> {
    let (_, rest) = url.trim().split_once("github.com")?;
    let rest = rest.trim_start_matches([':', '/']).trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, name) = rest.split_once('/')?;
    (!owner.is_empty() && !name.is_empty() && !name.contains('/'))
        .then(|| format!("{owner}/{name}"))
}

/// `url` of `[remote "origin"]` in a `.git/config`.
fn origin_url(git_config: &str) -> Option<&str> {
    let mut in_origin = false;
    for line in git_config.lines().map(str::trim) {
        if line.starts_with('[') {
            in_origin = line == r#"[remote "origin"]"#;
        } else if in_origin
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            return Some(value.trim());
        }
    }
    None
}

/// Main clone of `repo` among the direct children of `roots` (linked worktrees have a `.git` file, not a dir).
fn find_clone(roots: &[String], repo: &str) -> Option<PathBuf> {
    roots
        .iter()
        .filter_map(|root| std::fs::read_dir(expand(root)).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .find(|dir| {
            std::fs::read_to_string(dir.join(".git/config"))
                .ok()
                .and_then(|cfg| origin_url(&cfg).and_then(repo_from_url))
                .is_some_and(|r| r.eq_ignore_ascii_case(repo))
        })
}

/// Path of the worktree on `branch`, from `git worktree list --porcelain`.
fn parse_worktrees(porcelain: &str, branch: &str) -> Option<String> {
    let wanted = format!("refs/heads/{branch}");
    porcelain.split("\n\n").find_map(|block| {
        let path = block.lines().find_map(|l| l.strip_prefix("worktree "))?;
        let on_branch = block.lines().any(|l| {
            l.strip_prefix("branch ") == Some(wanted.as_str())
                // A rebase detaches HEAD, but git still counts the branch as checked out there.
                || l == "detached" && rebasing_branch(path).as_deref() == Some(wanted.as_str())
        });
        on_branch.then(|| path.to_string())
    })
}

/// Branch being rebased in a worktree (`refs/heads/…`), from `head-name` in its git dir.
pub fn rebasing_branch(worktree: &str) -> Option<String> {
    let dot_git = Path::new(worktree).join(".git");
    // A linked worktree's `.git` is a file pointing at its git dir, maybe relative to the worktree.
    let git_dir = match std::fs::read_to_string(&dot_git) {
        Ok(file) => Path::new(worktree).join(file.strip_prefix("gitdir:")?.trim()),
        Err(_) => dot_git,
    };
    ["rebase-merge", "rebase-apply"]
        .into_iter()
        .find_map(|dir| std::fs::read_to_string(git_dir.join(dir).join("head-name")).ok())
        .map(|name| name.trim().to_string())
}

/// Checked once: installing worktrunk needs an app restart to show up.
async fn wt_installed() -> bool {
    static WT: tokio::sync::OnceCell<bool> = tokio::sync::OnceCell::const_new();
    *WT.get_or_init(|| async {
        command("wt")
            .arg("--version")
            .output()
            .await
            .is_ok_and(|o| o.status.success())
    })
    .await
}

pub async fn lookup(roots: &[String], repo: &str, branch: &str) -> Result<Worktree> {
    let wt = wt_installed().await;
    let Some(clone) = find_clone(roots, repo) else {
        return Ok(Worktree {
            wt,
            ..Default::default()
        });
    };
    let out = command("git")
        .arg("-C")
        .arg(&clone)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .await
        .context("running git")?;
    Ok(Worktree {
        path: parse_worktrees(&String::from_utf8_lossy(&out.stdout), branch),
        clone: Some(clone.display().to_string()),
        wt,
    })
}

/// `wt switch pr:<number>` in the repo's clone, running the user's worktrunk hooks.
pub async fn create(roots: &[String], repo: &str, number: u64) -> Result<()> {
    let clone = find_clone(roots, repo).with_context(|| format!("no local clone of {repo}"))?;
    let out = command("wt")
        .arg("-C")
        .arg(&clone)
        .args(["switch", &format!("pr:{number}"), "--no-cd", "--yes"])
        .output()
        .await
        .context("running worktrunk (`wt`)")?;
    if !out.status.success() {
        bail!(
            "wt switch failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Runs `args` through the login shell (for the user's PATH) without shell-parsing them.
pub fn login_command(args: &[String]) -> Command {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let mut cmd = command(&shell);
    cmd.args(["-lc", r#"exec "$@""#, "ghreviewer"]).args(args);
    cmd
}

/// Quotes `s` as one word for a POSIX shell.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Replaces `{key}` placeholders in each argument of a command template.
pub fn fill(template: &[String], vars: &[(&str, &str)]) -> Vec<String> {
    template
        .iter()
        .map(|arg| {
            vars.iter().fold(arg.clone(), |acc, (k, v)| {
                acc.replace(&format!("{{{k}}}"), v)
            })
        })
        .collect()
}

/// Runs a user `open_with` command through the login shell (for the user's PATH),
/// passing the arguments untouched so paths never get shell-parsed.
pub async fn open_with(template: &[String], vars: &[(&str, &str)]) -> Result<()> {
    let args = fill(template, vars);
    let program = args.first().context("command is empty")?;
    let child = login_command(&args)
        .kill_on_drop(false)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("running {program}"))?;
    // Report failures of quick commands; leave long-running ones (e.g. `code --wait`) alone.
    let wait = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait_with_output());
    match wait.await {
        Ok(out) => {
            let out = out?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                bail!("{program} failed: {}", stderr.trim());
            }
            Ok(())
        }
        Err(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_urls() {
        for url in [
            "git@github.com:Acme/api.git",
            "https://github.com/Acme/api",
            "https://github.com/Acme/api.git/",
            "ssh://git@github.com/Acme/api.git",
        ] {
            assert_eq!(repo_from_url(url).as_deref(), Some("Acme/api"), "{url}");
        }
        assert_eq!(repo_from_url("git@gitlab.com:acme/api.git"), None);
        assert_eq!(repo_from_url("https://github.com/acme"), None);
    }

    #[test]
    fn origin_from_git_config() {
        let cfg = "[core]\n\turl = nope\n[remote \"upstream\"]\n\turl = git@github.com:up/x.git\n\
                   [remote \"origin\"]\n\tfetch = +refs/heads/*\n\turl = git@github.com:me/x.git\n";
        assert_eq!(origin_url(cfg), Some("git@github.com:me/x.git"));
        assert_eq!(origin_url("[core]\n"), None);
    }

    #[test]
    fn fills_placeholders() {
        let t = ["sh", "-c", "x", "{name}", "{path}/{path}", "{unknown}"].map(String::from);
        let out = fill(&t, &[("path", "/a b"), ("name", "a b")]);
        assert_eq!(out, ["sh", "-c", "x", "a b", "/a b//a b", "{unknown}"]);
    }

    #[tokio::test]
    async fn open_with_runs_and_reports_failures() {
        let dir = std::env::temp_dir().join(format!("ghreviewer-open-{}", std::process::id()));
        let marker = dir.join("it's \"here\"");
        std::fs::create_dir_all(&dir).unwrap();
        let touch = ["touch", "{path}"].map(String::from);
        open_with(&touch, &[("path", marker.to_str().unwrap())])
            .await
            .unwrap();
        assert!(marker.exists());
        let fail = ["sh", "-c", "echo nope >&2; exit 3"].map(String::from);
        let err = open_with(&fail, &[]).await.unwrap_err().to_string();
        assert!(err.contains("nope"), "{err}");
        assert!(open_with(&[], &[]).await.is_err());
        let quoted = [
            "sh",
            "-c",
            &format!("test -e {}", shell_quote(marker.to_str().unwrap())),
        ];
        open_with(&quoted.map(String::from), &[]).await.unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn worktree_on_branch() {
        let porcelain = "worktree /dev/ti\nHEAD abc\nbranch refs/heads/staging\n\n\
                         worktree /dev/ti.feat\nHEAD def\nbranch refs/heads/feat/x\n\n\
                         worktree /dev/ti.detached\nHEAD 123\ndetached\n";
        assert_eq!(
            parse_worktrees(porcelain, "feat/x").as_deref(),
            Some("/dev/ti.feat")
        );
        assert_eq!(
            parse_worktrees(porcelain, "staging").as_deref(),
            Some("/dev/ti")
        );
        assert_eq!(parse_worktrees(porcelain, "feat"), None);
    }

    #[test]
    fn worktree_mid_rebase_stays_on_its_branch() {
        let root = std::env::temp_dir().join(format!("ghreviewer-rebase-{}", std::process::id()));
        let git_dir = root.join("ti/.git/worktrees/ti.feat");
        std::fs::create_dir_all(git_dir.join("rebase-merge")).unwrap();
        std::fs::write(
            git_dir.join("rebase-merge/head-name"),
            "refs/heads/feat/x\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("ti.feat")).unwrap();
        // Relative, as with `worktree.useRelativePaths`.
        std::fs::write(
            root.join("ti.feat/.git"),
            "gitdir: ../ti/.git/worktrees/ti.feat\n",
        )
        .unwrap();
        let wt = root.join("ti.feat").display().to_string();
        let porcelain = format!("worktree {wt}\nHEAD abc\ndetached\n");
        assert_eq!(parse_worktrees(&porcelain, "feat/x"), Some(wt));
        assert_eq!(parse_worktrees(&porcelain, "feat/y"), None);
        assert_eq!(
            rebasing_branch(&root.join("ti").display().to_string()),
            None
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn finds_clone_by_origin() {
        let root = std::env::temp_dir().join(format!("ghreviewer-clones-{}", std::process::id()));
        let git = root.join("lens-ai-demo/.git");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(
            git.join("config"),
            "[remote \"origin\"]\n\turl = git@github.com:ti/lens-ai.git\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("other")).unwrap();
        let roots = [root.display().to_string()];
        assert_eq!(
            find_clone(&roots, "TI/lens-ai"),
            Some(root.join("lens-ai-demo"))
        );
        assert_eq!(find_clone(&roots, "ti/other"), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(test)]
mod live {
    /// Real clones under ~/Documents/dev: `cargo test -- --ignored live_worktree --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_worktree() {
        let roots = ["~/Documents/dev".to_string()];
        let t = std::time::Instant::now();
        let w = super::lookup(&roots, "thoughtindustries/ti", "feat/rbac-superadmin")
            .await
            .unwrap();
        println!("{w:?} in {:?}", t.elapsed());
        let w = super::lookup(&roots, "thoughtindustries/lens-ai", "nope")
            .await
            .unwrap();
        println!("{w:?}");
    }
}
