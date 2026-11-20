//! Captcha, email codes, registration, login, logout, password reset.

use super::{client_ip, json_body, user_agent, Reply};
use crate::app::{clear_cookie, ok_json, App, SESSION_COOKIE, USER_COLS};
use crate::args;
use crate::captcha;
use crate::crypto::{self, sha256_hex};
use crate::email;
use crate::invite;
use crate::error::{ApiError, ApiResult};
use crate::models::{AccountQuota, UserRow, UNLIMITED_COUNT};
use crate::util::{log, new_id, normalize_email, now_ms, random_bytes, valid_email};
use serde::Deserialize;
use serde_json::json;
use worker::Request;

const CAPTCHA_TTL_MS: i64 = 5 * 60 * 1000;
const CODE_TTL_MS: i64 = 10 * 60 * 1000;
const CODE_COOLDOWN_MS: i64 = 60 * 1000;
const CODE_MAX_ATTEMPTS: i64 = 5;
const LOGIN_WINDOW_MS: i64 = 15 * 60 * 1000;
const LOGIN_FAILS_FOR_CAPTCHA: i64 = 5;

fn captcha_hash(id: &str, answer: &str) -> String {
    sha256_hex(format!("{id}:{}", captcha::normalize_answer(answer)).as_bytes())
}

fn code_hash(email: &str, purpose: &str, code: &str) -> String {
    sha256_hex(format!("{email}:{purpose}:{}", code.trim()).as_bytes())
}

pub async fn captcha(app: &App) -> ApiResult<Reply> {
    let mut rng = captcha::Rng::new(u64::from_le_bytes(random_bytes::<8>()));
    let answer = captcha::random_answer(&mut rng);
    let svg = captcha::render_svg(&answer, &mut rng);
    let id = new_id();
    app.db
        .run(
            "INSERT INTO captchas (id, answer_hash, expires_at) VALUES (?1, ?2, ?3)",
            args![id, captcha_hash(&id, &answer), now_ms() + CAPTCHA_TTL_MS],
        )
        .await?;
    if app.dev {
        log(&format!("[dev] captcha {id} answer {answer}"));
    }
    Ok(Reply::json(json!({"id": id, "svg": svg})))
}

/// Consumes a captcha. Every attempt deletes it, so each captcha is single use.
async fn check_captcha(app: &App, id: Option<&str>, answer: Option<&str>) -> ApiResult<()> {
    let invalid = || ApiError::new(400, "captcha_invalid", "图形验证码错误或已过期，请刷新后重试");
    let (Some(id), Some(answer)) = (id.filter(|s| !s.is_empty()), answer.filter(|s| !s.is_empty())) else {
        return Err(invalid());
    };
    #[derive(Deserialize)]
    struct Row {
        answer_hash: String,
        expires_at: i64,
    }
    let row: Option<Row> = app.db.first("DELETE FROM captchas WHERE id = ?1 RETURNING answer_hash, expires_at", args![id]).await?;
    match row {
        Some(r) if r.expires_at > now_ms() && crypto::ct_eq(r.answer_hash.as_bytes(), captcha_hash(id, answer).as_bytes()) => Ok(()),
        _ => Err(invalid()),
    }
}

async fn user_by_email(app: &App, email: &str) -> ApiResult<Option<UserRow>> {
    app.db.first(&format!("SELECT {USER_COLS} FROM users WHERE email = ?1"), args![email]).await
}

fn check_purpose(p: &str) -> ApiResult<()> {
    if p == "register" || p == "reset" {
        Ok(())
    } else {
        Err(ApiError::bad_request("purpose 只能是 register 或 reset"))
    }
}

fn check_email(email: &str) -> ApiResult<()> {
    if valid_email(email) {
        Ok(())
    } else {
        Err(ApiError::new(400, "email_invalid", "邮箱格式不正确"))
    }
}

fn check_password(pw: &str) -> ApiResult<()> {
    let n = pw.chars().count();
    if !(8..=128).contains(&n) {
        return Err(ApiError::new(400, "weak_password", "密码长度应为 8 到 128 个字符"));
    }
    if pw.chars().all(|c| c.is_ascii_digit()) || pw.chars().all(|c| c.is_alphabetic()) {
        return Err(ApiError::new(400, "weak_password", "密码需要同时包含字母和数字或符号"));
    }
    Ok(())
}

#[derive(Deserialize)]
struct EmailCodeReq {
    email: String,
    purpose: String,
    captcha_id: Option<String>,
    captcha_answer: Option<String>,
    invite_code: Option<String>,
}

