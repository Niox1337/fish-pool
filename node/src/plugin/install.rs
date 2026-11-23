use std::collections::HashSet;
use std::io::Read;
use std::path::{Component as PathComponent, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::config::safe_name;
use crate::logs::LogBuffer;
use crate::model::PluginSpec;

pub const MAX_ZIP_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_UNPACKED_BYTES: u64 = 256 * 1024 * 1024;
const MARKER: &str = ".fish_install.json";
const SHARED_MARKER: &str = ".fish_shared.json";
const MAX_OUTPUT_LINES: usize = 20_000;

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

/// True for a full 40 character hexadecimal commit id.
pub fn is_commit(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Candidate codeload URLs in the order they are tried. A valid `commit` pins the archive to that commit.
pub fn zip_urls(repo: &Repo, commit: &str) -> Vec<String> {
    let base = format!("https://codeload.github.com/{}/{}/zip", repo.owner, repo.name);
    if is_commit(commit) {
        return vec![format!("{base}/{}", commit.to_ascii_lowercase())];
    }
    match &repo.branch {
        Some(b) => vec![format!("{base}/refs/heads/{b}"), format!("{base}/{b}")],
        None => vec![
            format!("{base}/HEAD"),
            format!("{base}/refs/heads/main"),
            format!("{base}/refs/heads/master"),
        ],
    }
}

pub const CODELOAD: &str = "https://codeload.github.com";
/// Console route that streams codeload archives for signed node requests.
pub const ARCHIVE_PROXY_PATH: &str = "/api/internal/plugin-archive";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const RETRY_DELAY: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Proxy,
    Direct,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Proxy => "经 Cloudflare 中转",
            Source::Direct => "直连 GitHub",
        }
    }
}

pub struct Downloaded {
    pub bytes: Vec<u8>,
    pub source: Source,
}

enum Attempt {
    Done(Vec<u8>),
    /// The ref does not exist. The next candidate URL is tried.
    NotFound(anyhow::Error),
    /// Network error, timeout, 429 or 5xx. Retried once.
    Transient(anyhow::Error),
    /// Other failures. The next source is tried without retry.
    Failed(anyhow::Error),
    TooLarge,
}

fn too_large() -> anyhow::Error {
    anyhow!("插件压缩包超过 {} MB", MAX_ZIP_BYTES / 1024 / 1024)
}

async fn fetch_once(req: reqwest::RequestBuilder, what: &str) -> Attempt {
    let mut resp = match req.timeout(DOWNLOAD_TIMEOUT).send().await {
        Ok(r) => r,
        Err(e) => return Attempt::Transient(anyhow!("{what} 失败: {e}")),
    };
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Attempt::NotFound(anyhow!("{what} 返回 404"));
    }
    if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
        return Attempt::TooLarge;
    }
    if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Attempt::Transient(anyhow!("{what} 返回 {status}"));
    }
    if !status.is_success() {
        return Attempt::Failed(anyhow!("{what} 返回 {status}"));
    }
    if resp.content_length().is_some_and(|l| l as usize > MAX_ZIP_BYTES) {
        return Attempt::TooLarge;
    }
    let mut buf = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                buf.extend_from_slice(&chunk);
                if buf.len() > MAX_ZIP_BYTES {
                    return Attempt::TooLarge;
                }
            }
            Ok(None) => return Attempt::Done(buf),
            Err(e) => return Attempt::Transient(anyhow!("{what} 中断: {e}")),
        }
    }
}

/// Downloads the first available archive among codeload `urls`.
///
/// Each URL is tried through the console proxy first, then directly. `direct_base` replaces the
/// codeload origin for direct requests. Transient failures are retried once per source.
pub async fn download_zip(
    http: &reqwest::Client,
    proxy: Option<&crate::sign::SignedClient>,
    direct_base: Option<&str>,
    urls: &[String],
) -> Result<Downloaded> {
    download_zip_with(http, proxy, direct_base, urls, RETRY_DELAY).await
}

