use std::io::Read;
use std::path::{Component as PathComponent, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::logs::LogBuffer;
use crate::model::PluginSpec;

pub const MAX_ZIP_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_UNPACKED_BYTES: u64 = 256 * 1024 * 1024;
const MARKER: &str = ".fish_install.json";

#[derive(Debug, PartialEq, Eq)]
pub struct Repo {
    pub owner: String,
    pub name: String,
    pub branch: Option<String>,
}

/// Parses `https://github.com/owner/repo` with optional `.git`, trailing slash and `/tree/<branch>`.
pub fn parse_repo(url: &str) -> Result<Repo> {
    let u = url.trim();
    let rest = u
        .strip_prefix("https://github.com/")
        .or_else(|| u.strip_prefix("http://github.com/"))
        .or_else(|| u.strip_prefix("github.com/"))
        .ok_or_else(|| anyhow!("仅支持 GitHub 仓库地址"))?;
    let parts: Vec<&str> = rest
        .trim_end_matches('/')
        .split('/')
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() < 2 {
        return Err(anyhow!("仓库地址格式错误"));
    }
    let valid =
        |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && s != "..";
    let owner = parts[0].to_string();
    let name = parts[1].trim_end_matches(".git").to_string();
    if !valid(&owner) || !valid(&name) {
        return Err(anyhow!("仓库地址格式错误"));
    }
    let branch = if parts.len() >= 4 && parts[2] == "tree" {
        Some(parts[3..].join("/"))
    } else {
        None
    };
    Ok(Repo { owner, name, branch })
}

/// Candidate codeload URLs in the order they are tried.
pub fn zip_urls(repo: &Repo) -> Vec<String> {
    let base = format!("https://codeload.github.com/{}/{}/zip", repo.owner, repo.name);
    match &repo.branch {
        Some(b) => vec![format!("{base}/refs/heads/{b}"), format!("{base}/{b}")],
        None => vec![
            format!("{base}/HEAD"),
            format!("{base}/refs/heads/main"),
            format!("{base}/refs/heads/master"),
        ],
    }
}

pub async fn download_zip(http: &reqwest::Client, urls: &[String]) -> Result<Vec<u8>> {
    let mut last = anyhow!("没有可用的下载地址");
    for url in urls {
        let resp = match http.get(url).timeout(Duration::from_secs(120)).send().await {
            Ok(r) => r,
            Err(e) => {
                last = anyhow!("下载 {url} 失败: {e}");
                continue;
            }
        };
        if !resp.status().is_success() {
            last = anyhow!("下载 {url} 返回 {}", resp.status());
            continue;
        }
        if resp.content_length().is_some_and(|l| l as usize > MAX_ZIP_BYTES) {
            return Err(anyhow!("插件压缩包超过 {} MB", MAX_ZIP_BYTES / 1024 / 1024));
        }
        let mut buf = Vec::new();
        let mut resp = resp;
        while let Some(chunk) = resp.chunk().await? {
            buf.extend_from_slice(&chunk);
            if buf.len() > MAX_ZIP_BYTES {
                return Err(anyhow!("插件压缩包超过 {} MB", MAX_ZIP_BYTES / 1024 / 1024));
            }
        }
        return Ok(buf);
    }
    Err(last)
}

/// Extracts a GitHub archive into `dest`, dropping the single top level directory.
pub fn extract_zip(bytes: &[u8], dest: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).context("压缩包损坏")?;
    std::fs::create_dir_all(dest)?;
    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let Some(path) = file.enclosed_name() else {
            continue;
        };
        let rel: PathBuf = path.components().skip(1).collect();
        if rel.as_os_str().is_empty() || rel.components().any(|c| !matches!(c, PathComponent::Normal(_))) {
            continue;
        }
        let is_symlink = file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000);
        if is_symlink {
            continue;
        }
        let out = dest.join(&rel);
        if file.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        total += file.size();
        if total > MAX_UNPACKED_BYTES {
            return Err(anyhow!("插件解压后超过 {} MB", MAX_UNPACKED_BYTES / 1024 / 1024));
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut data = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut data)?;
        std::fs::write(&out, data)?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Marker {
    repo_url: String,
    version: String,
    name: String,
}

pub struct Installed {
    pub src: PathBuf,
    pub venv: PathBuf,
    pub site_packages: Option<PathBuf>,
    pub data: PathBuf,
}

pub struct Installer {
    pub http: reqwest::Client,
    pub uv: String,
    pub python: String,
    pub cache_dir: PathBuf,
    pub codeload_base: Option<String>,
}

pub fn find_site_packages(venv: &Path) -> Option<PathBuf> {
    let lib = venv.join("lib");
    let entries = std::fs::read_dir(&lib).ok()?;
    for e in entries.flatten() {
        let sp = e.path().join("site-packages");
        if sp.is_dir() {
            return Some(sp);
        }
    }
    let win = venv.join("Lib").join("site-packages");
    win.is_dir().then_some(win)
}

pub fn layout(plugin_dir: &Path) -> Installed {
    let venv = plugin_dir.join("venv");
    Installed {
        src: plugin_dir.join("src"),
        site_packages: find_site_packages(&venv),
        venv,
        data: plugin_dir.join("data"),
    }
}

/// True when the plugin directory already holds an install of this spec.
pub fn is_installed(spec: &PluginSpec, plugin_dir: &Path) -> bool {
    let want = Marker {
        repo_url: spec.repo_url.clone(),
        version: spec.version.clone(),
        name: spec.name.clone(),
    };
    std::fs::read_to_string(plugin_dir.join(MARKER))
        .ok()
        .and_then(|s| serde_json::from_str::<Marker>(&s).ok())
        .is_some_and(|m| m == want)
        && plugin_dir.join("src").is_dir()
        && plugin_dir.join("venv").is_dir()
}