pub async fn email_code(req: &mut Request, app: &App) -> ApiResult<Reply> {
    let body: EmailCodeReq = json_body(req).await?;
    let email = normalize_email(&body.email);
    check_email(&email)?;
    check_purpose(&body.purpose)?;
    check_captcha(app, body.captcha_id.as_deref(), body.captcha_answer.as_deref()).await?;
    if body.purpose == "register" {
        require_invite(app, &email, body.invite_code.as_deref()).await?;
    }

    let now = now_ms();
    let last: Option<i64> = app
        .db
        .first::<serde_json::Value>(
            "SELECT created_at FROM email_codes WHERE email = ?1 AND purpose = ?2 ORDER BY created_at DESC LIMIT 1",
            args![email, body.purpose],
        )
        .await?
        .and_then(|v| v.get("created_at").and_then(|c| c.as_i64()));
    if let Some(t) = last {
        if now - t < CODE_COOLDOWN_MS {
            let wait = (CODE_COOLDOWN_MS - (now - t)) / 1000 + 1;
            return Err(ApiError::rate_limited(format!("发送太频繁，请 {wait} 秒后再试")));
        }
    }
    let ip_bucket = format!("email_code_ip:{}", client_ip(req));
    let mail_bucket = format!("email_code_mail:{email}");
    if app.rate_count(&ip_bucket, 3600 * 1000).await? >= 10 || app.rate_count(&mail_bucket, 24 * 3600 * 1000).await? >= 10 {
        return Err(ApiError::rate_limited("验证码请求次数过多，请稍后再试"));
    }

    let exists = user_by_email(app, &email).await?.is_some();
    if body.purpose == "register" && exists {
        return Err(ApiError::conflict("email_taken", "该邮箱已注册，请直接登录"));
    }
    app.rate_hit(&ip_bucket).await?;
    app.rate_hit(&mail_bucket).await?;
    if body.purpose == "reset" && !exists {
        // The response does not reveal whether the account exists.
        return Ok(Reply::json(json!({"ok": true, "cooldown_s": CODE_COOLDOWN_MS / 1000})));
    }

    let code = format!("{:06}", u32::from_le_bytes(random_bytes::<4>()) % 1_000_000);
    app.db
        .batch(vec![
            app.db.stmt("DELETE FROM email_codes WHERE email = ?1 AND purpose = ?2", args![email, body.purpose])?,
            app.db.stmt(
                "INSERT INTO email_codes (email, purpose, code_hash, expires_at, attempts, created_at) VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                args![email, body.purpose, code_hash(&email, &body.purpose, &code), now + CODE_TTL_MS, now],
            )?,
        ])
        .await?;
    let mail = email::code_mail(&email, &body.purpose, &code, CODE_TTL_MS / 60_000);
    if let Err(e) = email::send(app, &mail, &code).await {
        app.db.run("DELETE FROM email_codes WHERE email = ?1 AND purpose = ?2", args![email, body.purpose]).await?;
        return Err(e);
    }
    Ok(Reply::json(json!({"ok": true, "cooldown_s": CODE_COOLDOWN_MS / 1000})))
}

/// Checks the newest code for (email, purpose) and consumes it on success.
async fn verify_code(app: &App, email: &str, purpose: &str, code: &str) -> ApiResult<()> {
    #[derive(Deserialize)]
    struct Row {
        rid: i64,
        code_hash: String,
        expires_at: i64,
        attempts: i64,
    }
    let row: Option<Row> = app
        .db
        .first(
            "SELECT rowid AS rid, code_hash, expires_at, attempts FROM email_codes WHERE email = ?1 AND purpose = ?2 ORDER BY created_at DESC LIMIT 1",
            args![email, purpose],
        )
        .await?;
    let Some(row) = row else { return Err(ApiError::new(400, "code_invalid", "邮箱验证码错误，请重新获取")) };
    if row.expires_at <= now_ms() {
        return Err(ApiError::new(400, "code_expired", "邮箱验证码已过期，请重新获取"));
    }
    if row.attempts >= CODE_MAX_ATTEMPTS {
        return Err(ApiError::new(400, "code_invalid", "验证码错误次数过多，请重新获取"));
    }
    if !crypto::ct_eq(row.code_hash.as_bytes(), code_hash(email, purpose, code).as_bytes()) {
        app.db.run("UPDATE email_codes SET attempts = attempts + 1 WHERE rowid = ?1", args![row.rid]).await?;
        return Err(ApiError::new(400, "code_invalid", "邮箱验证码错误"));
    }
    app.db.run("DELETE FROM email_codes WHERE email = ?1 AND purpose = ?2", args![email, purpose]).await?;
    Ok(())
}

#[derive(Deserialize)]
struct RegisterReq {
    email: String,
    password: String,
    code: String,
    invite_code: Option<String>,
    #[serde(default)]
    agree: bool,
}

