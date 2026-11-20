-- Commit each plugin is pinned to and the last upstream check per repository.
ALTER TABLE plugins ADD COLUMN commit_sha TEXT NOT NULL DEFAULT '';

CREATE TABLE plugin_repos (
  repo_url TEXT PRIMARY KEY,
  commit_sha TEXT NOT NULL DEFAULT '',
  checked_at INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_plugin_repos_checked ON plugin_repos(checked_at);
CREATE INDEX idx_plugins_repo ON plugins(repo_url);
