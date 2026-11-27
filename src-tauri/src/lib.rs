//! The desktop app: Tauri commands and protocols over `ghreviewer_core`.

use ghreviewer_core::{
    self as backend, AppState, Event, Status, Target, agent::Finding, agent::Run, config::Config,
    config::Settings, err, jira, local, sync::PrSummary, sync::Repo,
};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use tauri::{AppHandle, Emitter, Manager, State};

type Res<T> = Result<T, String>;
type St<'a> = State<'a, Arc<AppState>>;

#[tauri::command]
fn get_status(st: St<'_>) -> Status {
    st.status.lock().unwrap().clone()
}

#[tauri::command]
fn get_config(st: St<'_>) -> Config {
    st.cfg.read().unwrap().clone()
}

#[tauri::command]
fn list_repos(st: St<'_>) -> Res<Vec<Repo>> {
    backend::list_repos(&st).map_err(err)
}

#[tauri::command]
fn list_inbox(st: St<'_>) -> Res<Vec<PrSummary>> {
    backend::list_inbox(&st).map_err(err)
}

/// Cached Jira tickets, for the keys the inbox gave on a PR.
#[tauri::command]
fn get_tickets(st: St<'_>, keys: Vec<String>) -> Res<Vec<jira::Ticket>> {
    st.db.tickets(&keys).map_err(err)
}

/// Returns the cached PR right away and refreshes it in the background (`pr-updated` event).
#[tauri::command]
async fn get_pr(st: St<'_>, id: String) -> Res<Value> {
    backend::get_pr(&st, id).await.map_err(err)
}

#[tauri::command]
fn set_unread(st: St<'_>, id: String, unread: bool) -> Res<()> {
    backend::set_unread(&st, &id, unread).map_err(err)
}

/// Snoozes a PR for `days`, or wakes it with `None`.
#[tauri::command]
fn snooze(st: St<'_>, id: String, days: Option<u64>) -> Res<()> {
    backend::snooze(&st, &id, days).map_err(err)
}

#[tauri::command]
fn refresh(st: St<'_>) {
    backend::refresh(&st);
}

#[tauri::command]
fn save_settings(st: St<'_>, settings: Settings) -> Res<()> {
    backend::save_settings(&st, &settings).map_err(err)
}

#[tauri::command]
fn set_repo_flag(st: St<'_>, repo: String, flag: String, on: bool) -> Res<()> {
    backend::set_repo_flag(&st, &repo, &flag, on).map_err(err)
}

/// Links the `ghr` bundled next to the app into /usr/local/bin (on every Mac's PATH), asking
/// for an admin password like VS Code's `code` command.
#[tauri::command]
async fn install_cli() -> Res<()> {
    let exe = std::env::current_exe().map_err(err)?;
    let ghr = exe.with_file_name("ghr");
    if !ghr.is_file() {
        return Err(format!(
            "no ghr next to the app ({}): build it with `mise run build`",
            ghr.display()
        ));
    }
    let quoted = ghr
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let script = format!(
        r#"do shell script "mkdir -p /usr/local/bin && ln -sf " & quoted form of "{quoted}" & " /usr/local/bin/ghr" with administrator privileges
display alert "ghr is installed" message "Run ghr in a terminal: the inbox, PRs and runs, over the same config as the app.""#
    );
    // Off the async runtime: it waits for the password prompt.
    let out = tauri::async_runtime::spawn_blocking(move || {
        std::process::Command::new("osascript")
            .args(["-e", &script])
            .output()
    })
    .await
    .map_err(err)?
    .map_err(err)?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    match out.status.success() {
        true => Ok(()),
        // Cancelling the password prompt is not an error worth showing.
        false if stderr.contains("-128") => Ok(()),
        false => Err(format!("installing ghr failed: {}", stderr.trim())),
    }
}

#[tauri::command]
fn open_config(st: St<'_>) -> Res<()> {
    std::process::Command::new("open")
        .arg("-t")
        .arg(&st.cfg_path)
        .spawn()
        .map_err(err)?;
    Ok(())
}

#[tauri::command]
async fn get_worktree(st: St<'_>, repo: String, branch: String) -> Res<local::Worktree> {
    backend::get_worktree(&st, &repo, &branch)
        .await
        .map_err(err)
}

#[tauri::command]
async fn create_worktree(st: St<'_>, repo: String, number: u64) -> Res<()> {
    backend::create_worktree(&st, &repo, number)
        .await
        .map_err(err)
}