/// Checks the invite for a registration. Addresses in `ADMIN_EMAILS` may register without one.
/// Returns the canonical code to redeem, if any.
async fn require_invite(app: &App, email: &str, input: Option<&str>) -> ApiResult<Option<String>> {
    match input.map(str::trim).filter(|s| !s.is_empty()) {
        Some(code) => Ok(Some(invite::check(app, code).await?.code)),
        None if is_admin_email(app, email) => Ok(None),
        None => Err(ApiError::new(400, "invite_invalid", "注册需要邀请码")),
    }
}

fn is_admin_email(app: &App, email: &str) -> bool {
    app.var("ADMIN_EMAILS").map(|list| email_in_list(&list, email)).unwrap_or(false)
}

/// Whether `email` (already normalized) appears in a comma separated list.
pub fn email_in_list(list: &str, email: &str) -> bool {
    !email.is_empty() && list.split(',').any(|e| normalize_email(e) == email)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_list_matching() {
        assert!(email_in_list("a@b.co, Admin@Example.com", "admin@example.com"));
        assert!(!email_in_list("a@b.co", "c@b.co"));
        assert!(!email_in_list("", ""));
        assert!(!email_in_list("a@b.co,,", ""));
    }
}

pub async fn register(req: &mut Request, app: &App) -> ApiResult<Reply> {
    let body: RegisterReq = json_body(req).await?;
    let email = normalize_email(&body.email);
    check_email(&email)?;
    if !body.agree {
        return Err(ApiError::new(400, "terms_not_accepted", "请先阅读并同意用户协议"));
    }
    check_password(&body.password)?;
    if user_by_email(app, &email).await?.is_some() {
        return Err(ApiError::conflict("email_taken", "该邮箱已注册，请直接登录"));
    }
    let invite_code = require_invite(app, &email, body.invite_code.as_deref()).await?;
    verify_code(app, &email, "register", &body.code).await?;
    let hash = crypto::hash_password(&body.password).await.map_err(ApiError::internal)?;
    let id = new_id();
    let role = if is_admin_email(app, &email) { "admin" } else { "user" };
    let now = now_ms();
    let quota = AccountQuota::default();
    let bots = quota.bots.unwrap_or(UNLIMITED_COUNT);
    let plugins = quota.plugins.unwrap_or(UNLIMITED_COUNT);
    let taken = |e: ApiError| if e.code == "conflict" { ApiError::conflict("email_taken", "该邮箱已注册，请直接登录") } else { e };
    match invite_code {
        Some(code) => {
            // The user row is inserted only when the conditional claim changed one row,
            // and the batch is one transaction, so a failed insert also undoes the claim.
            let changes = app
                .db
                .batch(vec![
                    invite::claim_stmt(app, &code, &id, now)?,
                    app.db.stmt(
                        "INSERT INTO users (id, email, password_hash, role, created_at, last_login_at, service_until, \
                         quota_memory_mb, quota_storage_mb, quota_bots, quota_plugins) \
                         SELECT ?1, ?2, ?3, ?4, ?5, ?5, ?5 + days * ?7, \
                         COALESCE(json_extract(quota_json, '$.memory_mb'), ?8), COALESCE(json_extract(quota_json, '$.storage_mb'), ?9), \
                         CASE WHEN quota_json IS NULL THEN ?10 ELSE COALESCE(json_extract(quota_json, '$.bots'), ?12) END, \
                         CASE WHEN quota_json IS NULL THEN ?11 ELSE COALESCE(json_extract(quota_json, '$.plugins'), ?12) END \
                         FROM invite_codes WHERE code = ?6 AND changes() = 1",
                        args![id, email, hash, role, now, code, invite::DAY_MS, quota.memory_mb, quota.storage_mb, bots, plugins, UNLIMITED_COUNT],
                    )?,
                    invite::record_stmt(app, &code, &id, now)?,
                ])
                .await
                .map_err(taken)?;
            if changes.get(1).copied().unwrap_or(0) != 1 {
                return Err(invite::failure(app, &code, &id).await);
            }
        }
        None => {
            app.db
                .run(
                    "INSERT INTO users (id, email, password_hash, role, created_at, last_login_at, quota_memory_mb, quota_storage_mb, quota_bots, quota_plugins) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8, ?9)",
                    args![id, email, hash, role, now, quota.memory_mb, quota.storage_mb, bots, plugins],
                )
                .await
                .map_err(taken)?;
        }
    }
    let user = app.user_by_id(&id).await?;
    let cookie = app.create_session(&user.id, &user_agent(req)).await?;
    Ok(Reply::json(json!({"user": user.view()})).with_cookie(cookie))
}

#[derive(Deserialize)]
struct LoginReq {
    email: String,
    password: String,
    captcha_id: Option<String>,
    captcha_answer: Option<String>,
}