impl Installer {
    /// Downloads the plugin, creates its venv and installs requirements. Skips work when already installed.
    pub async fn ensure(
        &self,
        spec: &PluginSpec,
        plugin_dir: &Path,
        logs: &Arc<LogBuffer>,
        source: &str,
    ) -> Result<Installed> {
        if is_installed(spec, plugin_dir) {
            return Ok(layout(plugin_dir));
        }
        let repo = parse_repo(&spec.repo_url)?;
        logs.info(source, format!("下载插件 {}/{}", repo.owner, repo.name));
        let mut urls = zip_urls(&repo);
        if let Some(base) = &self.codeload_base {
            for url in &mut urls {
                *url = url.replacen("https://codeload.github.com", base.trim_end_matches('/'), 1);
            }
        }
        let bytes = download_zip(&self.http, &urls).await?;
        std::fs::create_dir_all(plugin_dir)?;
        let _ = std::fs::remove_file(plugin_dir.join(MARKER));
        let tmp = plugin_dir.join("src.tmp");
        let _ = std::fs::remove_dir_all(&tmp);
        let tmp2 = tmp.clone();
        tokio::task::spawn_blocking(move || extract_zip(&bytes, &tmp2)).await??;
        let src = plugin_dir.join("src");
        let _ = std::fs::remove_dir_all(&src);
        std::fs::rename(&tmp, &src)?;
        std::fs::create_dir_all(plugin_dir.join("data"))?;

        let venv = plugin_dir.join("venv");
        let _ = std::fs::remove_dir_all(&venv);
        logs.info(source, "创建虚拟环境");
        let mut cmd = tokio::process::Command::new(&self.uv);
        cmd.arg("venv")
            .arg("--system-site-packages")
            .arg("--python")
            .arg(&self.python)
            .arg(&venv);
        self.run(cmd, logs, source, Duration::from_secs(120)).await?;

        let req = src.join("requirements.txt");
        if req.is_file() {
            logs.info(source, "安装依赖 requirements.txt");
            let py = venv.join("bin").join("python");
            let mut cmd = tokio::process::Command::new(&self.uv);
            cmd.arg("pip")
                .arg("install")
                .arg("--python")
                .arg(&py)
                .arg("-r")
                .arg(&req);
            self.run(cmd, logs, source, Duration::from_secs(900)).await?;
        }
        let marker = Marker {
            repo_url: spec.repo_url.clone(),
            version: spec.version.clone(),
            name: spec.name.clone(),
        };
        std::fs::write(plugin_dir.join(MARKER), serde_json::to_vec(&marker)?)?;
        logs.info(source, "插件安装完成");
        Ok(layout(plugin_dir))
    }

    async fn run(
        &self,
        mut cmd: tokio::process::Command,
        logs: &Arc<LogBuffer>,
        source: &str,
        timeout: Duration,
    ) -> Result<()> {
        cmd.env("UV_CACHE_DIR", &self.cache_dir)
            .env("UV_PYTHON_DOWNLOADS", "never")
            .env("UV_NO_PROGRESS", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().with_context(|| format!("无法运行 {}", self.uv))?;
        let mut readers = vec![];
        for pipe in [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        ]
        .into_iter()
        .flatten()
        {
            let logs = logs.clone();
            let source = source.to_string();
            readers.push(tokio::spawn(async move {
                let mut lines = BufReader::new(pipe).lines();
                let mut n = 0;
                while let Ok(Some(l)) = lines.next_line().await {
                    n += 1;
                    if n <= 200 && !l.trim().is_empty() {
                        logs.info(&source, l);
                    }
                }
            }));
        }
        let status = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(s) => s?,
            Err(_) => {
                let _ = child.kill().await;
                return Err(anyhow!("安装超时"));
            }
        };
        for r in readers {
            let _ = r.await;
        }
        if !status.success() {
            return Err(anyhow!("uv 执行失败: {status}"));
        }
        Ok(())
    }
}

/// Total apparent size of regular files under `path`, without following symlinks.
pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&p) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(md) = e.path().symlink_metadata() else {
                continue;
            };
            if md.is_dir() {
                stack.push(e.path());
            } else if md.is_file() {
                total += md.len();
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn repo_urls() {
        let r = parse_repo("https://github.com/Soulter/astrbot_plugin_x.git/").unwrap();
        assert_eq!(
            r,
            Repo {
                owner: "Soulter".into(),
                name: "astrbot_plugin_x".into(),
                branch: None
            }
        );
        let r = parse_repo("https://github.com/a/b/tree/dev").unwrap();
        assert_eq!(r.branch.as_deref(), Some("dev"));
        assert_eq!(zip_urls(&r)[0], "https://codeload.github.com/a/b/zip/refs/heads/dev");
        assert!(parse_repo("https://gitlab.com/a/b").is_err());
        assert!(parse_repo("https://github.com/a").is_err());
        assert!(parse_repo("https://github.com/../b").is_err());
    }

    #[test]
    fn extract_strips_top_dir_and_blocks_traversal() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            w.add_directory("repo-main/", opts).unwrap();
            w.start_file("repo-main/main.py", opts).unwrap();
            w.write_all(b"print(1)").unwrap();
            w.start_file("repo-main/pkg/a.txt", opts).unwrap();
            w.write_all(b"a").unwrap();
            w.start_file("../evil.txt", opts).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("src");
        extract_zip(buf.get_ref(), &dest).unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("main.py")).unwrap(), "print(1)");
        assert!(dest.join("pkg/a.txt").is_file());
        assert!(!dir.path().join("evil.txt").exists());
        assert_eq!(dir_size(&dest), 9);
    }
}