/// Runs the `opener`-th `open_with` command on a PR's worktree.
#[tauri::command]
async fn open_worktree(
    st: St<'_>,
    opener: usize,
    path: String,
    repo: String,
    branch: String,
    number: u64,
) -> Res<()> {
    backend::open_worktree(&st, opener, &path, &repo, &branch, number)
        .await
        .map_err(err)
}

/// Latest run of each prompt on a PR, letting the webview load the files they mention.
#[tauri::command]
fn get_runs(app: AppHandle, st: St<'_>, pr_id: String) -> Res<Vec<Run>> {
    let runs = st.db.runs(&pr_id).map_err(err)?;
    let scope = app.asset_protocol_scope();
    for file in runs.iter().flat_map(|r| &r.files) {
        // The folder too, for the stylesheets and images an HTML file links to.
        if let Some(dir) = Path::new(file).parent() {
            let _ = scope.allow_directory(dir, false);
        }
    }
    Ok(runs)
}

/// Runs to list in the sidebar: in progress, or finished and not looked at yet.
#[tauri::command]
fn list_active_runs(st: St<'_>) -> Res<Vec<Run>> {
    st.db.active_runs().map_err(err)
}

#[tauri::command]
fn mark_run_seen(st: St<'_>, pr_id: String, prompt: String) -> Res<()> {
    backend::mark_run_seen(&st, &pr_id, &prompt).map_err(err)
}

/// Runs a configured prompt on a PR in the background, replacing its previous run.
#[tauri::command]
fn run_prompt(st: St<'_>, prompt: String, pr: Target) -> Res<()> {
    backend::run_prompt(&st, &prompt, pr).map_err(err)
}

/// Hands findings picked in a review to the `[fix]` agent, as a run of the `fix` prompt.
#[tauri::command]
fn fix_findings(st: St<'_>, pr: Target, findings: Vec<Finding>) -> Res<()> {
    backend::fix_findings(&st, pr, &findings).map_err(err)
}

#[tauri::command]
fn cancel_run(st: St<'_>, pr_id: String, prompt: String) -> Res<()> {
    backend::cancel_run(&st, &pr_id, &prompt).map_err(err)
}

/// Reopens a run's agent session interactively with the `terminal` command.
#[tauri::command]
async fn continue_run(st: St<'_>, pr_id: String, prompt: String) -> Res<()> {
    backend::continue_run(&st, &pr_id, &prompt)
        .await
        .map_err(err)
}

/// Posts the picked findings of a run's review to GitHub as one review.
#[tauri::command]
async fn post_review(
    st: St<'_>,
    pr_id: String,
    prompt: String,
    picked: Vec<usize>,
    approve: bool,
) -> Res<()> {
    backend::post_review(&st, &pr_id, &prompt, &picked, approve)
        .await
        .map_err(err)
}

/// Approves the commit of a PR that was on screen (`head_oid`), with an optional message, then
/// refreshes the PR to show the approval.
#[tauri::command]
async fn approve_pr(st: St<'_>, id: String, head_oid: String, body: String) -> Res<()> {
    backend::approve_pr(&st, &id, &head_oid, body)
        .await
        .map_err(err)
}

/// Posts the page's height to the app, so its iframe can show it whole.
/// Measures the body's children (with what spills out of them): the document height would follow
/// `min-height: 100vh` and grow along with the iframe forever.
/// Skips a height that grew exactly as much as the iframe: a `100vh` child follows our own resize.
/// It also hides the vertical scrollbar: `min-height: 100vh` plus padding overflows any height.
const FIT_SCRIPT: &str = r#"<script>(() => {
  document.documentElement.style.overflowY = "hidden";
  const b = document.body;
  let extra;
  const post = () => {
    let h = 0;
    for (const e of b.children) {
      const r = e.getBoundingClientRect();
      h = Math.max(h, r.bottom, r.top + e.scrollHeight);
    }
    const s = getComputedStyle(b);
    h += scrollY + parseFloat(s.paddingBottom) + parseFloat(s.borderBottomWidth) + parseFloat(s.marginBottom);
    h = Math.ceil(h);
    if (h - innerHeight === extra) return;
    extra = h - innerHeight;
    parent.postMessage({ ghreviewerHeight: h }, "*");
  };
  new ResizeObserver(post).observe(b);
  document.fonts.ready.then(post);
})();</script>"#;

