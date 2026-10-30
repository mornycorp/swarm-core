//! GitHub, through the `gh` CLI: every call the swarm makes to a project's issues and pull
//! requests, with a deadline, and the server's own failures tried again ([`transient`]).
//!
//! The answers are read into one shape each, whatever the call (REST or GraphQL), so the
//! commands' decisions are pure functions over it:
//!
//! - **A pull request** ([`pr_from_rest`], [`pr_from_graphql`]): `number`, `state` (`open`,
//!   `closed` or `merged`), `draft`, `head_ref`, `head_sha`, `base_ref`, `title`, `body`,
//!   `html_url`, `node_id`, `cross_repo`, and where the list has them `merge_state` (GitHub's
//!   `mergeable_state`, lower case: `clean`, `behind`, `dirty`, `blocked`, `unstable`, `draft`,
//!   `has_hooks`, `unknown`), `checks` (the head commit's check rollup: `success`, `failure`,
//!   `pending`, `error`, `expected`, or null) and `comments` (how many).
//! - **An issue** ([`issue_from_rest`], [`issue_from_graphql`]): `number`, `state` (`open` or
//!   `closed`), `title`, `body`, `labels` (their names). GitHub's issue lists hold pull requests
//!   too: those are left out.
//!
//! `gh` asks `GH_HOST` when it is set, else the project's host ([`crate::project::GitHub::host`]).

use std::process::Command;
use std::time::Duration;

use serde_json::{Value, json};

use crate::Project;

/// How long one `gh api` call may take.
pub const DEADLINE: Duration = Duration::from_secs(45);
/// Tries of one call that failed for the server's reasons, and the pause between them.
const TRIES: u32 = 4;
const RETRY_PAUSE: Duration = if cfg!(test) { Duration::ZERO } else { Duration::from_secs(10) };
/// The most pages (100 each) one list reads.
pub const MAX_PAGES: u32 = 10;

/// A failure worth trying again: the server's (`(HTTP 502)`, a timeout, a reset), not the
/// request's.
pub fn transient(err: &str) -> bool {
    ["(HTTP 500)", "(HTTP 502)", "(HTTP 503)", "(HTTP 504)", "took longer than", "timed out", "onnection reset"].iter().any(|w| err.contains(w))
}

/// `call`, tried again after `pause` while it fails for the server's reasons ([`transient`]), up
/// to [`TRIES`] times in all.
pub fn with_retries(pause: Duration, mut call: impl FnMut() -> Result<String, String>) -> Result<String, String> {
    let mut attempt = 1;
    loop {
        match call() {
            Err(e) if transient(&e) && attempt < TRIES => {
                attempt += 1;
                std::thread::sleep(pause);
            }
            done => return done,
        }
    }
}

/// `gh <args>` for `project`: its host when `GH_HOST` doesn't say (`env_host`).
fn gh_command(project: &Project, args: &[&str], env_host: Option<std::ffi::OsString>) -> Command {
    let mut cmd = Command::new("gh");
    cmd.args(args).env("GH_PROMPT_DISABLED", "1").env("NO_COLOR", "1");
    if env_host.is_none() && project.github.host != "github.com" {
        cmd.env("GH_HOST", project.github.host);
    }
    cmd
}

/// `gh <args>` once, within [`DEADLINE`]: its output.
fn gh_once(project: &Project, args: &[&str]) -> Result<String, String> {
    let what = match args {
        ["api", path, ..] if !path.starts_with('-') => format!("gh api {path}"),
        ["api", ..] => "gh api".to_owned(),
        [first, second, ..] => format!("gh {first} {second}"),
        _ => "gh".to_owned(),
    };
    crate::process::output(gh_command(project, args, std::env::var_os("GH_HOST")), None, DEADLINE, &what)
}

