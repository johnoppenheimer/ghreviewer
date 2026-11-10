mod agent;
mod config;
mod db;
mod github;
mod local;
mod sync;

use agent::{Review, Run, RunStatus};
use anyhow::Context;
use config::{Config, Prompt, Settings};
use db::Db;
use github::Github;
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use sync::{PrSummary, Repo};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

const REPOS_EVERY: Duration = Duration::from_secs(3600);

struct AppState {
    cfg_path: PathBuf,
    cfg: RwLock<Config>,
    db: Db,
    gh: Github,
    status: Mutex<Status>,
    wake: Notify,
    force_repos: AtomicBool,
    data_dir: PathBuf,
    runs: Mutex<HashMap<(String, String), RunningTask>>,
    run_seq: AtomicU64,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    viewer: Option<String>,
    syncing: bool,
    last_sync_ms: Option<u64>,
    rate_remaining: Option<u64>,
    error: Option<String>,
    config_error: Option<String>,
}

/// A prompt run in progress (keyed by PR id and prompt name), with a token telling runs apart.
type RunningTask = (u64, tauri::async_runtime::JoinHandle<()>);

type Res<T> = Result<T, String>;

fn err(e: impl Into<anyhow::Error>) -> String {
    format!("{:#}", e.into())
}

/// Tells the frontend that local data changed and should be re-read.
fn changed(app: &AppHandle) {
    let _ = app.emit("changed", ());
}

fn update_status(app: &AppHandle, st: &AppState, f: impl FnOnce(&mut Status)) {
    f(&mut st.status.lock().unwrap());
    let _ = app.emit("status", ());
}

fn reload_config(app: &AppHandle, st: &AppState) {
    match config::load(&st.cfg_path) {
        Ok(None) => {} // missing or empty mid-save: keep the current config
        Ok(Some(new)) => {
            let rules_changed = {
                let mut cfg = st.cfg.write().unwrap();
                let rules_changed = cfg.rules != new.rules;
                *cfg = new;
                rules_changed
            };
            if rules_changed {
                st.wake.notify_one();
            }
            update_status(app, st, |s| s.config_error = None);
            changed(app);
        }
        Err(e) => update_status(app, st, |s| s.config_error = Some(err(e))),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

async fn sync_once(st: &AppState, with_repos: bool) -> anyhow::Result<Option<u64>> {
    let cached = st.status.lock().unwrap().viewer.clone();
    let viewer = match cached {
        Some(v) => v,
        None => sync::viewer(&st.gh).await?,
    };
    st.status.lock().unwrap().viewer = Some(viewer.clone());
    if with_repos {
        st.db.replace_repos(&sync::fetch_repos(&st.gh).await?)?;
    }
    let rules = st.cfg.read().unwrap().rules.clone();
    let (prs, remaining) = sync::fetch_inbox(&st.gh, &rules, &viewer).await?;
    st.db.replace_prs(&prs)?;
    Ok(remaining)
}

async fn refresh_detail(st: &AppState, id: &str) -> anyhow::Result<Value> {
    let detail = sync::fetch_detail(&st.gh, id).await?;
    st.db.put_detail(id, &detail)?;
    Ok(detail)
}

const PREFETCH_MAX: usize = 20;

/// Caches details of the newest unread PRs that changed, so opening them is instant.
async fn prefetch_details(st: &Arc<AppState>) {
    let filters = st.cfg.read().unwrap().filters.clone();
    let (Ok(prs), Ok(cached)) = (st.db.inbox(&filters), st.db.detail_versions()) else {
        return;
    };
    let stale: Vec<String> = prs
        .into_iter()
        .filter(|pr| pr.unread && !pr.context && cached.get(&pr.id) != Some(&pr.updated_at))
        .take(PREFETCH_MAX)
        .map(|pr| pr.id)
        .collect();
    for chunk in stale.chunks(4) {
        let mut set = tokio::task::JoinSet::new();
        for id in chunk {
            let (st, id) = (st.clone(), id.clone());
            set.spawn(async move { refresh_detail(&st, &id).await });
        }
        set.join_all().await;
    }
}

async fn poller(app: AppHandle, st: Arc<AppState>) {
    let mut repos_at: Option<Instant> = None;
    loop {
        let with_repos = st.force_repos.swap(false, Ordering::Relaxed)
            || repos_at.is_none_or(|t| t.elapsed() > REPOS_EVERY);
        update_status(&app, &st, |s| s.syncing = true);
        let res = sync_once(&st, with_repos).await;
        if res.is_ok() && with_repos {
            repos_at = Some(Instant::now());
        }
        update_status(&app, &st, |s| {
            s.syncing = false;
            match res {
                Ok(remaining) => {
                    s.error = None;
                    s.last_sync_ms = Some(now_ms());
                    s.rate_remaining = remaining.or(s.rate_remaining);
                }
                Err(e) => s.error = Some(err(e)),
            }
        });
        changed(&app);
        prefetch_details(&st).await;
        let secs = st.cfg.read().unwrap().poll_interval_secs.max(30);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(secs)) => {}
            _ = st.wake.notified() => {}
        }
    }
}

