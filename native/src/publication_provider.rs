//! Pull-request transport. Git transport and authority remain in
//! `pending_publication`; this adapter never infers permission from login state.
use crate::{Error, Result, require};
use regex::Regex;
use serde_json::{Value, json};
use std::{process::Command, sync::LazyLock, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    pub id: String,
    pub state: String,
    pub url: String,
    pub head: String,
    pub body: String,
}

pub trait Provider {
    fn list(&mut self, scope: &Value) -> Result<Vec<PullRequest>>;
    fn create(
        &mut self,
        scope: &Value,
        title: &str,
        body: &str,
        request_id: &str,
    ) -> Result<PullRequest>;
    fn update(&mut self, scope: &Value, id: &str, title: &str, body: &str) -> Result<PullRequest>;
}

pub struct GitHubProvider {
    repository: String,
}

impl GitHubProvider {
    pub fn new(repository: &str) -> Self {
        Self {
            repository: repository.into(),
        }
    }

    fn identity(&self) -> Result<(String, String)> {
        static SSH: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"^git@([^:]+):([^/]+/[^/]+?)(?:\.git)?$").unwrap());
        static NAME: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$").unwrap());
        let (host, name) = if let Some(found) = SSH.captures(&self.repository) {
            (found[1].to_owned(), found[2].to_owned())
        } else {
            let rest = self
                .repository
                .strip_prefix("https://")
                .or_else(|| self.repository.strip_prefix("ssh://"))
                .ok_or_else(|| Error("configure a supported GitHub repository URL".into()))?;
            require(
                !rest.contains(['?', '#', '@']),
                "configure a supported GitHub repository URL",
            )?;
            let (host, path) = rest.split_once('/').ok_or_else(|| {
                Error("repository must identify one GitHub owner and repository".into())
            })?;
            (
                host.to_owned(),
                path.trim_matches('/').trim_end_matches(".git").to_owned(),
            )
        };
        require(
            !host.is_empty() && NAME.is_match(&name),
            "repository must identify one GitHub owner and repository",
        )?;
        Ok((host, name))
    }

    fn api(&self, method: &str, endpoint: &str, fields: Option<&Value>) -> Result<Value> {
        let (host, _) = self.identity()?;
        let mut command = Command::new("gh");
        command.args(["api", "--hostname", &host, "--method", method, endpoint]);
        let payload = if let Some(fields) = fields {
            command.args(["--input", "-"]);
            serde_json::to_vec(fields)?
        } else {
            vec![]
        };
        let raw = crate::reasoning_runtime::run_command_bounded(
            &mut command,
            payload,
            Duration::from_secs(30),
            16 * 1024 * 1024,
        )
        .map_err(|error| {
            if error.0 == "native reasoning process failed" {
                Error("provider request failed; reconcile its outcome before retrying".into())
            } else {
                Error("provider is unavailable; remote outcome is unknown".into())
            }
        })?;
        crate::json_ingress::parse_slice(&raw, crate::json_ingress::DuplicateKeys::Reject)
            .map_err(|_| Error("provider returned an unreadable response".into()))
    }

    fn row(value: &Value) -> Result<PullRequest> {
        let state = if value.get("merged_at").is_some_and(|value| !value.is_null()) {
            "merged"
        } else {
            value["state"]
                .as_str()
                .ok_or_else(|| Error("provider returned an incomplete publication state".into()))?
        };
        Ok(PullRequest {
            id: value["number"]
                .as_i64()
                .map(|value| value.to_string())
                .or_else(|| value["id"].as_str().map(str::to_owned))
                .ok_or_else(|| Error("provider returned an incomplete publication state".into()))?,
            state: state.into(),
            url: value["html_url"].as_str().unwrap_or_default().into(),
            head: value["head"]["sha"].as_str().unwrap_or_default().into(),
            body: value["body"].as_str().unwrap_or_default().into(),
        })
    }
}

impl Provider for GitHubProvider {
    fn list(&mut self, scope: &Value) -> Result<Vec<PullRequest>> {
        let (_, name) = self.identity()?;
        let branch = scope["branch"].as_str().unwrap_or_default();
        let target = scope["target"].as_str().unwrap_or_default();
        let mut result = vec![];
        for page in 1..=100 {
            let endpoint = format!("repos/{}/pulls?state=all&per_page=100&page={page}", name);
            let rows = self.api("GET", &endpoint, None)?;
            let rows = rows.as_array().ok_or_else(|| {
                Error("provider did not return a complete pull-request list".into())
            })?;
            for row in rows {
                if row["head"]["ref"] == branch
                    && row["base"]["ref"] == target
                    && row["head"]["repo"]["full_name"] == name
                    && row["base"]["repo"]["full_name"] == name
                {
                    result.push(Self::row(row)?);
                }
            }
            if rows.len() < 100 {
                return Ok(result);
            }
        }
        Err(Error(
            "provider listing exceeded its bounded pagination limit".into(),
        ))
    }

    fn create(
        &mut self,
        scope: &Value,
        title: &str,
        body: &str,
        _request_id: &str,
    ) -> Result<PullRequest> {
        let (_, name) = self.identity()?;
        Self::row(&self.api(
            "POST",
            &format!("repos/{name}/pulls"),
            Some(&json!({"head":scope["branch"],"base":scope["target"],"title":title,"body":body})),
        )?)
    }

    fn update(&mut self, _scope: &Value, id: &str, title: &str, body: &str) -> Result<PullRequest> {
        let (_, name) = self.identity()?;
        require(
            id.bytes().all(|byte| byte.is_ascii_digit()),
            "invalid pull request ID",
        )?;
        Self::row(&self.api(
            "PATCH",
            &format!("repos/{name}/pulls/{id}"),
            Some(&json!({"title":title,"body":body})),
        )?)
    }
}
