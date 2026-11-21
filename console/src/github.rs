//! GitHub repository URL parsing and AstrBot plugin metadata extraction.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct RepoRef {
    pub owner: String,
    pub repo: String,
    /// Branch from a `/tree/<branch>` URL, otherwise the default branch is used.
    pub branch: Option<String>,
}

impl RepoRef {
    pub fn canonical_url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.repo)
    }
}

fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// Accepts `https://github.com/owner/repo` with optional `.git`, trailing slash or `/tree/<branch>`.
pub fn parse_repo_url(input: &str) -> Option<RepoRef> {
    let url = url::Url::parse(input.trim()).ok()?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    if host != "github.com" && host != "www.github.com" {
        return None;
    }
    let segs: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    if segs.len() < 2 {
        return None;
    }
    let owner = segs[0];
    let repo = segs[1].strip_suffix(".git").unwrap_or(segs[1]);
    if !valid_segment(owner) || !valid_segment(repo) {
        return None;
    }
    let branch = match segs.len() {
        2 => None,
        n if n >= 4 && segs[2] == "tree" => {
            let b = segs[3..].join("/");
            if b.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c)) && !b.contains("..") {
                Some(b)
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some(RepoRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
        branch,
    })
}

pub const API_BASE: &str = "https://api.github.com";
pub const RAW_BASE: &str = "https://raw.githubusercontent.com";

pub fn raw_url(raw_base: &str, r: &RepoRef, branch: &str, file: &str) -> String {
    format!(
        "{}/{}/{}/{}/{}",
        raw_base.trim_end_matches('/'),
        r.owner,
        r.repo,
        branch,
        file
    )
}

pub fn api_repo_url(api_base: &str, r: &RepoRef) -> String {
    format!("{}/repos/{}/{}", api_base.trim_end_matches('/'), r.owner, r.repo)
}

pub const GIT_BASE: &str = "https://github.com";

/// Git smart HTTP ref advertisement of the repository. It is not subject to the REST API rate limit.
pub fn refs_url(git_base: &str, r: &RepoRef) -> String {
    format!(
        "{}/{}/{}.git/info/refs?service=git-upload-pack",
        git_base.trim_end_matches('/'),
        r.owner,
        r.repo
    )
}

pub fn api_commit_url(api_base: &str, r: &RepoRef, reference: &str) -> String {
    format!(
        "{}/repos/{}/{}/commits/{reference}",
        api_base.trim_end_matches('/'),
        r.owner,
        r.repo
    )
}