#[tauri::command]
fn get_status(st: State<'_, Arc<AppState>>) -> Status {
    st.status.lock().unwrap().clone()
}

#[tauri::command]
fn get_config(st: State<'_, Arc<AppState>>) -> Config {
    st.cfg.read().unwrap().clone()
}

#[tauri::command]
fn list_repos(st: State<'_, Arc<AppState>>) -> Res<Vec<Repo>> {
    st.db.repos().map_err(err)
}

#[tauri::command]
fn list_inbox(st: State<'_, Arc<AppState>>) -> Res<Vec<PrSummary>> {
    let filters = st.cfg.read().unwrap().filters.clone();
    st.db.inbox(&filters).map_err(err)
}

/// Returns the cached PR right away and refreshes it in the background (`pr-updated` event).
#[tauri::command]
async fn get_pr(app: AppHandle, st: State<'_, Arc<AppState>>, id: String) -> Res<Value> {
    if let Some(cached) = st.db.detail(&id).map_err(err)? {
        let st = st.inner().clone();
        tauri::async_runtime::spawn(async move {
            if let Ok(detail) = refresh_detail(&st, &id).await {
                let _ = app.emit("pr-updated", json!({ "id": id, "detail": detail }));
            }
        });
        return Ok(cached);
    }
    refresh_detail(&st, &id).await.map_err(err)
}

#[tauri::command]
fn set_unread(app: AppHandle, st: State<'_, Arc<AppState>>, id: String, unread: bool) -> Res<()> {
    st.db.set_unread(&id, unread).map_err(err)?;
    changed(&app);
    Ok(())
}

#[tauri::command]
fn refresh(st: State<'_, Arc<AppState>>) {
    st.force_repos.store(true, Ordering::Relaxed);
    st.wake.notify_one();
}

fn edit_config(
    app: &AppHandle,
    st: &AppState,
    f: impl FnOnce(&str) -> anyhow::Result<String>,
) -> Res<()> {
    let text = std::fs::read_to_string(&st.cfg_path).map_err(err)?;
    config::write(&st.cfg_path, &f(&text).map_err(err)?).map_err(err)?;
    reload_config(app, st);
    Ok(())
}

#[tauri::command]
fn save_settings(app: AppHandle, st: State<'_, Arc<AppState>>, settings: Settings) -> Res<()> {
    edit_config(&app, &st, |text| config::apply_settings(text, &settings))
}

#[tauri::command]
fn set_repo_flag(
    app: AppHandle,
    st: State<'_, Arc<AppState>>,
    repo: String,
    flag: String,
    on: bool,
) -> Res<()> {
    edit_config(&app, &st, |text| {
        config::set_repo_flag(text, &repo, &flag, on)
    })
}

#[tauri::command]
fn open_config(st: State<'_, Arc<AppState>>) -> Res<()> {
    std::process::Command::new("open")
        .arg("-t")
        .arg(&st.cfg_path)
        .spawn()
        .map_err(err)?;
    Ok(())
}

fn clone_roots(st: &AppState) -> Vec<String> {
    st.cfg.read().unwrap().clone_roots.clone()
}

#[tauri::command]
async fn get_worktree(
    st: State<'_, Arc<AppState>>,
    repo: String,
    branch: String,
) -> Res<local::Worktree> {
    local::lookup(&clone_roots(&st), &repo, &branch)
        .await
        .map_err(err)
}

#[tauri::command]
async fn create_worktree(st: State<'_, Arc<AppState>>, repo: String, number: u64) -> Res<()> {
    local::create(&clone_roots(&st), &repo, number)
        .await
        .map_err(err)
}

