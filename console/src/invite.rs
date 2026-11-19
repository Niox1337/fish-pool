//! Invite codes: generation, input normalization, status checks and redemption.

use crate::app::App;
use crate::args;
use crate::error::{ApiError, ApiResult};
use crate::util::{now_ms, random_bytes};
use serde::Deserialize;
use worker::D1PreparedStatement;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const PREFIX: &str = "FISH";
pub const DAY_MS: i64 = 24 * 3600 * 1000;

/// Formats 8 random symbols as `FISH-XXXX-XXXX`.
pub fn format_code(bytes: &[u8; 8]) -> String {
    let sym: String = bytes.iter().map(|b| ALPHABET[(b % 32) as usize] as char).collect();
    format!("{PREFIX}-{}-{}", &sym[..4], &sym[4..])
}

/// A new code from the CSPRNG. 256 is a multiple of 32, so `b % 32` is uniform.
pub fn generate() -> String {
    format_code(&random_bytes::<8>())
}

/// Canonical `FISH-XXXX-XXXX` form of user input, or None when it cannot be a code.
/// Case, dashes and spaces are ignored, O reads as 0 and I or L read as 1.
pub fn normalize(input: &str) -> Option<String> {
    let compact: String = input.chars().filter(|c| *c != '-' && !c.is_whitespace()).collect::<String>().to_ascii_uppercase();
    let body = match compact.len() {
        12 => compact.strip_prefix(PREFIX).or_else(|| compact.strip_prefix("F1SH"))?,
        _ => compact.as_str(),
    };
    let mapped: String = body
        .chars()
        .map(|c| match c {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect();
    if mapped.len() != 8 || !mapped.bytes().all(|b| ALPHABET.contains(&b)) {
        return None;
    }
    Some(format!("{PREFIX}-{}-{}", &mapped[..4], &mapped[4..]))
}

#[derive(Debug, Clone, Deserialize)]
pub struct InviteRow {
    pub code: String,
    pub days: i64,
    pub max_uses: i64,
    pub used_count: i64,
    pub note: String,
    pub created_by: Option<String>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

pub const INVITE_COLS: &str = "code, days, max_uses, used_count, note, created_by, created_at, expires_at, revoked_at";

pub fn invalid() -> ApiError {
    ApiError::new(400, "invite_invalid", "邀请码无效")
}

/// The error a redemption of `row` would fail with at `now`, if any.
pub fn status_error(row: &InviteRow, now: i64) -> Option<ApiError> {
    if row.revoked_at.is_some() {
        return Some(invalid());
    }
    if row.expires_at.is_some_and(|t| t <= now) {
        return Some(ApiError::new(400, "invite_expired", "邀请码已过期"));
    }
    if row.used_count >= row.max_uses {
        return Some(ApiError::new(400, "invite_used", "邀请码已被使用"));
    }
    None
}

pub async fn find(app: &App, code: &str) -> ApiResult<Option<InviteRow>> {
    app.db.first(&format!("SELECT {INVITE_COLS} FROM invite_codes WHERE code = ?1"), args![code]).await
}

/// Normalizes `input` and returns the invite when it can be redeemed now.
pub async fn check(app: &App, input: &str) -> ApiResult<InviteRow> {
    let code = normalize(input).ok_or_else(invalid)?;
    let row = find(app, &code).await?.ok_or_else(invalid)?;
    match status_error(&row, now_ms()) {
        Some(e) => Err(e),
        None => Ok(row),
    }
}

/// Explains why a conditional redemption changed nothing.
pub async fn failure(app: &App, code: &str, user_id: &str) -> ApiError {
    let row = match find(app, code).await {
        Ok(Some(r)) => r,
        Ok(None) => return invalid(),
        Err(e) => return e,
    };
    if let Some(e) = status_error(&row, now_ms()) {
        return e;
    }
    let again = app
        .db
        .count("SELECT COUNT(*) AS n FROM invite_redemptions WHERE code = ?1 AND user_id = ?2", args![code, user_id])
        .await;
    if matches!(again, Ok(n) if n > 0) {
        return ApiError::new(400, "invite_used", "你已经使用过这个邀请码");
    }
    ApiError::new(409, "conflict", "邀请码状态已变化，请重试")
}

/// The conditional use count increment. It changes one row only when the code can be
/// redeemed by `user_id` at `now`. Statements after it in the same batch read `changes()`.
pub fn claim_stmt(app: &App, code: &str, user_id: &str, now: i64) -> ApiResult<D1PreparedStatement> {
    app.db.stmt(
        "UPDATE invite_codes SET used_count = used_count + 1 \
         WHERE code = ?1 AND revoked_at IS NULL AND used_count < max_uses \
         AND (expires_at IS NULL OR expires_at > ?3) \
         AND NOT EXISTS (SELECT 1 FROM invite_redemptions WHERE code = ?1 AND user_id = ?2)",
        args![code, user_id, now],
    )
}

/// Records the redemption when the previous statement changed exactly one row.
pub fn record_stmt(app: &App, code: &str, user_id: &str, now: i64) -> ApiResult<D1PreparedStatement> {
    app.db.stmt(
        "INSERT INTO invite_redemptions (code, user_id, days, redeemed_at) \
         SELECT code, ?2, days, ?3 FROM invite_codes WHERE code = ?1 AND changes() = 1",
        args![code, user_id, now],
    )
}

/// Adds the days of an invite to an existing user in one transaction.
pub async fn redeem_for_user(app: &App, code: &str, user_id: &str) -> ApiResult<()> {
    let now = now_ms();
    let changes = app
        .db
        .batch(vec![
            claim_stmt(app, code, user_id, now)?,
            app.db.stmt(
                "UPDATE users SET service_until = MAX(?2, COALESCE(service_until, 0)) + \
                 (SELECT days FROM invite_codes WHERE code = ?3) * ?4 WHERE id = ?1 AND changes() = 1",
                args![user_id, now, code, DAY_MS],
            )?,
            record_stmt(app, code, user_id, now)?,
        ])
        .await?;
    if changes.first().copied().unwrap_or(0) != 1 || changes.get(1).copied().unwrap_or(0) != 1 {
        return Err(failure(app, code, user_id).await);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> InviteRow {
        InviteRow {
            code: "FISH-0000-0000".into(),
            days: 30,
            max_uses: 1,
            used_count: 0,
            note: String::new(),
            created_by: None,
            created_at: 1,
            expires_at: None,
            revoked_at: None,
        }
    }

    #[test]
    fn generated_codes_are_canonical() {
        for _ in 0..200 {
            let c = generate();
            assert_eq!(c.len(), 14);
            assert!(c.starts_with("FISH-"));
            assert_eq!(normalize(&c).as_deref(), Some(c.as_str()));
        }
        assert_eq!(format_code(&[0, 31, 32, 63, 10, 20, 30, 255]), "FISH-0Z0Z-AMYZ");
    }

    #[test]
    fn normalization() {
        assert_eq!(normalize("fish-abcd-efgh").as_deref(), Some("FISH-ABCD-EFGH"));
        assert_eq!(normalize(" FISH ABCD EFGH ").as_deref(), Some("FISH-ABCD-EFGH"));
        assert_eq!(normalize("fishabcdefgh").as_deref(), Some("FISH-ABCD-EFGH"));
        assert_eq!(normalize("abcd-efgh").as_deref(), Some("FISH-ABCD-EFGH"));
        assert_eq!(normalize("f1sh-abcd").as_deref(), Some("FISH-F1SH-ABCD"));
        assert_eq!(normalize("ABCD-ABCD-EFGH"), None);
        assert_eq!(normalize("F1SH-oIl0-0000").as_deref(), Some("FISH-0110-0000"));
        assert_eq!(normalize("FISH-ABCD-EFGU"), None);
        assert_eq!(normalize("FISH-ABCD-EFG"), None);
        assert_eq!(normalize("FISH-ABCD-EFGHJ"), None);
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("FISH-ABCD-EF!H"), None);
    }

    #[test]
    fn status_errors() {
        assert!(status_error(&row(), 10).is_none());
        let mut r = row();
        r.used_count = 1;
        assert_eq!(status_error(&r, 10).unwrap().code, "invite_used");
        let mut r = row();
        r.expires_at = Some(10);
        assert_eq!(status_error(&r, 10).unwrap().code, "invite_expired");
        assert!(status_error(&r, 9).is_none());
        let mut r = row();
        r.revoked_at = Some(5);
        r.used_count = 1;
        assert_eq!(status_error(&r, 10).unwrap().code, "invite_invalid");
    }
}