/// Agent pages get no network, so a page can't send out what the agent read. Google Fonts stay:
/// diagrams use them, and a request there reaches nobody else.
const RUN_FILE_CSP: &str = "default-src 'unsafe-inline' data: blob: runfile:; \
    style-src 'unsafe-inline' runfile: https://fonts.googleapis.com; \
    font-src data: runfile: https://fonts.gstatic.com; form-action 'none'";

/// `runfile://localhost/<path>`: the asset protocol's files, with `FIT_SCRIPT` added to HTML.
fn run_file(app: &AppHandle, uri: &tauri::http::Uri) -> tauri::http::Response<Vec<u8>> {
    let path = percent_encoding::percent_decode_str(uri.path()).decode_utf8_lossy();
    let allowed = tauri::path::SafePathBuf::new(path.as_ref().into())
        .is_ok_and(|p| app.asset_protocol_scope().is_allowed(p));
    let res = tauri::http::Response::builder();
    let Some(mut body) = allowed.then(|| std::fs::read(path.as_ref()).ok()).flatten() else {
        return res.status(404).body(Vec::new()).unwrap();
    };
    let lower = path.to_lowercase();
    // Sniffing would serve `<?xml`-led pages as XML, where the appended script is a parse error.
    let mime = if lower.ends_with(".html") || lower.ends_with(".htm") {
        body.extend_from_slice(FIT_SCRIPT.as_bytes());
        "text/html".to_string()
    } else {
        tauri::utils::mime_type::MimeType::parse(&body, &path)
    };
    res.header("Content-Type", mime)
        .header("Content-Security-Policy", RUN_FILE_CSP)
        .body(body)
        .unwrap()
}

/// `ghimg://localhost/<owner>/<repo>/<ref>/<file>`: an image in a repo, fetched with the token.
/// The webview has no GitHub session, so a private repo's files are a 404 to it.
async fn gh_image(st: &AppState, path: &str) -> tauri::http::Response<Vec<u8>> {
    let res = tauri::http::Response::builder();
    match st.gh.raw(path).await {
        // Images only: the token can read any file.
        Ok((mime, body)) if mime.starts_with("image/") => {
            res.header("Content-Type", mime).body(body).unwrap()
        }
        _ => res.status(404).body(Vec::new()).unwrap(),
    }
}

/// Opens a file a run on the PR mentioned with its default app.
#[tauri::command]
fn open_path(st: St<'_>, pr_id: String, path: String) -> Res<()> {
    let runs = st.db.runs(&pr_id).map_err(err)?;
    if !runs.iter().any(|r| r.files.contains(&path)) {
        return Err("not a file from this PR's runs".into());
    }
    std::process::Command::new("open")
        .arg(&path)
        .spawn()
        .map_err(err)?;
    Ok(())
}

/// Forwards a core event to the webview, under the names `src/api.ts` listens to.
fn forward(app: &AppHandle, e: Event) {
    let _ = match e {
        Event::Changed => app.emit("changed", ()),
        Event::Status => app.emit("status", ()),
        Event::PrUpdated { id, detail } => {
            app.emit("pr-updated", json!({ "id": id, "detail": detail }))
        }
        Event::RunUpdated(pr_id) => app.emit("run-updated", pr_id),
        Event::WorktreeCreated => app.emit("worktree-created", ()),
    };
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .register_asynchronous_uri_scheme_protocol("runfile", |ctx, req, responder| {
            // Off the main thread: a page can link big files.
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                responder.respond(run_file(&app, req.uri()))
            });
        })
        .register_asynchronous_uri_scheme_protocol("ghimg", |ctx, req, responder| {
            let st = ctx.app_handle().state::<Arc<AppState>>().inner().clone();
            let path = req.uri().path().to_string();
            tauri::async_runtime::spawn(
                async move { responder.respond(gh_image(&st, &path).await) },
            );
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let rt = tauri::async_runtime::block_on(async { tokio::runtime::Handle::current() });
            let st = AppState::open(move |e| forward(&handle, e), rt)?;
            app.manage(Mutex::new(backend::start(&st)?));
            app.manage(st);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_config,
            list_repos,
            list_inbox,
            get_tickets,
            get_pr,
            set_unread,
            snooze,
            refresh,
            save_settings,
            set_repo_flag,
            open_config,
            install_cli,
            get_worktree,
            create_worktree,
            open_worktree,
            get_runs,
            list_active_runs,
            mark_run_seen,
            run_prompt,
            fix_findings,
            cancel_run,
            continue_run,
            post_review,
            approve_pr,
            open_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