async fn download_zip_with(
    http: &reqwest::Client,
    proxy: Option<&crate::sign::SignedClient>,
    direct_base: Option<&str>,
    urls: &[String],
    retry_delay: Duration,
) -> Result<Downloaded> {
    let mut last = anyhow!("没有可用的下载地址");
    'urls: for url in urls {
        let rest = url.strip_prefix(CODELOAD);
        let direct = match (direct_base, rest) {
            (Some(base), Some(rest)) => format!("{}{rest}", base.trim_end_matches('/')),
            _ => url.clone(),
        };
        for source in [Source::Proxy, Source::Direct] {
            let proxy_target = match source {
                Source::Proxy => match (proxy, rest) {
                    (Some(p), Some(rest)) => Some((p, format!("{ARCHIVE_PROXY_PATH}{rest}"))),
                    _ => continue,
                },
                Source::Direct => None,
            };
            let what = format!("{}下载 {url}", source.label());
            for attempt in 0..2 {
                if attempt > 0 {
                    tokio::time::sleep(retry_delay).await;
                }
                let req = match &proxy_target {
                    Some((p, path)) => p.get(path),
                    None => http.get(&direct),
                };
                match fetch_once(req, &what).await {
                    Attempt::Done(bytes) => return Ok(Downloaded { bytes, source }),
                    Attempt::NotFound(e) => {
                        last = e;
                        continue 'urls;
                    }
                    Attempt::Transient(e) => last = e,
                    Attempt::Failed(e) => {
                        last = e;
                        break;
                    }
                    Attempt::TooLarge => return Err(too_large()),
                }
            }
        }
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

/// Install record in a bot's plugin directory.
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
struct Marker {
    repo_url: String,
    version: String,
    name: String,
    #[serde(default)]
    python: String,
    #[serde(default)]
    commit: String,
    #[serde(default)]
    install_rev: i64,
    /// Name of the shared install under `Installer::shared_dir` that holds the code and venv.
    #[serde(default)]
    shared: String,
}

pub struct Installed {
    pub src: PathBuf,
    pub venv: PathBuf,
    pub site_packages: Option<PathBuf>,
    pub data: PathBuf,
    /// Name of the shared install. Bots using the same name run the same code and venv.
    pub share_key: String,
}

/// Largest shared install (code and dependencies) that a build may produce.
pub const MAX_SHARED_INSTALL_BYTES: u64 = 1024 * 1024 * 1024;

pub struct Installer {
    pub http: reqwest::Client,
    pub uv: String,
    pub python: String,
    /// `major.minor` of `python`, recorded in the install marker so venvs built for another interpreter are rebuilt.
    pub python_version: String,
    pub cache_dir: PathBuf,
    /// Holds one directory per distinct plugin archive and interpreter, with `src`, `venv` and `SHARED_MARKER`.
    pub shared_dir: PathBuf,
    pub codeload_base: Option<String>,
    pub archive_proxy: Option<crate::sign::SignedClient>,
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

/// Returns `major.minor` of the given interpreter, or an empty string when it cannot be run.
pub fn python_version(python: &str) -> String {
    std::process::Command::new(python)
        .args(["-c", "import sys; print('%d.%d' % sys.version_info[:2])"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Identifies the plugin source: the pinned commit, or the archive digest when no commit is pinned.
pub fn source_id(commit: &str, archive: &[u8]) -> String {
    if is_commit(commit) {
        format!("commit:{}", commit.to_ascii_lowercase())
    } else {
        format!("sha256:{}", hex::encode(Sha256::digest(archive)))
    }
}

/// Shared install name from the plugin name, the source, the resolved dependency set and the interpreter version.
/// Installs share code and venv only when all of them match.
pub fn share_key(name: &str, source: &str, deps: &str, python_version: &str) -> String {
    let mut h = Sha256::new();
    h.update(source.as_bytes());
    h.update(b"\n");
    h.update(deps.as_bytes());
    let digest = hex::encode(&h.finalize()[..8]);
    let py = if python_version.is_empty() { "x" } else { python_version };
    format!("{}-{digest}-py{py}", safe_name(name))
}

/// Package changes from the output of `uv pip install --dry-run`, sorted. Lines look like ` + name==1.0`.
pub fn dry_run_changes(lines: &[String]) -> String {
    let mut changes: Vec<&str> = lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| l.starts_with("+ ") || l.starts_with("- "))
        .collect();
    changes.sort_unstable();
    changes.dedup();
    changes.join("\n")
}

fn read_marker(plugin_dir: &Path) -> Option<Marker> {
    let text = std::fs::read_to_string(plugin_dir.join(MARKER)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Share keys recorded by the bots under `tenants_dir`.
fn referenced_keys(tenants_dir: &Path) -> HashSet<String> {
    let mut keys = HashSet::new();
    let level = |p: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(p)
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
            .unwrap_or_default()
    };
    for user in level(tenants_dir) {
        for bot in level(&user) {
            for plugin in level(&bot.join("plugins")) {
                if let Some(m) = read_marker(&plugin) {
                    if !m.shared.is_empty() {
                        keys.insert(m.shared);
                    }
                }
            }
        }
    }
    keys
}

impl Installer {
    fn marker(&self, spec: &PluginSpec, shared: &str) -> Marker {
        Marker {
            repo_url: spec.repo_url.clone(),
            version: spec.version.clone(),
            name: spec.name.clone(),
            python: self.python_version.clone(),
            commit: spec.commit.clone(),
            install_rev: spec.install_rev,
            shared: shared.to_string(),
        }
    }

    fn shared_entry(&self, key: &str) -> PathBuf {
        self.shared_dir.join(key)
    }

    /// True when the shared install finished building.
    fn shared_complete(&self, key: &str) -> bool {
        let entry = self.shared_entry(key);
        !key.is_empty()
            && !key.contains(['/', '\\'])
            && key != ".."
            && entry.join(SHARED_MARKER).is_file()
            && entry.join("src").is_dir()
            && entry.join("venv").is_dir()
    }

    fn layout(&self, plugin_dir: &Path, key: &str) -> Installed {
        let entry = self.shared_entry(key);
        let venv = entry.join("venv");
        Installed {
            src: entry.join("src"),
            site_packages: find_site_packages(&venv),
            venv,
            data: plugin_dir.join("data"),
            share_key: key.to_string(),
        }
    }

    /// The install of this spec for the current interpreter, when the bot's plugin directory records one.
    pub fn installed(&self, spec: &PluginSpec, plugin_dir: &Path) -> Option<Installed> {
        let m = read_marker(plugin_dir)?;
        (m == self.marker(spec, &m.shared) && self.shared_complete(&m.shared))
            .then(|| self.layout(plugin_dir, &m.shared))
    }

    pub fn is_installed(&self, spec: &PluginSpec, plugin_dir: &Path) -> bool {
        self.installed(spec, plugin_dir).is_some()
    }

    /// Size of the shared install the bot's plugin directory uses, zero when it has none.
    pub fn shared_size(&self, plugin_dir: &Path) -> u64 {
        match read_marker(plugin_dir) {
            Some(m) if self.shared_complete(&m.shared) => dir_size(&self.shared_entry(&m.shared)),
            _ => 0,
        }
    }

    /// Downloads the plugin, creates its venv and installs requirements. Skips work when already installed.
    /// The code and venv go to a shared install named by `share_key`, which other bots reuse. The key
    /// includes the dependency versions a fresh install would pick now, so a reused install never has
    /// older dependencies than a new one.
    /// Callers serialize calls, and `gc_shared`, with one lock.
    pub async fn ensure(
        &self,
        spec: &PluginSpec,
        plugin_dir: &Path,
        logs: &Arc<LogBuffer>,
        source: &str,
    ) -> Result<Installed> {
        if let Some(inst) = self.installed(spec, plugin_dir) {
            return Ok(inst);
        }
        let repo = parse_repo(&spec.repo_url)?;
        logs.info(source, format!("下载插件 {}/{}", repo.owner, repo.name));
        let urls = zip_urls(&repo, &spec.commit);
        let Downloaded { bytes, source: via } = download_zip(
            &self.http,
            self.archive_proxy.as_ref(),
            self.codeload_base.as_deref(),
            &urls,
        )
        .await?;
        logs.info(
            source,
            format!("插件{}下载完成 ({} KB)", via.label(), bytes.len() / 1024),
        );
        let staging = self
            .shared_dir
            .join(format!(".staging-{}", hex::encode(rand::random::<[u8; 8]>())));
        let result = self.place(spec, &bytes, &staging, logs, source).await;
        if staging.exists() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        let key = result?;
        std::fs::create_dir_all(plugin_dir.join("data"))?;
        std::fs::write(plugin_dir.join(MARKER), serde_json::to_vec(&self.marker(spec, &key))?)?;
        for legacy in ["src", "src.tmp", "venv"] {
            let p = plugin_dir.join(legacy);
            if p.exists() {
                let _ = std::fs::remove_dir_all(p);
            }
        }
        logs.info(source, "插件安装完成");
        Ok(self.layout(plugin_dir, &key))
    }

    /// Extracts the archive to `staging`, resolves the dependencies and builds the shared install unless
    /// one with the same key exists. Returns the key.
    async fn place(
        &self,
        spec: &PluginSpec,
        bytes: &[u8],
        staging: &Path,
        logs: &Arc<LogBuffer>,
        source: &str,
    ) -> Result<String> {
        let src = staging.join("src");
        let (data, dest) = (bytes.to_vec(), src.clone());
        tokio::task::spawn_blocking(move || extract_zip(&data, &dest)).await??;
        let deps = self.resolve_deps(&src, &staging.join("venv"), logs, source).await?;
        let key = share_key(&spec.name, &source_id(&spec.commit, bytes), &deps, &self.python_version);
        if self.shared_complete(&key) {
            logs.info(source, "其他机器人已安装同一版本和相同依赖，直接使用已安装的代码和依赖");
        } else {
            self.build_shared(&key, &src, logs, source).await?;
        }
        Ok(key)
    }

    /// Resolves `requirements.txt` against a throwaway venv that sees the system packages, the same way
    /// the real install does, and returns the package changes it would make. Empty without requirements.
    async fn resolve_deps(&self, src: &Path, venv: &Path, logs: &Arc<LogBuffer>, source: &str) -> Result<String> {
        let req = src.join("requirements.txt");
        if !req.is_file() {
            return Ok(String::new());
        }
        logs.info(source, "解析依赖版本");
        let mut cmd = tokio::process::Command::new(&self.uv);
        cmd.arg("venv")
            .arg("--system-site-packages")
            .arg("--python")
            .arg(&self.python)
            .arg(venv);
        self.run_quiet(cmd, logs, source, Duration::from_secs(120)).await?;
        let mut cmd = tokio::process::Command::new(&self.uv);
        cmd.args(["pip", "install", "--dry-run", "--python"])
            .arg(venv.join("bin").join("python"))
            .arg("-r")
            .arg(&req);
        let lines = self.run_quiet(cmd, logs, source, Duration::from_secs(300)).await?;
        Ok(dry_run_changes(&lines))
    }

    async fn build_shared(&self, key: &str, staged_src: &Path, logs: &Arc<LogBuffer>, source: &str) -> Result<()> {
        let entry = self.shared_entry(key);
        if entry.exists() {
            std::fs::remove_dir_all(&entry)?;
        }
        std::fs::create_dir_all(&entry)?;
        let src = entry.join("src");
        std::fs::rename(staged_src, &src)?;

        let venv = entry.join("venv");
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
                .arg("--compile-bytecode")
                .arg("-r")
                .arg(&req);
            self.run(cmd, logs, source, Duration::from_secs(900)).await?;
        }
        // The plugin process cannot write bytecode caches next to the source, so they are built here.
        let mut cmd = tokio::process::Command::new(&self.python);
        cmd.args(["-m", "compileall", "-q", "-j", "0"]).arg(&src);
        if self.run(cmd, logs, source, Duration::from_secs(120)).await.is_err() {
            logs.warn(source, "部分插件源码无法预编译，加载时会再次编译");
        }
        let size = dir_size(&entry);
        if size > MAX_SHARED_INSTALL_BYTES {
            let _ = std::fs::remove_dir_all(&entry);
            return Err(anyhow!(
                "插件代码和依赖共 {} MB，超过单个插件 {} MB 的上限",
                size / (1024 * 1024),
                MAX_SHARED_INSTALL_BYTES / (1024 * 1024)
            ));
        }
        std::fs::write(entry.join(SHARED_MARKER), b"{}")?;
        Ok(())
    }

    /// Removes shared installs that no bot under `tenants_dir` records, including unfinished builds.
    /// Returns the number of removed installs. Callers hold the lock that serializes `ensure`.
    pub fn gc_shared(&self, tenants_dir: &Path) -> usize {
        let Ok(rd) = std::fs::read_dir(&self.shared_dir) else {
            return 0;
        };
        let keep = referenced_keys(tenants_dir);
        let mut removed = 0;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if keep.contains(&name) && self.shared_complete(&name) {
                continue;
            }
            if std::fs::remove_dir_all(e.path()).is_ok() {
                removed += 1;
            }
        }
        removed
    }

    async fn run(
        &self,
        cmd: tokio::process::Command,
        logs: &Arc<LogBuffer>,
        source: &str,
        timeout: Duration,
    ) -> Result<()> {
        self.run_with(cmd, logs, source, timeout, true).await.map(|_| ())
    }

    /// Runs the command and returns its output lines. Output reaches the log only when the command fails.
    async fn run_quiet(
        &self,
        cmd: tokio::process::Command,
        logs: &Arc<LogBuffer>,
        source: &str,
        timeout: Duration,
    ) -> Result<Vec<String>> {
        self.run_with(cmd, logs, source, timeout, false).await
    }

    async fn run_with(
        &self,
        mut cmd: tokio::process::Command,
        logs: &Arc<LogBuffer>,
        source: &str,
        timeout: Duration,
        log_output: bool,
    ) -> Result<Vec<String>> {
        cmd.env("UV_CACHE_DIR", &self.cache_dir)
            .env("UV_PYTHON_DOWNLOADS", "never")
            .env("UV_NO_PROGRESS", "1")
            .env_remove("RUST_LOG")
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
                let mut kept = Vec::new();
                let mut n = 0;
                while let Ok(Some(l)) = lines.next_line().await {
                    n += 1;
                    if log_output && n <= 200 && !l.trim().is_empty() {
                        logs.info(&source, l.clone());
                    }
                    if kept.len() < MAX_OUTPUT_LINES {
                        kept.push(l);
                    }
                }
                kept
            }));
        }
        let status = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(s) => s?,
            Err(_) => {
                let _ = child.kill().await;
                return Err(anyhow!("安装超时"));
            }
        };
        let mut output = Vec::new();
        for r in readers {
            output.extend(r.await.unwrap_or_default());
        }
        if !status.success() {
            if !log_output {
                for l in output.iter().filter(|l| !l.trim().is_empty()).take(200) {
                    logs.info(source, l.clone());
                }
            }
            return Err(anyhow!("uv 执行失败: {status}"));
        }
        Ok(output)
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

    type Hits = Arc<std::sync::Mutex<Vec<String>>>;

    fn demo_spec() -> PluginSpec {
        PluginSpec {
            id: "p".into(),
            name: "demo".into(),
            repo_url: "https://github.com/o/demo".into(),
            version: "1".into(),
            enabled: true,
            config: serde_json::Value::Null,
            ..Default::default()
        }
    }

    fn test_installer(root: &Path, python_version: &str) -> Installer {
        Installer {
            http: reqwest::Client::new(),
            uv: "uv".into(),
            python: "python3".into(),
            python_version: python_version.into(),
            cache_dir: root.join("cache"),
            shared_dir: root.join("shared"),
            codeload_base: None,
            archive_proxy: None,
        }
    }

    fn fake_shared(installer: &Installer, key: &str) {
        let entry = installer.shared_dir.join(key);
        std::fs::create_dir_all(entry.join("src")).unwrap();
        std::fs::create_dir_all(entry.join("venv/lib/python3.13/site-packages")).unwrap();
        std::fs::write(entry.join(SHARED_MARKER), b"{}").unwrap();
    }

    #[test]
    fn install_marker_tracks_interpreter_version_and_shared_install() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("tenants/u/b/plugins/demo");
        std::fs::create_dir_all(plugin_dir.join("src")).unwrap();
        std::fs::create_dir_all(plugin_dir.join("venv")).unwrap();
        let spec = demo_spec();
        let legacy = r#"{"repo_url":"https://github.com/o/demo","version":"1","name":"demo","python":"3.13"}"#;
        std::fs::write(plugin_dir.join(MARKER), legacy).unwrap();
        let current = test_installer(dir.path(), "3.13");
        assert!(
            !current.is_installed(&spec, &plugin_dir),
            "installs without a shared install are rebuilt"
        );

        let key = share_key("demo", &source_id("", b"zip"), "", "3.13");
        assert!(key.starts_with("demo-") && key.ends_with("-py3.13"));
        std::fs::write(
            plugin_dir.join(MARKER),
            serde_json::to_vec(&current.marker(&spec, &key)).unwrap(),
        )
        .unwrap();
        assert!(
            !current.is_installed(&spec, &plugin_dir),
            "the shared install is missing"
        );
        fake_shared(&current, &key);
        let inst = current.installed(&spec, &plugin_dir).unwrap();
        assert_eq!(inst.src, current.shared_dir.join(&key).join("src"));
        assert_eq!(inst.data, plugin_dir.join("data"));
        assert_eq!(inst.share_key, key);
        assert!(inst.site_packages.unwrap().ends_with("site-packages"));
        assert!(!test_installer(dir.path(), "3.11").is_installed(&spec, &plugin_dir));
        std::fs::remove_file(current.shared_dir.join(&key).join(SHARED_MARKER)).unwrap();
        assert!(
            !current.is_installed(&spec, &plugin_dir),
            "an unfinished build is not installed"
        );
    }

    #[test]
    fn new_commit_or_install_rev_needs_a_new_install() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("p");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        let installer = test_installer(dir.path(), "3.13");
        let spec = PluginSpec {
            commit: "a".repeat(40),
            install_rev: 1,
            ..demo_spec()
        };
        std::fs::write(
            plugin_dir.join(MARKER),
            serde_json::to_vec(&installer.marker(&spec, "k")).unwrap(),
        )
        .unwrap();
        fake_shared(&installer, "k");
        assert!(installer.is_installed(&spec, &plugin_dir));
        for other in [
            PluginSpec {
                commit: "b".repeat(40),
                ..spec.clone()
            },
            PluginSpec {
                install_rev: 2,
                ..spec.clone()
            },
        ] {
            assert!(spec.needs_reinstall(&other));
            assert!(!installer.is_installed(&other, &plugin_dir));
        }
        assert!(!spec.needs_reinstall(&PluginSpec {
            config: serde_json::json!({"a": 1}),
            ..spec.clone()
        }));
    }

    #[test]
    fn share_key_separates_sources_and_dependency_sets() {
        let commit = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            source_id(commit, b"x"),
            source_id(&commit.to_uppercase(), b"y"),
            "a pinned commit ignores archive bytes"
        );
        assert_ne!(source_id("", b"x"), source_id("", b"y"));
        assert_ne!(
            source_id("main", b"x"),
            source_id("main", b"y"),
            "only full commit ids pin"
        );
        let src = source_id(commit, b"");
        let base = share_key("demo", &src, "+ a==1", "3.13");
        assert_eq!(base, share_key("demo", &src, "+ a==1", "3.13"));
        assert_ne!(
            base,
            share_key("demo", &src, "+ a==2", "3.13"),
            "newer dependencies get their own install"
        );
        assert_ne!(base, share_key("demo", &source_id("", b"z"), "+ a==1", "3.13"));
        assert_ne!(base, share_key("demo", &src, "+ a==1", "3.14"));
    }

    #[test]
    fn dry_run_changes_keep_only_sorted_package_lines() {
        let out: Vec<String> = [
            "Using Python 3.13.1 environment at: v",
            "Resolved 3 packages in 20ms",
            "Would install 2 packages",
            " + rich==15.0.0",
            " + idna==3.20",
            " - idna==3.10",
            "",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(dry_run_changes(&out), "+ idna==3.20\n+ rich==15.0.0\n- idna==3.10");
        assert_eq!(dry_run_changes(&["Audited 2 packages in 1ms".to_string()]), "");
    }

    #[test]
    fn pinned_commit_downloads_only_that_commit() {
        let repo = Repo {
            owner: "a".into(),
            name: "b".into(),
            branch: Some("dev".into()),
        };
        let commit = "0123456789ABCDEF0123456789abcdef01234567";
        assert_eq!(
            zip_urls(&repo, commit),
            vec![format!("https://codeload.github.com/a/b/zip/{}", commit.to_lowercase())]
        );
        assert_eq!(zip_urls(&repo, "").len(), 2);
        assert_eq!(zip_urls(&repo, "short").len(), 2);
    }

    #[test]
    fn gc_shared_keeps_only_referenced_installs() {
        let dir = tempfile::tempdir().unwrap();
        let installer = test_installer(dir.path(), "3.13");
        let tenants = dir.path().join("tenants");
        let spec = demo_spec();
        for (bot, key) in [
            ("b1", "demo-a-py3.13"),
            ("b2", "demo-a-py3.13"),
            ("b3", "demo-b-py3.13"),
        ] {
            let plugin_dir = tenants.join("u").join(bot).join("plugins/demo");
            std::fs::create_dir_all(&plugin_dir).unwrap();
            std::fs::write(
                plugin_dir.join(MARKER),
                serde_json::to_vec(&installer.marker(&spec, key)).unwrap(),
            )
            .unwrap();
        }
        for key in ["demo-a-py3.13", "demo-b-py3.13", "demo-old-py3.13"] {
            fake_shared(&installer, key);
        }
        std::fs::create_dir_all(installer.shared_dir.join("demo-partial-py3.13/src")).unwrap();
        assert_eq!(installer.gc_shared(&tenants), 2);
        assert!(installer.shared_dir.join("demo-a-py3.13").is_dir());
        assert!(installer.shared_dir.join("demo-b-py3.13").is_dir());

        std::fs::remove_dir_all(tenants.join("u/b3")).unwrap();
        std::fs::remove_dir_all(tenants.join("u/b1")).unwrap();
        assert_eq!(installer.gc_shared(&tenants), 1);
        assert!(installer.shared_dir.join("demo-a-py3.13").is_dir());
        assert!(!installer.shared_dir.join("demo-b-py3.13").exists());
        assert!(installer.shared_size(&tenants.join("u/b2/plugins/demo")) == 2);
    }

    #[test]
    fn python_version_reads_major_minor() {
        let v = python_version("python3");
        assert!(v.starts_with("3."), "{v}");
        assert_eq!(python_version("/nonexistent/python"), "");
    }

    /// Serves `respond(path)` on a local port and records every request path.
    async fn serve(respond: fn(&str) -> (u16, Vec<u8>)) -> (String, Hits) {
        use axum::{extract::Request, response::IntoResponse};
        let hits: Hits = Arc::default();
        let log = hits.clone();
        let app = axum::Router::new().fallback(move |req: Request| {
            let log = log.clone();
            async move {
                let path = req.uri().path().to_string();
                let signed = req.headers().contains_key(crate::sign::HEADER_SIG);
                log.lock()
                    .unwrap()
                    .push(format!("{}{path}", if signed { "signed:" } else { "" }));
                let (code, body) = respond(&path);
                (axum::http::StatusCode::from_u16(code).unwrap(), body).into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), hits)
    }

    fn proxy_client(base_url: &str) -> crate::sign::SignedClient {
        crate::sign::SignedClient {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            node_id: "node-1".into(),
            key: b"k".to_vec(),
        }
    }

    fn urls() -> Vec<String> {
        zip_urls(
            &Repo {
                owner: "a".into(),
                name: "b".into(),
                branch: None,
            },
            "",
        )
    }

    async fn run(proxy: Option<&str>, direct: &str) -> Result<Downloaded> {
        let http = reqwest::Client::new();
        let proxy = proxy.map(proxy_client);
        download_zip_with(&http, proxy.as_ref(), Some(direct), &urls(), Duration::from_millis(10)).await
    }

    #[tokio::test]
    async fn proxy_server_errors_retry_then_fall_back_to_direct() {
        let (proxy, proxy_hits) = serve(|_| (502, vec![])).await;
        let (direct, direct_hits) = serve(|_| (200, b"zip".to_vec())).await;
        let got = run(Some(&proxy), &direct).await.unwrap();
        assert_eq!((got.bytes.as_slice(), got.source), (&b"zip"[..], Source::Direct));
        assert_eq!(
            *proxy_hits.lock().unwrap(),
            vec!["signed:/api/internal/plugin-archive/a/b/zip/HEAD"; 2]
        );
        assert_eq!(*direct_hits.lock().unwrap(), vec!["/a/b/zip/HEAD"]);
    }

    #[tokio::test]
    async fn proxy_not_found_moves_to_next_ref_without_direct() {
        let (proxy, proxy_hits) = serve(|p| {
            if p.ends_with("/HEAD") {
                (404, vec![])
            } else {
                (200, b"main".to_vec())
            }
        })
        .await;
        let (direct, direct_hits) = serve(|_| (200, b"direct".to_vec())).await;
        let got = run(Some(&proxy), &direct).await.unwrap();
        assert_eq!((got.bytes.as_slice(), got.source), (&b"main"[..], Source::Proxy));
        assert_eq!(proxy_hits.lock().unwrap().len(), 2);
        assert!(direct_hits.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn proxy_auth_failure_falls_back_without_retry() {
        let (proxy, proxy_hits) = serve(|_| (401, vec![])).await;
        let (direct, _) = serve(|_| (200, b"zip".to_vec())).await;
        let got = run(Some(&proxy), &direct).await.unwrap();
        assert_eq!(got.source, Source::Direct);
        assert_eq!(proxy_hits.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn without_proxy_downloads_directly_and_reports_missing_repo() {
        let (direct, direct_hits) = serve(|_| (404, vec![])).await;
        let err = run(None, &direct).await.err().unwrap();
        assert!(err.to_string().contains("404"), "{err}");
        assert_eq!(direct_hits.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn oversized_archive_stops_immediately() {
        let (proxy, _) = serve(|_| (413, vec![])).await;
        let (direct, direct_hits) = serve(|_| (200, b"zip".to_vec())).await;
        let err = run(Some(&proxy), &direct).await.err().unwrap();
        assert!(err.to_string().contains("MB"), "{err}");
        assert!(direct_hits.lock().unwrap().is_empty());
    }

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
        assert_eq!(
            zip_urls(&r, "")[0],
            "https://codeload.github.com/a/b/zip/refs/heads/dev"
        );
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
