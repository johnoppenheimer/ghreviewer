use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::sync::Mutex;

const ENDPOINT: &str = "https://api.github.com/graphql";

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

/// GitHub answered a mutation with errors, so it changed nothing. Unlike a timeout or a 5xx,
/// where the change may have gone through.
#[derive(Debug)]
pub struct Rejected(String);

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GitHub: {}", self.0)
    }
}

impl std::error::Error for Rejected {}

impl Github {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("ghreviewer")
                .timeout(std::time::Duration::from_secs(30))
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
        let (data, errors) = self.send(query, variables).await?;
        match data {
            Some(data) => {
                // Partial data is common (e.g. SAML-protected orgs); keep what we got.
                if !errors.is_empty() {
                    eprintln!("GitHub partial errors: {errors}");
                }
                Ok(data)
            }
            None => bail!("GitHub: {errors}"),
        }
    }

    /// Like `query`, but any error fails it: a failed mutation comes back as partial data.
    pub async fn mutate<T: DeserializeOwned>(&self, query: &str, variables: Value) -> Result<T> {
        match self.send(query, variables).await? {
            (Some(data), errors) if errors.is_empty() => Ok(data),
            (_, errors) => Err(Rejected(errors).into()),
        }
    }

    /// The response's data and its errors joined with `; `.
    async fn send<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: Value,
    ) -> Result<(Option<T>, String)> {
        let body = json!({ "query": query, "variables": variables });
        let mut refreshed = false;
        loop {
            let token = self.token(refreshed).await?;
            let res = self
                .http
                .post(ENDPOINT)
                .bearer_auth(token)
                .json(&body)
                .send()
                .await?;
            let status = res.status();
            if status == reqwest::StatusCode::UNAUTHORIZED && !refreshed {
                refreshed = true;
                continue;
            }
            let text = res.text().await?;
            if !status.is_success() {
                bail!(
                    "GitHub {status}: {}",
                    text.chars().take(300).collect::<String>()
                );
            }
            let res: Response<T> =
                serde_json::from_str(&text).context("unexpected GitHub response")?;
            let errors = res
                .errors
                .iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            return Ok((res.data, errors));
        }
    }

    /// A file on raw.githubusercontent.com (`path` is `/<owner>/<repo>/<ref>/<file>`) and its
    /// content type.
    pub async fn raw(&self, path: &str) -> Result<(String, Vec<u8>)> {
        // Anything but a path could send the token to another host.
        ensure!(path.starts_with('/'), "not a path: {path}");
        let res = self
            .http
            .get(format!("https://raw.githubusercontent.com{path}"))
            .bearer_auth(self.token(false).await?)
            .send()
            .await?
            .error_for_status()?;
        let mime = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        Ok((mime, res.bytes().await?.to_vec()))
    }
}

async fn gh_token() -> Result<String> {
    let out = crate::local::login_command(&["gh", "auth", "token"].map(String::from))
        .output()
        .await
        .context("GitHub CLI `gh` not found. Install it and run `gh auth login`.")?;
    if !out.status.success() {
        bail!(
            "`gh auth token` failed ({}). Run `gh auth login`.",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    // The last line: a login file may print before `gh` runs.
    let out = String::from_utf8(out.stdout)?;
    Ok(out.lines().last().unwrap_or_default().trim().to_string())
}