/// Runs the `opener`-th `open_with` command on a PR's worktree.
#[tauri::command]
async fn open_worktree(
    st: State<'_, Arc<AppState>>,
    opener: usize,
    path: String,
    repo: String,
    branch: String,
    number: u64,
) -> Res<()> {
    let command = st
        .cfg
        .read()
        .unwrap()
        .open_with
        .get(opener)
        .map(|o| o.command.clone());
    let command = command.ok_or("no such `open_with` entry")?;
    let name = path.rsplit('/').next().unwrap_or_default().to_string();
    let number = number.to_string();
    let vars = [
        ("path", path.as_str()),
        ("name", &name),
        ("branch", &branch),
        ("repo", &repo),
        ("number", &number),
    ];
    local::open_with(&command, &vars).await.map_err(err)
}

/// Latest run of each prompt on a PR, letting the webview load the files they mention.
#[tauri::command]
fn get_runs(app: AppHandle, st: State<'_, Arc<AppState>>, pr_id: String) -> Res<Vec<Run>> {
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
fn list_active_runs(st: State<'_, Arc<AppState>>) -> Res<Vec<Run>> {
    st.db.active_runs().map_err(err)
}

#[tauri::command]
fn mark_run_seen(
    app: AppHandle,
    st: State<'_, Arc<AppState>>,
    pr_id: String,
    prompt: String,
) -> Res<()> {
    st.db.mark_run_seen(&pr_id, &prompt).map_err(err)?;
    let _ = app.emit("run-updated", &pr_id);
    Ok(())
}

/// The PR a prompt runs on, as the frontend knows it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Target {
    id: String,
    repo: String,
    number: u64,
    url: String,
    branch: String,
    base: String,
    head_oid: Option<String>,
}

/// Whether the run with `token` is still the live one for `key`: not cancelled, not replaced.
fn is_current<T>(
    running: &HashMap<(String, String), (u64, T)>,
    key: &(String, String),
    token: u64,
) -> bool {
    running.get(key).map(|r| r.0) == Some(token)
}

/// Saves a run, unless it was cancelled or replaced by a newer run of the same prompt.
fn save_run(app: &AppHandle, st: &AppState, token: u64, run: &Run, finished: bool) {
    let key = (run.pr_id.clone(), run.prompt.clone());
    let mut running = st.runs.lock().unwrap();
    if !is_current(&running, &key, token) {
        return;
    }
    if finished {
        running.remove(&key);
    }
    let _ = st.db.put_run(run);
    drop(running);
    let _ = app.emit("run-updated", &run.pr_id);
}

/// `{out}` of a prompt on a PR, unique per PR (node id) and prompt name.
fn run_dir(pr_id: &str, prompt: &str) -> anyhow::Result<PathBuf> {
    let safe = |s: &str| s.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "_");
    // An empty name would make `{out}` the PR's folder, and each run empties `{out}`.
    anyhow::ensure!(
        !safe(prompt).trim_matches('_').is_empty(),
        "prompt names need a letter or digit"
    );
    Ok(Path::new(&safe(pr_id)).join(safe(prompt)))
}

async fn run_task(
    app: &AppHandle,
    st: &AppState,
    token: u64,
    spec: &Prompt,
    pr: &Target,
    run: &mut Run,
) -> anyhow::Result<()> {
    let roots = clone_roots(st);
    let mut wt = local::lookup(&roots, &pr.repo, &pr.branch).await?;
    if wt.path.is_none() {
        run.activity
            .push(format!("creating worktree: wt switch pr:{}", pr.number));
        save_run(app, st, token, run, false);
        let created = local::create(&roots, &pr.repo, pr.number).await;
        let _ = app.emit("worktree-created", ());
        wt = local::lookup(&roots, &pr.repo, &pr.branch).await?;
        // Another prompt started at the same time may have made it first.
        if wt.path.is_none() {
            created?;
        }
    }
    let path = wt.path.context("no worktree on the PR branch")?;
    let name = path.rsplit('/').next().unwrap_or_default();
    anyhow::ensure!(
        local::rebasing_branch(&path).is_none(),
        "{name} is in the middle of a rebase: finish it, then re-run"
    );
    // The commit the agent will read: an existing worktree may be behind the PR's head.
    if let Ok(oid) = local::head(&path).await {
        run.head_oid = Some(oid);
    }
    let out = st.data_dir.join("runs").join(run_dir(&pr.id, &spec.name)?);
    // Fresh for each run, so it only holds this run's files.
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out)?;
    let (number, out_str) = (pr.number.to_string(), out.display().to_string());
    let vars = [
        ("path", path.as_str()),
        ("name", name),
        ("branch", &pr.branch),
        ("base", &pr.base),
        ("repo", &pr.repo),
        ("number", &number),
        ("url", &pr.url),
        ("out", &out_str),
        ("review_format", agent::REVIEW_FORMAT),
    ];
    let prompt = local::fill(std::slice::from_ref(&spec.prompt), &vars).remove(0);
    run.path = Some(path.clone());
    let title = format!("{}#{} {}", pr.repo, pr.number, spec.name);
    agent::run(spec, Path::new(&path), &out, &title, &prompt, run, |r| {
        save_run(app, st, token, r, false)
    })
    .await
}

