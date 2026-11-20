//! Cloudflare AI Gateway routing for LLM providers that the node cannot reach directly.

use crate::models::DesiredLlm;

const GATEWAY_PREFIX: &str = "https://gateway.ai.cloudflare.com/v1/";

/// Official base URL and the matching provider path under the gateway.
const ROUTES: &[(&str, &str)] = &[
    ("https://api.openai.com/v1", "openai"),
    ("https://api.anthropic.com/v1", "anthropic/v1"),
    ("https://generativelanguage.googleapis.com/v1beta/openai", "google-ai-studio/v1beta/openai"),
    ("https://api.x.ai/v1", "grok/v1"),
];

/// Returns the gateway provider path for an official base URL.
pub fn route(base_url: &str) -> Option<&'static str> {
    let base = base_url.trim_end_matches('/');
    ROUTES.iter().find(|(official, _)| *official == base).map(|(_, path)| *path)
}

#[derive(Debug, Clone, PartialEq)]
pub struct AiGateway {
    /// `https://gateway.ai.cloudflare.com/v1/{account_id}/{gateway_id}` without a trailing slash.
    pub base: String,
    /// Sent as `cf-aig-authorization: Bearer <token>`.
    pub token: String,
}

impl AiGateway {
    /// Accepts only `https://gateway.ai.cloudflare.com/v1/{account_id}/{gateway_id}`.
    pub fn new(url: &str, token: &str) -> Option<Self> {
        let base = url.trim().trim_end_matches('/');
        let rest = base.strip_prefix(GATEWAY_PREFIX)?;
        let mut parts = rest.split('/');
        let valid = |s: Option<&str>| s.is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        if !valid(parts.next()) || !valid(parts.next()) || parts.next().is_some() {
            return None;
        }
        let token = token.trim();
        if token.is_empty() {
            return None;
        }
        Some(AiGateway { base: base.to_string(), token: token.to_string() })
    }

    /// Gateway base URL that replaces `base_url`, if the provider is routed.
    pub fn endpoint(&self, base_url: &str) -> Option<String> {
        route(base_url).map(|path| format!("{}/{path}", self.base))
    }

    /// Points a routed LLM spec at the gateway. Other providers are left unchanged.
    pub fn apply(&self, llm: &mut DesiredLlm) {
        if let Some(endpoint) = self.endpoint(&llm.base_url) {
            llm.endpoint = endpoint;
            llm.gateway_token = self.token.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GW: &str = "https://gateway.ai.cloudflare.com/v1/acc123/fish";

    #[test]
    fn parses_gateway_url() {
        assert_eq!(AiGateway::new(&format!("{GW}/"), " t ").unwrap(), AiGateway { base: GW.into(), token: "t".into() });
        assert!(AiGateway::new(GW, "").is_none());
        assert!(AiGateway::new("https://gateway.ai.cloudflare.com/v1/acc123", "t").is_none());
        assert!(AiGateway::new("https://gateway.ai.cloudflare.com/v1/acc123/fish/openai", "t").is_none());
        assert!(AiGateway::new("https://example.com/v1/acc123/fish", "t").is_none());
        assert!(AiGateway::new("https://gateway.ai.cloudflare.com/v1/acc?x/fish", "t").is_none());
    }

    #[test]
    fn routes_only_blocked_providers() {
        let gw = AiGateway::new(GW, "t").unwrap();
        assert_eq!(gw.endpoint("https://api.openai.com/v1/").as_deref(), Some("https://gateway.ai.cloudflare.com/v1/acc123/fish/openai"));
        assert_eq!(gw.endpoint("https://api.anthropic.com/v1").as_deref(), Some("https://gateway.ai.cloudflare.com/v1/acc123/fish/anthropic/v1"));
        assert_eq!(
            gw.endpoint("https://generativelanguage.googleapis.com/v1beta/openai").as_deref(),
            Some("https://gateway.ai.cloudflare.com/v1/acc123/fish/google-ai-studio/v1beta/openai")
        );
        assert_eq!(gw.endpoint("https://api.x.ai/v1").as_deref(), Some("https://gateway.ai.cloudflare.com/v1/acc123/fish/grok/v1"));
        assert!(gw.endpoint("https://api.deepseek.com").is_none());
        assert!(gw.endpoint("https://api.openai.com").is_none());

        let mut llm = DesiredLlm { base_url: "https://api.x.ai/v1".into(), model: "grok-4.7".into(), api_key: "k".into(), endpoint: String::new(), gateway_token: String::new() };
        gw.apply(&mut llm);
        assert_eq!(llm.endpoint, "https://gateway.ai.cloudflare.com/v1/acc123/fish/grok/v1");
        assert_eq!(llm.gateway_token, "t");
        let mut direct = DesiredLlm { base_url: "https://api.deepseek.com".into(), model: "m".into(), api_key: "k".into(), endpoint: String::new(), gateway_token: String::new() };
        gw.apply(&mut direct);
        assert!(direct.endpoint.is_empty() && direct.gateway_token.is_empty());
    }
}