/// `gh api <args>` on `project`'s GitHub, its body as text (a delete answers with none). The
/// server's failures are tried again.
pub fn api_text(project: &Project, args: &[&str]) -> Result<String, String> {
    let all: Vec<&str> = std::iter::once("api").chain(args.iter().copied()).collect();
    with_retries(RETRY_PAUSE, || gh_once(project, &all))
}

/// [`api_text`], its body as JSON (`null` for an empty body).
pub fn api(project: &Project, args: &[&str]) -> Result<Value, String> {
    let out = api_text(project, args)?;
    if out.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&out).map_err(|e| format!("gh api {}: bad JSON: {e}", args.first().copied().unwrap_or_default()))
}

/// A repository as GitHub names it, `owner/name`, checked: anything else is refused, so a value
/// never becomes part of another path or a query.
pub fn check_repo(repo: &str) -> Result<&str, String> {
    let ok = repo.len() <= 200
        && repo.split('/').count() == 2
        && repo.split('/').all(|p| !p.is_empty() && !p.starts_with(['.', '-']) && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')));
    if ok { Ok(repo) } else { Err(format!("`{repo}` is not a GitHub repository (owner/name)")) }
}

/// The REST path of `project`'s repository (`of` `None`), or of another by its `owner/name`.
pub fn repo_path(project: &Project, of: Option<&str>) -> Result<String, String> {
    Ok(format!("repos/{}", check_repo(of.unwrap_or(project.github.repo))?))
}

/// `text` as one segment of a URL path (a label's name, say): every byte but the unreserved ones
/// percent-encoded.
pub fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// A GraphQL `query` with string `vars`, its `data`. An answer with errors and no data is an
/// error; one with both keeps the data (a missing issue among several asked for), which `gh`
/// prints though it exits with a failure.
pub fn graphql(project: &Project, query: &str, vars: &[(&str, &str)]) -> Result<Value, String> {
    let mut args = vec!["api".to_owned(), "graphql".to_owned(), "-f".to_owned(), format!("query={query}")];
    for (k, v) in vars {
        args.extend(["-f".to_owned(), format!("{k}={v}")]);
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = with_retries(RETRY_PAUSE, || {
        let (ok, out, err) = crate::process::captured(gh_command(project, &args, std::env::var_os("GH_HOST")), None, DEADLINE, "gh api graphql")?;
        let out = String::from_utf8_lossy(&out).into_owned();
        if ok || out.trim_start().starts_with('{') { Ok(out) } else { Err(format!("gh api graphql: {}", String::from_utf8_lossy(&err).trim())) }
    })?;
    data_of(&out)
}

/// The `data` of a GraphQL answer `out`, or its errors.
fn data_of(out: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(out).map_err(|e| format!("gh api graphql: bad JSON: {e}"))?;
    match v.get("data") {
        Some(d) if !d.is_null() => Ok(d.clone()),
        _ => Err(format!("gh api graphql: {}", v.get("errors").map(Value::to_string).unwrap_or_default())),
    }
}

/// The owner and name of `of` (`None`: `project`'s), for a GraphQL query's variables.
fn owner_name<'a>(project: &'a Project, of: Option<&'a str>) -> Result<(&'a str, &'a str), String> {
    check_repo(of.unwrap_or(project.github.repo))?.split_once('/').ok_or_else(|| "a repository is owner/name".to_owned())
}

// ---- The shapes ----

fn text(v: &Value, pointer: &str) -> String {
    v.pointer(pointer).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn lower(v: &Value, pointer: &str) -> Value {
    v.pointer(pointer).and_then(Value::as_str).map_or(Value::Null, |s| Value::String(s.to_ascii_lowercase()))
}

/// A pull request from the REST API (`GET repos/…/pulls/<n>`, or a list of them); none without a
/// number.
pub fn pr_from_rest(v: &Value) -> Option<Value> {
    let merged = v.get("merged").and_then(Value::as_bool).unwrap_or(false) || v.get("merged_at").is_some_and(|m| !m.is_null());
    let state = if merged { "merged".to_owned() } else { text(v, "/state") };
    let head_repo = text(v, "/head/repo/full_name");
    let base_repo = text(v, "/base/repo/full_name");
    Some(json!({
        "number": v.get("number")?.as_u64()?,
        "state": state,
        "draft": v.get("draft").and_then(Value::as_bool).unwrap_or(false),
        "head_ref": text(v, "/head/ref"),
        "head_sha": text(v, "/head/sha"),
        "base_ref": text(v, "/base/ref"),
        "title": text(v, "/title"),
        "body": text(v, "/body"),
        "html_url": text(v, "/html_url"),
        "node_id": text(v, "/node_id"),
        "cross_repo": !head_repo.is_empty() && head_repo != base_repo,
        "merge_state": lower(v, "/mergeable_state"),
        "checks": Value::Null,
        "comments": v.get("comments").and_then(Value::as_u64).unwrap_or(0) + v.get("review_comments").and_then(Value::as_u64).unwrap_or(0),
    }))
}

/// A pull request from [`OPEN_PRS`]' nodes; none without a number.
pub fn pr_from_graphql(v: &Value) -> Option<Value> {
    Some(json!({
        "number": v.get("number")?.as_u64()?,
        "state": v.get("state").and_then(Value::as_str).unwrap_or_default().to_ascii_lowercase(),
        "draft": v.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
        "head_ref": text(v, "/headRefName"),
        "head_sha": text(v, "/headRefOid"),
        "base_ref": text(v, "/baseRefName"),
        "title": text(v, "/title"),
        "body": text(v, "/body"),
        "html_url": text(v, "/url"),
        "node_id": text(v, "/id"),
        "cross_repo": v.get("isCrossRepository").and_then(Value::as_bool).unwrap_or(false),
        "merge_state": lower(v, "/mergeStateStatus"),
        "checks": lower(v, "/commits/nodes/0/commit/statusCheckRollup/state"),
        "comments": v.pointer("/comments/totalCount").and_then(Value::as_u64).unwrap_or(0),
    }))
}

/// The label names of an issue's `labels` (REST: objects with a `name`; GraphQL: `nodes`).
fn label_names(labels: Option<&Value>) -> Vec<String> {
    let list = labels.and_then(|l| l.as_array().or_else(|| l.get("nodes").and_then(Value::as_array)));
    list.into_iter().flatten().filter_map(|l| l.get("name").and_then(Value::as_str).or_else(|| l.as_str())).map(str::to_owned).collect()
}

/// An issue from the REST API; none without a number, or for a pull request (GitHub's issue
/// lists hold both).
pub fn issue_from_rest(v: &Value) -> Option<Value> {
    if v.get("pull_request").is_some() {
        return None;
    }
    Some(json!({
        "number": v.get("number")?.as_u64()?,
        "state": text(v, "/state"),
        "title": text(v, "/title"),
        "body": text(v, "/body"),
        "labels": label_names(v.get("labels")),
    }))
}

/// An issue (or a pull request, `merged` read as `closed`) from a GraphQL node.
pub fn issue_from_graphql(v: &Value) -> Option<Value> {
    let state = v.get("state").and_then(Value::as_str).unwrap_or_default().to_ascii_lowercase();
    Some(json!({
        "number": v.get("number")?.as_u64()?,
        "state": if state == "merged" { "closed".to_owned() } else { state },
        "title": text(v, "/title"),
        "body": text(v, "/body"),
        "labels": label_names(v.get("labels")),
    }))
}

/// The numbers in a list of either shape.
pub fn numbers(list: &[Value]) -> Vec<u64> {
    list.iter().filter_map(|v| v.get("number")?.as_u64()).collect()
}

// ---- Pull requests ----

/// The open pull requests of a repository, with their merge state, checks and comment counts.
pub const OPEN_PRS: &str = "query($owner: String!, $name: String!, $after: String) { repository(owner: $owner, name: $name) { \
pullRequests(states: OPEN, first: 100, after: $after, orderBy: {field: CREATED_AT, direction: ASC}) { pageInfo { hasNextPage endCursor } \
nodes { id number title body state isDraft headRefName headRefOid baseRefName url isCrossRepository mergeStateStatus \
comments { totalCount } commits(last: 1) { nodes { commit { statusCheckRollup { state } } } } } } } }";

/// The open pull requests of `project`'s repository (`of` `None`) or of another, every page's
/// (up to [`MAX_PAGES`]).
pub fn open_prs(project: &Project, of: Option<&str>) -> Result<Vec<Value>, String> {
    let (owner, name) = owner_name(project, of)?;
    let mut out = Vec::new();
    let mut after = String::new();
    for _ in 0..MAX_PAGES {
        let mut vars = vec![("owner", owner), ("name", name)];
        if !after.is_empty() {
            vars.push(("after", after.as_str()));
        }
        let data = graphql(project, OPEN_PRS, &vars)?;
        let page = data.pointer("/repository/pullRequests").cloned().unwrap_or_default();
        out.extend(page.pointer("/nodes").and_then(Value::as_array).into_iter().flatten().filter_map(pr_from_graphql));
        match page.pointer("/pageInfo/endCursor").and_then(Value::as_str) {
            Some(cursor) if page.pointer("/pageInfo/hasNextPage") == Some(&Value::Bool(true)) => after = cursor.to_owned(),
            _ => break,
        }
    }
    Ok(out)
}

/// Pull request `n` of `project`'s repository.
pub fn pr(project: &Project, n: u64) -> Result<Value, String> {
    let v = api(project, &[&format!("{}/pulls/{n}", repo_path(project, None)?)])?;
    pr_from_rest(&v).ok_or_else(|| format!("GitHub's answer for pull request #{n} has no number"))
}

/// Open a draft pull request from `head` into `base`: the new pull request.
pub fn create_draft_pr(project: &Project, head: &str, base: &str, title: &str, body: &str) -> Result<Value, String> {
    let path = format!("{}/pulls", repo_path(project, None)?);
    let fields = [format!("head={head}"), format!("base={base}"), format!("title={title}"), format!("body={body}")];
    let mut args = vec!["-X", "POST", &path, "-F", "draft=true"];
    for f in &fields {
        args.extend(["-f", f]);
    }
    let v = api(project, &args)?;
    pr_from_rest(&v).ok_or_else(|| "GitHub's answer to the new pull request has no number".to_owned())
}

/// Close pull request `n` without merging it.
pub fn close_pr(project: &Project, n: u64) -> Result<(), String> {
    api(project, &["-X", "PATCH", &format!("{}/pulls/{n}", repo_path(project, None)?), "-f", "state=closed"]).map(drop)
}

/// Mark pull request `n` ready for review (GitHub has no REST call for it: `gh pr ready`).
pub fn mark_ready(project: &Project, n: u64) -> Result<(), String> {
    let (repo, n) = (check_repo(project.github.repo)?, n.to_string());
    with_retries(RETRY_PAUSE, || gh_once(project, &["pr", "ready", &n, "--repo", repo])).map(drop)
}

/// Rebase pull request `node_id`'s branch onto its base on the server, only while its head is
/// still `head`: `Ok(Some(why))` when GitHub refuses the rebase (a conflict, merge commits).
pub fn rebase_pr(project: &Project, node_id: &str, head: &str) -> Result<Option<String>, String> {
    const REBASE: &str = "mutation($id: ID!, $head: GitObjectID!) { updatePullRequestBranch(input: {pullRequestId: $id, expectedHeadOid: $head, updateMethod: REBASE}) { pullRequest { number } } }";
    match graphql(project, REBASE, &[("id", node_id), ("head", head)]) {
        Ok(_) => Ok(None),
        Err(e) if transient(&e) => Err(e),
        Err(e) => Ok(Some(e)),
    }
}

/// Merge pull request `n` with a merge commit, only while its head is `sha`; `message` the merge
/// commit's body, else GitHub's own.
pub fn merge_pr(project: &Project, n: u64, sha: &str, message: Option<&str>) -> Result<(), String> {
    let path = format!("{}/pulls/{n}/merge", repo_path(project, None)?);
    let sha = format!("sha={sha}");
    let body = message.map(|m| format!("commit_message={m}"));
    let mut args = vec!["-X", "PUT", &path, "-f", &sha, "-f", "merge_method=merge"];
    if let Some(b) = &body {
        args.extend(["-f", b]);
    }
    api(project, &args).map(drop)
}

/// Delete `branch` of `project`'s repository.
pub fn delete_branch(project: &Project, branch: &str) -> Result<(), String> {
    api_text(project, &["-X", "DELETE", &format!("{}/git/refs/heads/{branch}", repo_path(project, None)?)]).map(drop)
}

/// The workflow runs in `runs` (`GET …/actions/runs`) not finished: their ids.
pub fn unfinished_runs(runs: &Value) -> Vec<u64> {
    let live = ["queued", "in_progress", "waiting", "requested", "pending"];
    runs.get("workflow_runs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|r| r.get("status").and_then(Value::as_str).is_some_and(|s| live.contains(&s)))
        .filter_map(|r| r.get("id").and_then(Value::as_u64))
        .collect()
}

/// Cancel the workflow runs of commit `sha` still queued or running: their ids.
pub fn cancel_runs(project: &Project, sha: &str) -> Result<Vec<u64>, String> {
    let repo = repo_path(project, None)?;
    let ids = unfinished_runs(&api(project, &[&format!("{repo}/actions/runs?head_sha={sha}&per_page=100")])?);
    for id in &ids {
        api_text(project, &["-X", "POST", &format!("{repo}/actions/runs/{id}/cancel")])?;
    }
    Ok(ids)
}

// ---- Issues ----

/// Issue `n` of `project`'s repository (`of` `None`) or of another.
pub fn issue(project: &Project, of: Option<&str>, n: u64) -> Result<Value, String> {
    let v = api(project, &[&format!("{}/issues/{n}", repo_path(project, of)?)])?;
    issue_from_rest(&v).ok_or_else(|| format!("#{n} is not an issue"))
}

/// The pull requests numbered `numbers` (at most 100) of `project`'s repository, any state, in one
/// call; a number that names no pull request is left out.
pub fn prs(project: &Project, numbers: &[u64]) -> Result<Vec<Value>, String> {
    if numbers.is_empty() {
        return Ok(Vec::new());
    }
    let (owner, name) = owner_name(project, None)?;
    let fields = "id number title body state isDraft headRefName headRefOid baseRefName url isCrossRepository";
    let asks: String = numbers.iter().take(100).map(|n| format!("p{n}: pullRequest(number: {n}) {{ {fields} }} ")).collect();
    let query = format!("query($owner: String!, $name: String!) {{ repository(owner: $owner, name: $name) {{ {asks}}} }}");
    let data = graphql(project, &query, &[("owner", owner), ("name", name)])?;
    let repo = data.get("repository").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut out: Vec<Value> = repo.values().filter_map(pr_from_graphql).collect();
    out.sort_by_key(|p| p.get("number").and_then(Value::as_u64));
    Ok(out)
}

/// The issues numbered `numbers` (at most 100) of `project`'s repository (`of` `None`) or of
/// another, any state, in one call; a number that names no issue is left out.
pub fn issues(project: &Project, of: Option<&str>, numbers: &[u64]) -> Result<Vec<Value>, String> {
    if numbers.is_empty() {
        return Ok(Vec::new());
    }
    let (owner, name) = owner_name(project, of)?;
    let fields = "number title body state labels(first: 50) { nodes { name } }";
    let asks: String = numbers
        .iter()
        .take(100)
        .map(|n| format!("i{n}: issueOrPullRequest(number: {n}) {{ ... on Issue {{ {fields} }} ... on PullRequest {{ {fields} }} }} "))
        .collect();
    let query = format!("query($owner: String!, $name: String!) {{ repository(owner: $owner, name: $name) {{ {asks}}} }}");
    let data = graphql(project, &query, &[("owner", owner), ("name", name)])?;
    let repo = data.get("repository").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut out: Vec<Value> = repo.values().filter_map(issue_from_graphql).collect();
    out.sort_by_key(|i| i.get("number").and_then(Value::as_u64));
    Ok(out)
}

/// The issues of `project`'s repository (`of` `None`) or another's labelled `label`, in `state`
/// (`open`, `closed`, `all`), every page's up to `max_pages`.
pub fn labelled(project: &Project, of: Option<&str>, label: &str, state: &str, max_pages: u32) -> Result<Vec<Value>, String> {
    let repo = repo_path(project, of)?;
    let mut out = Vec::new();
    for page in 1..=max_pages {
        let v = api(project, &[&format!("{repo}/issues?state={state}&labels={}&per_page=100&page={page}", encode(label))])?;
        let rows = v.as_array().cloned().unwrap_or_default();
        out.extend(rows.iter().filter_map(issue_from_rest));
        if rows.len() < 100 {
            break;
        }
    }
    Ok(out)
}

/// The comments on issue (or pull request) `n`, oldest first, every page's: `body` and
/// `created_at` each.
pub fn comments(project: &Project, n: u64) -> Result<Vec<Value>, String> {
    let repo = repo_path(project, None)?;
    let mut out = Vec::new();
    for page in 1..=MAX_PAGES {
        let v = api(project, &[&format!("{repo}/issues/{n}/comments?per_page=100&page={page}")])?;
        let rows = v.as_array().cloned().unwrap_or_default();
        let len = rows.len();
        out.extend(rows);
        if len < 100 {
            break;
        }
    }
    Ok(out)
}

/// Comment `body` on issue (or pull request) `n`.
pub fn comment(project: &Project, n: u64, body: &str) -> Result<(), String> {
    api(project, &["-X", "POST", &format!("{}/issues/{n}/comments", repo_path(project, None)?), "-f", &format!("body={body}")]).map(drop)
}

/// Put `label` on issue `n` of `project`'s repository (`of` `None`) or another's.
pub fn add_label(project: &Project, of: Option<&str>, n: u64, label: &str) -> Result<(), String> {
    api(project, &["-X", "POST", &format!("{}/issues/{n}/labels", repo_path(project, of)?), "-f", &format!("labels[]={label}")]).map(drop)
}

/// Take `label` off issue `n`; one it doesn't carry is no error.
pub fn remove_label(project: &Project, of: Option<&str>, n: u64, label: &str) -> Result<(), String> {
    match api_text(project, &["-X", "DELETE", &format!("{}/issues/{n}/labels/{}", repo_path(project, of)?, encode(label))]) {
        Err(e) if e.contains("(HTTP 404)") && e.contains("Label does not exist") => Ok(()),
        r => r.map(drop),
    }
}

/// Open an issue with `labels`: its number.
pub fn create_issue(project: &Project, title: &str, body: &str, labels: &[&str]) -> Result<u64, String> {
    let path = format!("{}/issues", repo_path(project, None)?);
    let mut fields = vec![format!("title={title}"), format!("body={body}")];
    fields.extend(labels.iter().map(|l| format!("labels[]={l}")));
    let mut args = vec!["-X", "POST", &path];
    for f in &fields {
        args.extend(["-f", f]);
    }
    api(project, &args)?.get("number").and_then(Value::as_u64).ok_or_else(|| "GitHub's answer to the new issue has no number".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::APP_SWARM;

    #[test]
    fn only_the_servers_failures_are_tried_again() {
        assert!(transient("gh api repos/a/b/pulls/1: gh: Bad Gateway (HTTP 502)"));
        assert!(transient("gh api: took longer than 45s"));
        assert!(!transient("gh api repos/a/b/pulls/1/merge: Pull Request is not mergeable (HTTP 405)"));
        assert!(!transient("gh api repos/a/b/pulls/502: Not Found (HTTP 404)"), "a pull request numbered 502");
        let mut calls = 0;
        let r = with_retries(Duration::ZERO, || {
            calls += 1;
            if calls < 3 { Err("gh: Bad Gateway (HTTP 502)".into()) } else { Ok("{}".into()) }
        });
        assert_eq!((r, calls), (Ok("{}".to_owned()), 3));
        let mut calls = 0;
        let r = with_retries(Duration::ZERO, || {
            calls += 1;
            Err("gh: Not Found (HTTP 404)".into())
        });
        assert_eq!((r, calls), (Err("gh: Not Found (HTTP 404)".to_owned()), 1), "the request's failure: once");
    }

    #[test]
    fn repositories_and_path_segments_are_checked() {
        assert_eq!(check_repo("mornycorp/swarm-core"), Ok("mornycorp/swarm-core"));
        for bad in ["", "a", "a/b/c", "../x", "a/..", "a/b?x=1", "-a/b", "a/b c", "a//b"] {
            assert!(check_repo(bad).is_err(), "{bad}");
        }
        assert_eq!(repo_path(&APP_SWARM, None).unwrap(), "repos/example/app-swarm");
        assert_eq!(encode("status::in-progress"), "status%3A%3Ain-progress");
        assert_eq!(encode("a b/c"), "a%20b%2Fc");
    }

    #[test]
    fn the_host_is_the_projects_unless_gh_host_says() {
        let host = |p: &Project, env: Option<&str>| {
            gh_command(p, &["api", "x"], env.map(Into::into)).get_envs().find(|(k, _)| *k == "GH_HOST").and_then(|(_, v)| v.map(|v| v.to_owned()))
        };
        assert_eq!(host(&APP_SWARM, None), None, "github.com is gh's own default");
        let enterprise = Project { github: crate::project::GitHub { host: "github.example.com", ..APP_SWARM.github }, ..APP_SWARM };
        assert_eq!(host(&enterprise, None), Some("github.example.com".into()));
        assert_eq!(host(&enterprise, Some("other.example.com")), None, "GH_HOST wins");
    }

    #[test]
    fn pull_requests_read_into_one_shape() {
        let rest = json!({"number": 7, "state": "closed", "merged": true, "draft": false, "title": "T", "body": null, "html_url": "u", "node_id": "PR_1",
            "head": {"ref": "7-x", "sha": "abc", "repo": {"full_name": "o/r"}}, "base": {"ref": "main", "repo": {"full_name": "o/r"}},
            "mergeable_state": "CLEAN", "comments": 2, "review_comments": 1});
        let p = pr_from_rest(&rest).unwrap();
        assert_eq!(
            (p["number"].as_u64(), p["state"].as_str(), p["head_ref"].as_str(), p["head_sha"].as_str()),
            (Some(7), Some("merged"), Some("7-x"), Some("abc"))
        );
        assert_eq!(
            (p["merge_state"].as_str(), p["comments"].as_u64(), p["body"].as_str(), p["cross_repo"].as_bool()),
            (Some("clean"), Some(3), Some(""), Some(false))
        );
        let fork = json!({"number": 8, "state": "open", "head": {"repo": {"full_name": "f/r"}}, "base": {"repo": {"full_name": "o/r"}}});
        assert_eq!(pr_from_rest(&fork).unwrap()["cross_repo"], true);
        assert!(pr_from_rest(&json!({"title": "no number"})).is_none());
        let node = json!({"id": "PR_2", "number": 9, "state": "OPEN", "isDraft": true, "headRefName": "9-y", "headRefOid": "def", "baseRefName": "main",
            "title": "U", "body": "Closes #9", "url": "v", "isCrossRepository": false, "mergeStateStatus": "BEHIND", "comments": {"totalCount": 4},
            "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "PENDING"}}}]}});
        let g = pr_from_graphql(&node).unwrap();
        assert_eq!(
            (g["state"].as_str(), g["draft"].as_bool(), g["merge_state"].as_str(), g["checks"].as_str(), g["comments"].as_u64()),
            (Some("open"), Some(true), Some("behind"), Some("pending"), Some(4))
        );
        let unchecked = json!({"number": 10, "commits": {"nodes": [{"commit": {"statusCheckRollup": null}}]}});
        assert!(pr_from_graphql(&unchecked).unwrap()["checks"].is_null());
    }

    #[test]
    fn issues_read_into_one_shape_and_pull_requests_are_left_out() {
        let rest = json!({"number": 5, "state": "open", "title": "T", "body": "Needs #4", "labels": [{"name": "status::in-progress"}, {"name": "type::bug"}]});
        let i = issue_from_rest(&rest).unwrap();
        assert_eq!(i["labels"], json!(["status::in-progress", "type::bug"]));
        assert_eq!(i["body"], "Needs #4");
        assert!(issue_from_rest(&json!({"number": 6, "pull_request": {}})).is_none(), "GitHub lists pull requests among issues");
        let node = json!({"number": 3, "state": "MERGED", "title": "x", "labels": {"nodes": [{"name": "a"}]}});
        assert_eq!(issue_from_graphql(&node).unwrap(), json!({"number": 3, "state": "closed", "title": "x", "body": "", "labels": ["a"]}));
        assert_eq!(numbers(&[json!({"number": 1}), json!({}), json!({"number": 2})]), [1, 2]);
    }

    #[test]
    fn only_unfinished_runs_are_cancelled() {
        let runs = json!({"workflow_runs": [{"id": 1, "status": "in_progress"}, {"id": 2, "status": "completed"}, {"id": 3, "status": "queued"}, {"status": "queued"}]});
        assert_eq!(unfinished_runs(&runs), [1, 3]);
        assert!(unfinished_runs(&json!([])).is_empty());
    }

    /// The read calls against GitHub itself: the queries' fields and the answers' shapes. Needs
    /// `gh`, signed in, and the network: `cargo test -p swarm-core --lib -- --ignored live`.
    /// `SWARM_CORE_LIVE_REPO` names the repository (default: this one).
    #[test]
    #[ignore = "needs gh, signed in, and the network"]
    fn live_reads_parse() {
        let repo: &'static str = std::env::var("SWARM_CORE_LIVE_REPO").map_or("mornycorp/swarm-core", |r| Box::leak(r.into_boxed_str()));
        let p = Project { github: crate::project::GitHub { host: "github.com", repo }, ..APP_SWARM };
        let open = open_prs(&p, None).unwrap();
        assert!(open.iter().all(|pr| pr["state"] == "open" && pr["number"].is_u64()), "{open:?}");
        assert!(issues(&p, None, &[999_999]).unwrap().is_empty(), "a missing issue is left out, the rest kept");
        assert!(prs(&p, &[999_999]).unwrap().is_empty());
        assert!(labelled(&p, None, "main-red", "open", 1).is_ok());
        assert!(comments(&p, 999_999).is_err(), "no such issue");
        assert!(unfinished_runs(&api(&p, &[&format!("{}/actions/runs?per_page=1", repo_path(&p, None).unwrap())]).unwrap()).len() <= 1);
    }

    #[test]
    fn a_graphql_answer_with_data_and_errors_keeps_the_data() {
        let both = r#"{"data":{"repository":{"i5":null}},"errors":[{"message":"Could not resolve"}]}"#;
        assert_eq!(data_of(both).unwrap(), json!({"repository": {"i5": null}}));
        assert!(data_of(r#"{"data":null,"errors":[{"message":"Bad credentials"}]}"#).unwrap_err().contains("Bad credentials"));
        assert!(data_of("not json").is_err());
    }
}