/// Runs a configured prompt on a PR in the background, replacing its previous run.
#[tauri::command]
fn run_prompt(app: AppHandle, st: State<'_, Arc<AppState>>, prompt: String, pr: Target) -> Res<()> {
    let spec = st
        .cfg
        .read()
        .unwrap()
        .prompts
        .iter()
        .find(|p| p.name == prompt)
        .cloned();
    let spec = spec.ok_or("no such prompt in the config")?;
    let run = Run {
        pr_id: pr.id.clone(),
        repo: pr.repo.clone(),
        number: pr.number,
        prompt: prompt.clone(),
        harness: spec.harness,
        started_at: now_ms(),
        head_oid: pr.head_oid.clone(),
        ..Default::default()
    };
    let token = st.run_seq.fetch_add(1, Ordering::Relaxed);
    let key = (pr.id.clone(), prompt);
    // Locked until the run is registered, so the task's first save can't be rejected.
    let mut running = st.runs.lock().unwrap();
    st.db.put_run(&run).map_err(err)?;
    let task = {
        let (app, st) = (app.clone(), st.inner().clone());
        tauri::async_runtime::spawn(async move {
            let mut run = run;
            let res = run_task(&app, &st, token, &spec, &pr, &mut run).await;
            if let Err(e) = res {
                run.error.get_or_insert_with(|| err(e));
            }
            run.status = if run.error.is_some() {
                RunStatus::Error
            } else {
                RunStatus::Done
            };
            run.finished_at = Some(now_ms());
            save_run(&app, &st, token, &run, true);
        })
    };
    if let Some((_, old)) = running.insert(key.clone(), (token, task)) {
        old.abort();
    }
    drop(running);
    let _ = app.emit("run-updated", &key.0);
    Ok(())
}

#[tauri::command]
fn cancel_run(
    app: AppHandle,
    st: State<'_, Arc<AppState>>,
    pr_id: String,
    prompt: String,
) -> Res<()> {
    let removed = st
        .runs
        .lock()
        .unwrap()
        .remove(&(pr_id.clone(), prompt.clone()));
    let Some((_, task)) = removed else {
        return Ok(());
    };
    task.abort(); // drops the harness process with it
    if let Some(mut run) = st.db.run(&pr_id, &prompt).map_err(err)? {
        run.status = RunStatus::Cancelled;
        run.finished_at = Some(now_ms());
        run.seen = true; // you cancelled it, nothing to catch up on
        st.db.put_run(&run).map_err(err)?;
    }
    let _ = app.emit("run-updated", &pr_id);
    Ok(())
}

/// Reopens a run's agent session interactively with the `terminal` command.
#[tauri::command]
async fn continue_run(st: State<'_, Arc<AppState>>, pr_id: String, prompt: String) -> Res<()> {
    let run = st.db.run(&pr_id, &prompt).map_err(err)?;
    let run = run.ok_or("this prompt hasn't run on this PR")?;
    let command = agent::resume_command(&run).ok_or("no agent session to continue yet")?;
    let path = run.path.unwrap_or_default();
    let name = path.rsplit('/').next().unwrap_or_default();
    let vars = [
        ("command", command.as_str()),
        ("path", &path),
        ("name", name),
    ];
    let terminal = st.cfg.read().unwrap().terminal.clone();
    local::open_with(&terminal, &vars).await.map_err(err)
}

