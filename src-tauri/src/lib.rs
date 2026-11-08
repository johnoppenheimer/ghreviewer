mod config;
mod db;
mod github;
mod sync;

use anyhow::Context;
use config::{Config, Settings};
use db::Db;
use github::Github;
use notify::{RecursiveMode, Watcher};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
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
    changed(app);
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
        .filter(|pr| pr.unread && cached.get(&pr.id) != Some(&pr.updated_at))
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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
            });
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
