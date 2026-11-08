use anyhow::{Context, Result, bail};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{process::Command, sync::Mutex};

const ENDPOINT: &str = "https://api.github.com/graphql";
// GUI apps on macOS don't inherit the shell PATH, so try the usual install locations too.
const GH_BINS: [&str; 3] = ["gh", "/opt/homebrew/bin/gh", "/usr/local/bin/gh"];

pub struct Github {
    http: reqwest::Client,
    token: Mutex<Option<String>>,
}

#[derive(Deserialize)]
struct Response<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GqlError>,
}

#[derive(Deserialize)]
struct GqlError {
    message: String,
}

impl Github {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("ghreviewer")
                .build()
                .expect("http client"),
            token: Mutex::new(None),
        }
    }

    async fn token(&self, refresh: bool) -> Result<String> {
        let mut token = self.token.lock().await;
        if refresh || token.is_none() {
            *token = Some(gh_token().await?);
        }
        Ok(token.clone().unwrap())
    }

    pub async fn query<T: DeserializeOwned>(&self, query: &str, variables: Value) -> Result<T> {
        let body = json!({ "query": query, "variables": variables });
        let mut refreshed = false;
        loop {
            let token = self.token(refreshed).await?;
            let res = self.http.post(ENDPOINT).bearer_auth(token).json(&body).send().await?;
            let status = res.status();
            if status == reqwest::StatusCode::UNAUTHORIZED && !refreshed {
                refreshed = true;
                continue;
            }
            let text = res.text().await?;
            if !status.is_success() {
                bail!("GitHub {status}: {}", text.chars().take(300).collect::<String>());
            }
            let res: Response<T> = serde_json::from_str(&text).context("unexpected GitHub response")?;
            let errors = res.errors.iter().map(|e| e.message.as_str()).collect::<Vec<_>>().join("; ");
            return match res.data {
                Some(data) => {
                    // Partial data is common (e.g. SAML-protected orgs); keep what we got.
                    if !errors.is_empty() {
                        eprintln!("GitHub partial errors: {errors}");
                    }
                    Ok(data)
                }
                None => bail!("GitHub: {errors}"),
            };
        }
    }
}

async fn gh_token() -> Result<String> {
    for bin in GH_BINS {
        match Command::new(bin).args(["auth", "token"]).output().await {
            Ok(out) if out.status.success() => {
                return Ok(String::from_utf8(out.stdout)?.trim().to_string());
            }
            Ok(out) => bail!(
                "`gh auth token` failed ({}). Run `gh auth login`.",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(_) => continue,
        }
    }
    bail!("GitHub CLI `gh` not found. Install it and run `gh auth login`.")
}