/// Posts the picked findings of a run's review to GitHub as one review.
#[tauri::command]
async fn post_review(
    app: AppHandle,
    st: State<'_, Arc<AppState>>,
    pr_id: String,
    prompt: String,
    picked: Vec<usize>,
    approve: bool,
) -> Res<()> {
    let latest = || -> Res<Run> {
        let run = st.db.run(&pr_id, &prompt).map_err(err)?;
        run.ok_or_else(|| "this prompt hasn't run on this PR".into())
    };
    let run = latest()?;
    let review = run
        .review
        .as_ref()
        .ok_or("this run has no findings to post")?;
    if review.posted.is_some() {
        return Err("this review was already posted".into());
    }
    if picked.is_empty() && !approve {
        return Err("pick at least one finding to comment".into());
    }
    let (url, all_in_body) = sync::post_review(
        &st.gh,
        &pr_id,
        run.head_oid.as_deref(),
        review,
        &picked,
        approve,
    )
    .await
    .map_err(err)?;
    // Re-read: the prompt may have been re-run while posting.
    let mut now = latest()?;
    if now.started_at == run.started_at
        && let Some(review) = now.review.as_mut()
    {
        review.posted = Some(url);
        review.all_in_body = all_in_body;
        st.db.put_run(&now).map_err(err)?;
    }
    let _ = app.emit("run-updated", &pr_id);
    Ok(())
}

/// Approves the commit of a PR that was on screen (`head_oid`), with an optional message, then
/// refreshes the PR to show the approval.
#[tauri::command]
async fn approve_pr(
    app: AppHandle,
    st: State<'_, Arc<AppState>>,
    id: String,
    head_oid: String,
    body: String,
) -> Res<()> {
    let review = Review {
        summary: body,
        ..Default::default()
    };
    sync::post_review(&st.gh, &id, Some(&head_oid), &review, &[], true)
        .await
        .map_err(err)?;
    if let Ok(detail) = refresh_detail(&st, &id).await {
        let _ = app.emit("pr-updated", json!({ "id": id, "detail": detail }));
    }
    Ok(())
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
fn open_path(st: State<'_, Arc<AppState>>, pr_id: String, path: String) -> Res<()> {
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

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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
            let data_dir = dirs::data_dir().context("no data dir")?.join("ghreviewer");
            std::fs::create_dir_all(&data_dir)?;
            let cfg_path = config::path();
            config::ensure(&cfg_path)?;
            let (cfg, config_error) = match config::load(&cfg_path) {
                Ok(c) => (c.unwrap_or_default(), None),
                Err(e) => (Config::default(), Some(err(e))),
            };
            let st = Arc::new(AppState {
                cfg: RwLock::new(cfg),
                db: Db::open(&data_dir.join("cache.db"))?,
                gh: Github::new(),
                status: Mutex::new(Status {
                    config_error,
                    ..Default::default()
                }),
                wake: Notify::new(),
                force_repos: AtomicBool::new(false),
                cfg_path: cfg_path.clone(),
                data_dir: data_dir.clone(),
                runs: Mutex::default(),
                run_seq: AtomicU64::new(0),
            });
            st.db.interrupt_runs()?;
            app.manage(st.clone());

            // Watch the directory: editors often save by replacing the file.
            let handle = app.handle().clone();
            let watched = st.clone();
            let mut watcher =
                notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                    if res.is_ok_and(|e| e.paths.iter().any(|p| p.ends_with("config.toml"))) {
                        reload_config(&handle, &watched);
                    }
                })?;
            watcher.watch(
                cfg_path.parent().context("bad config path")?,
                RecursiveMode::NonRecursive,
            )?;
            app.manage(Mutex::new(watcher));

            tauri::async_runtime::spawn(poller(app.handle().clone(), st));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_config,
            list_repos,
            list_inbox,
            get_pr,
            set_unread,
            refresh,
            save_settings,
            set_repo_flag,
            open_config,
            get_worktree,
            create_worktree,
            open_worktree,
            get_runs,
            list_active_runs,
            mark_run_seen,
            run_prompt,
            cancel_run,
            continue_run,
            post_review,
            approve_pr,
            open_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_live_run_saves() {
        let key = ("PR_1".to_string(), "review".to_string());
        let mut running = HashMap::new();
        running.insert(key.clone(), (1, ()));
        assert!(is_current(&running, &key, 1));
        running.insert(key.clone(), (2, ())); // re-run replaces it
        assert!(!is_current(&running, &key, 1));
        assert!(is_current(&running, &key, 2));
        running.remove(&key); // cancelled
        assert!(!is_current(&running, &key, 2));
    }

    #[test]
    fn run_dirs_are_per_pr_and_never_the_pr_folder() {
        assert_eq!(
            run_dir("PR_kw1", "pr review").unwrap(),
            Path::new("PR_kw1/pr_review")
        );
        assert_ne!(run_dir("PR_a", "x").unwrap(), run_dir("PR_b", "x").unwrap());
        assert!(run_dir("PR_1", "").is_err());
        assert!(run_dir("PR_1", " / ").is_err());
    }
}