pub async fn login(req: &mut Request, app: &App) -> ApiResult<Reply> {
    let body: LoginReq = json_body(req).await?;
    let email = normalize_email(&body.email);
    let ip_bucket = format!("login_ip:{}", client_ip(req));
    if app.rate_count(&ip_bucket, LOGIN_WINDOW_MS).await? >= 60 {
        return Err(ApiError::rate_limited("登录尝试过于频繁，请 15 分钟后再试"));
    }
    app.rate_hit(&ip_bucket).await?;

    let fail_bucket = format!("login_fail:{email}");
    let fails = app.rate_count(&fail_bucket, LOGIN_WINDOW_MS).await?;
    if fails >= LOGIN_FAILS_FOR_CAPTCHA {
        if body.captcha_id.as_deref().unwrap_or("").is_empty() {
            return Err(ApiError::new(400, "captcha_required", "登录失败次数较多，请输入图形验证码"));
        }
        check_captcha(app, body.captcha_id.as_deref(), body.captcha_answer.as_deref()).await?;
    }

    let user = user_by_email(app, &email).await?;
    let ok = match &user {
        Some(u) => crypto::verify_password(&body.password, &u.password_hash).await.map_err(ApiError::internal)?,
        None => false,
    };
    let Some(user) = user.filter(|_| ok) else {
        app.rate_hit(&fail_bucket).await?;
        if fails + 1 >= LOGIN_FAILS_FOR_CAPTCHA {
            return Err(ApiError::new(400, "captcha_required", "邮箱或密码错误，请输入图形验证码后重试"));
        }
        return Err(ApiError::new(401, "invalid_credentials", "邮箱或密码错误"));
    };
    let role = if is_admin_email(app, &email) { "admin" } else { user.role.as_str() };
    app.db
        .batch(vec![
            app.db.stmt("DELETE FROM rate_events WHERE bucket = ?1", args![fail_bucket])?,
            app.db.stmt("UPDATE users SET role = ?2, last_login_at = ?3 WHERE id = ?1", args![user.id, role, now_ms()])?,
        ])
        .await?;
    let user = app.user_by_id(&user.id).await?;
    let cookie = app.create_session(&user.id, &user_agent(req)).await?;
    Ok(Reply::json(json!({"user": user.view()})).with_cookie(cookie))
}

pub async fn logout(req: &Request, app: &App) -> ApiResult<Reply> {
    let header = req.headers().get("cookie").ok().flatten().unwrap_or_default();
    if let Some(token) = crate::app::cookie_value(&header, SESSION_COOKIE) {
        app.db.run("DELETE FROM sessions WHERE token_hash = ?1", args![sha256_hex(token.as_bytes())]).await?;
    }
    Ok(Reply::json(ok_json()).with_cookie(clear_cookie()))
}

#[derive(Deserialize)]
struct ResetReq {
    email: String,
    code: String,
    password: String,
}

pub async fn reset(req: &mut Request, app: &App) -> ApiResult<Reply> {
    let body: ResetReq = json_body(req).await?;
    let email = normalize_email(&body.email);
    check_email(&email)?;
    check_password(&body.password)?;
    verify_code(app, &email, "reset", &body.code).await?;
    let user = user_by_email(app, &email).await?.ok_or_else(|| ApiError::new(400, "code_invalid", "邮箱验证码错误"))?;
    let hash = crypto::hash_password(&body.password).await.map_err(ApiError::internal)?;
    app.db
        .batch(vec![
            app.db.stmt("UPDATE users SET password_hash = ?2 WHERE id = ?1", args![user.id, hash])?,
            app.db.stmt("DELETE FROM sessions WHERE user_id = ?1", args![user.id])?,
            app.db.stmt("DELETE FROM rate_events WHERE bucket = ?1", args![format!("login_fail:{email}")])?,
        ])
        .await?;
    Ok(Reply::json(ok_json()).with_cookie(clear_cookie()))
}

#[derive(Deserialize)]
struct RedeemReq {
    code: String,
}

pub async fn redeem(req: &mut Request, app: &App) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    let body: RedeemReq = json_body(req).await?;
    let bucket = format!("redeem:{}", user.id);
    if app.rate_count(&bucket, 3600 * 1000).await? >= 20 {
        return Err(ApiError::rate_limited("兑换尝试过于频繁，请稍后再试"));
    }
    app.rate_hit(&bucket).await?;
    let row = invite::check(app, &body.code).await?;
    invite::redeem_for_user(app, &row.code, &user.id).await?;
    let user = app.user_by_id(&user.id).await?;
    Ok(Reply::json(json!({"user": user.view()})))
}

pub async fn me(req: &Request, app: &App) -> ApiResult<Reply> {
    let user = app.current_user(req).await?;
    Ok(Reply::json(json!({"user": user.view()})))
}