/// True for a full 40 character hexadecimal commit id.
pub fn is_commit(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Commit of `refs/heads/<branch>` in a git-upload-pack ref advertisement, or of `HEAD` without a branch.
pub fn advertised_commit(body: &[u8], branch: Option<&str>) -> Option<String> {
    let want = match branch {
        Some(b) => format!("refs/heads/{b}"),
        None => "HEAD".to_string(),
    };
    let mut i = 0;
    while i + 4 <= body.len() {
        let len = usize::from_str_radix(std::str::from_utf8(&body[i..i + 4]).ok()?, 16).ok()?;
        if len == 0 {
            i += 4;
            continue;
        }
        if len < 4 || i + len > body.len() {
            return None;
        }
        let payload = &body[i + 4..i + len];
        i += len;
        let line = payload.split(|&b| b == 0).next().unwrap_or_default();
        let line = String::from_utf8_lossy(line);
        if let Some((sha, name)) = line.trim_end_matches('\n').split_once(' ') {
            if name == want && is_commit(sha) {
                return Some(sha.to_ascii_lowercase());
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PluginMeta {
    pub name: String,
    pub display_name: String,
    pub version: String,
    pub author: String,
    pub description: String,
}

fn scalar(v: &serde_yaml::Value) -> Option<String> {
    match v {
        serde_yaml::Value::String(s) => Some(s.trim().to_string()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Plugin directory name: ASCII letters, digits, `_` and `-`, at most 64 chars.
pub fn sanitize_name(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    cleaned.trim_matches('_').to_string()
}

/// Parses `metadata.yaml`. Returns None when the document has no usable `name`.
pub fn parse_metadata(yaml: &str, repo: &RepoRef) -> Option<PluginMeta> {
    let doc: serde_yaml::Value = serde_yaml::from_str(yaml).ok()?;
    let map = doc.as_mapping()?;
    let get = |k: &str| {
        map.get(serde_yaml::Value::String(k.into()))
            .and_then(scalar)
            .filter(|s| !s.is_empty())
    };
    let raw_name = get("name")?;
    let mut name = sanitize_name(&raw_name);
    if name.is_empty() {
        name = sanitize_name(&repo.repo);
    }
    if name.is_empty() {
        return None;
    }
    let clip = |s: String, n: usize| s.chars().take(n).collect::<String>();
    Some(PluginMeta {
        display_name: clip(get("display_name").unwrap_or_else(|| raw_name.clone()), 100),
        name,
        version: clip(get("version").unwrap_or_default(), 50),
        author: clip(get("author").unwrap_or_default(), 100),
        description: clip(get("desc").or_else(|| get("description")).unwrap_or_default(), 1000),
    })
}

/// Parses `_conf_schema.json`, which must be a JSON object of field definitions.
pub fn parse_schema(text: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.is_object() {
        Some(v)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(owner: &str, repo: &str, branch: Option<&str>) -> Option<RepoRef> {
        Some(RepoRef {
            owner: owner.into(),
            repo: repo.into(),
            branch: branch.map(Into::into),
        })
    }

    #[test]
    fn parses_repo_urls() {
        assert_eq!(
            parse_repo_url("https://github.com/Soulter/astrbot_plugin_x"),
            r("Soulter", "astrbot_plugin_x", None)
        );
        assert_eq!(parse_repo_url("https://github.com/a/b.git"), r("a", "b", None));
        assert_eq!(parse_repo_url("https://github.com/a/b/"), r("a", "b", None));
        assert_eq!(parse_repo_url(" https://www.github.com/a/b "), r("a", "b", None));
        assert_eq!(
            parse_repo_url("https://github.com/a/b/tree/dev"),
            r("a", "b", Some("dev"))
        );
        assert_eq!(
            parse_repo_url("https://github.com/a/b/tree/feature/x"),
            r("a", "b", Some("feature/x"))
        );
        assert_eq!(parse_repo_url("https://github.com/a/b?tab=readme"), r("a", "b", None));
        assert_eq!(parse_repo_url("https://github.com/a/b#readme"), r("a", "b", None));
        assert_eq!(parse_repo_url("https://gitlab.com/a/b"), None);
        assert_eq!(parse_repo_url("https://github.com/a"), None);
        assert_eq!(parse_repo_url("https://github.com/a/b/blob/main/x.py"), None);
        assert_eq!(parse_repo_url("https://github.com/a/b/tree/../x"), None);
        assert_eq!(parse_repo_url("ftp://github.com/a/b"), None);
        assert_eq!(parse_repo_url("not a url"), None);
        assert_eq!(parse_repo_url("https://github.com/a/b%20c"), None);
        let r = parse_repo_url("https://github.com/a/b").unwrap();
        assert_eq!(r.canonical_url(), "https://github.com/a/b");
        assert_eq!(
            raw_url(RAW_BASE, &r, "main", "metadata.yaml"),
            "https://raw.githubusercontent.com/a/b/main/metadata.yaml"
        );
        assert_eq!(api_repo_url(API_BASE, &r), "https://api.github.com/repos/a/b");
    }

    #[test]
    fn parses_metadata() {
        let repo = parse_repo_url("https://github.com/x/astrbot_plugin_demo").unwrap();
        let m = parse_metadata(
            "name: astrbot_plugin_demo\ndesc: 演示插件\nversion: v1.2\nauthor: someone\nrepo: https://github.com/x/y\n",
            &repo,
        )
        .unwrap();
        assert_eq!(m.name, "astrbot_plugin_demo");
        assert_eq!(m.display_name, "astrbot_plugin_demo");
        assert_eq!(m.description, "演示插件");
        assert_eq!(m.version, "v1.2");
        let m = parse_metadata(
            "name: 天气 查询\ndisplay_name: 天气\ndescription: |\n  多行\n  描述\nversion: 1.0\n",
            &repo,
        )
        .unwrap();
        assert_eq!(m.name, "astrbot_plugin_demo");
        assert_eq!(m.display_name, "天气");
        assert_eq!(m.description, "多行\n描述");
        assert_eq!(m.version, "1.0");
        assert!(parse_metadata("desc: no name", &repo).is_none());
        assert!(parse_metadata("- a\n- b", &repo).is_none());
        assert!(parse_metadata(":::", &repo).is_none());
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize_name("my-plugin_1"), "my-plugin_1");
        assert_eq!(sanitize_name("../evil"), "evil");
        assert_eq!(sanitize_name("中文"), "");
    }

    #[test]
    fn reads_commits_from_ref_advertisements() {
        fn pkt(line: &str) -> String {
            format!("{:04x}{line}", line.len() + 4)
        }
        let head = "de5ce91a45f102bc73cf8687793f0abb432a8535";
        let dev = "0123456789abcdef0123456789abcdef01234567";
        let body = format!(
            "{}0000{}{}{}0000",
            pkt("# service=git-upload-pack\n"),
            pkt(&format!("{head} HEAD\0multi_ack symref=HEAD:refs/heads/main\n")),
            pkt(&format!("{head} refs/heads/main\n")),
            pkt(&format!("{} refs/heads/dev\n", dev.to_uppercase())),
        );
        assert_eq!(advertised_commit(body.as_bytes(), None).as_deref(), Some(head));
        assert_eq!(advertised_commit(body.as_bytes(), Some("dev")).as_deref(), Some(dev));
        assert_eq!(advertised_commit(body.as_bytes(), Some("missing")), None);
        assert_eq!(advertised_commit(b"<html>sign in</html>", None), None);
        assert_eq!(advertised_commit(b"00ffshort", None), None);
        assert!(is_commit(head) && !is_commit("HEAD") && !is_commit(&head[..39]));
    }

    #[test]
    fn parses_schema() {
        assert!(parse_schema(r#"{"a": {"type": "string"}}"#).is_some());
        assert!(parse_schema("[1]").is_none());
        assert!(parse_schema("{").is_none());
    }
}
