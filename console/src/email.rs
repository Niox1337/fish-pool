//! Verification code emails through Cloudflare Email Service, Resend, or the log in dev mode.

use crate::app::App;
use crate::error::{ApiError, ApiResult};
use crate::http;
use crate::util::{log, log_error};
use serde_json::json;
use wasm_bindgen::{JsCast, JsValue};
use worker::{Method, SendEmail, SendEmailBuilder};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Provider {
    /// `send_email` binding named `EMAIL`.
    Cloudflare,
    /// Resend HTTPS API with secret `RESEND_API_KEY`.
    Resend,
    /// Writes the code to the Worker log, allowed only with `FISH_DEV=1`.
    Log,
}

pub fn parse_provider(s: Option<&str>) -> Provider {
    match s.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("cloudflare") => Provider::Cloudflare,
        Some("resend") => Provider::Resend,
        _ => Provider::Log,
    }
}

pub struct Mail {
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: String,
}

pub fn code_mail(to: &str, purpose: &str, code: &str, ttl_minutes: i64) -> Mail {
    let action = if purpose == "reset" { "重置密码" } else { "注册账号" };
    let subject = format!("大肥鱼托管班 {action}验证码：{code}");
    let text = format!(
        "你正在{action}，验证码是 {code}，{ttl_minutes} 分钟内有效。\n\n如果不是你本人操作，请忽略这封邮件，不要把验证码告诉任何人。\n\n大肥鱼托管班 fish.niox.lol"
    );
    let html = format!(
        "<div style=\"font-family:sans-serif;font-size:15px;line-height:1.6;color:#222\">\
<p>你正在{action}，验证码是：</p>\
<p style=\"font-size:28px;font-weight:bold;letter-spacing:6px\">{code}</p>\
<p>{ttl_minutes} 分钟内有效。如果不是你本人操作，请忽略这封邮件，不要把验证码告诉任何人。</p>\
<p style=\"color:#888\">大肥鱼托管班 fish.niox.lol</p></div>"
    );
    Mail { to: to.to_string(), subject, text, html }
}

fn send_failed() -> ApiError {
    ApiError::new(502, "email_send_failed", "验证码邮件发送失败，请稍后再试")
}

fn set(obj: &js_sys::Object, k: &str, v: &JsValue) {
    let _ = js_sys::Reflect::set(obj, &JsValue::from_str(k), v);
}

pub async fn send(app: &App, mail: &Mail, dev_code: &str) -> ApiResult<()> {
    let from = app.var("EMAIL_FROM").unwrap_or_else(|| "noreply@niox.lol".into());
    let from_name = app.var("EMAIL_FROM_NAME").unwrap_or_else(|| "大肥鱼托管班".into());
    match parse_provider(app.var("EMAIL_PROVIDER").as_deref()) {
        Provider::Log => {
            if !app.dev {
                log_error("EMAIL_PROVIDER=log needs FISH_DEV=1, refusing to put codes in production logs");
                return Err(send_failed());
            }
            log(&format!("[dev] email code for {}: {}", mail.to, dev_code));
            Ok(())
        }
        Provider::Cloudflare => {
            let binding: SendEmail = app.env.get_binding::<SendEmail>("EMAIL").map_err(|e| {
                log_error(&format!("send_email binding EMAIL missing: {e}"));
                send_failed()
            })?;
            let msg = js_sys::Object::new();
            let sender = js_sys::Object::new();
            set(&sender, "email", &from.as_str().into());
            set(&sender, "name", &from_name.as_str().into());
            set(&msg, "from", &sender);
            set(&msg, "to", &mail.to.as_str().into());
            set(&msg, "subject", &mail.subject.as_str().into());
            set(&msg, "text", &mail.text.as_str().into());
            set(&msg, "html", &mail.html.as_str().into());
            let builder: SendEmailBuilder = msg.unchecked_into();
            binding.send_with_builder(&builder).await.map_err(|e| {
                let code = js_sys::Reflect::get(&e, &"code".into()).ok().and_then(|c| c.as_string()).unwrap_or_default();
                log_error(&format!("cloudflare email send failed {code}: {:?}", e.message()));
                send_failed()
            })?;
            Ok(())
        }
        Provider::Resend => {
            let key = app.var("RESEND_API_KEY").filter(|k| !k.is_empty()).ok_or_else(|| {
                log_error("RESEND_API_KEY is not set");
                send_failed()
            })?;
            let body = json!({
                "from": format!("{from_name} <{from}>"),
                "to": [mail.to],
                "subject": mail.subject,
                "text": mail.text,
                "html": mail.html,
            });
            let bytes = serde_json::to_vec(&body)?;
            let auth = format!("Bearer {key}");
            let headers = [("authorization", auth.as_str()), ("content-type", "application/json"), ("user-agent", "fish-console")];
            let resp = http::send(Method::Post, "https://api.resend.com/emails", &headers, Some(&bytes), 10_000)
                .await
                .map_err(|e| {
                    log_error(&format!("resend: {e}"));
                    send_failed()
                })?;
            if !resp.ok() {
                log_error(&format!("resend {}: {}", resp.status, resp.text()));
                return Err(send_failed());
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_parsing() {
        assert_eq!(parse_provider(Some("Cloudflare")), Provider::Cloudflare);
        assert_eq!(parse_provider(Some("resend")), Provider::Resend);
        assert_eq!(parse_provider(Some("log")), Provider::Log);
        assert_eq!(parse_provider(None), Provider::Log);
    }

    #[test]
    fn mail_contains_code() {
        let m = code_mail("a@b.co", "reset", "123456", 10);
        assert!(m.subject.contains("123456"));
        assert!(m.text.contains("重置密码"));
        assert!(m.html.contains("123456"));
        assert!(code_mail("a@b.co", "register", "1", 10).text.contains("注册账号"));
    }
}
