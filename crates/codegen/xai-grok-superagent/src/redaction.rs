use std::env;

const SECRET_ENV_NAMES: &[&str] = &[
    "SUPERAGENT_API_TOKEN",
    "SUPERAGENT_APP_INBOX_SECRET",
    "SUPERAGENT_GATEWAY_TOKEN",
    "XIBECODE_APP_INBOX_SECRET",
    "XIBECODE_GATEWAY_TOKEN",
    "POLLINATIONS_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "ANTHROPIC_API_KEY",
    "DEEPSEEK_API_KEY",
    "GOOGLE_API_KEY",
    "GROQ_API_KEY",
    "ZAI_API_KEY",
    "KIMI_API_KEY",
    "DASHSCOPE_API_KEY",
    "ROUTINGRUN_API_KEY",
    "ZENLLM_API_KEY",
    "NOUS_API_KEY",
    "TELEGRAM_BOT_TOKEN",
    "DISCORD_BOT_TOKEN",
    "SLACK_BOT_TOKEN",
    "SLACK_APP_TOKEN",
    "WHATSAPP_ACCESS_TOKEN",
    "SIGNAL_ACCOUNT",
    "MATRIX_ACCESS_TOKEN",
    "EMAIL_SMTP_PASSWORD",
    "EMAIL_IMAP_PASSWORD",
];

fn mask(value: &str) -> String {
    if value.len() <= 8 {
        return "[redacted]".to_owned();
    }
    format!("{}…{}", &value[..4], &value[value.len() - 4..])
}

fn replace_known_values(mut output: String) -> String {
    for name in SECRET_ENV_NAMES {
        if let Ok(value) = env::var(name) {
            let value = value.trim();
            if value.len() >= 8 {
                output = output.replace(value, &mask(value));
            }
        }
    }
    output
}

fn redact_key_value_tokens(input: &str) -> String {
    input
        .split_whitespace()
        .map(|token| {
            let Some((key, value)) = token.split_once('=') else {
                return token.to_owned();
            };
            let key_lower = key.to_ascii_lowercase();
            let sensitive = key_lower.contains("token")
                || key_lower.contains("secret")
                || key_lower.contains("password")
                || key_lower.contains("api_key")
                || key_lower.contains("apikey")
                || key_lower.ends_with("_key");
            if sensitive && value.len() >= 6 {
                format!("{key}=[redacted]")
            } else {
                token.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_prefixed_tokens(input: &str) -> String {
    input
        .split_whitespace()
        .map(|token| {
            let clean = token
                .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | ',' | ';' | ')' | ']'));
            let lower = clean.to_ascii_lowercase();
            let sensitive = lower.starts_with("sk-")
                || lower.starts_with("rk_")
                || lower.starts_with("xoxb-")
                || lower.starts_with("xapp-")
                || lower.starts_with("ghp_")
                || lower.starts_with("github_pat_")
                || (clean.chars().take(12).filter(|ch| *ch == ':').count() == 1
                    && clean.len() > 28);
            if sensitive && clean.len() >= 12 {
                token.replace(clean, "[redacted]")
            } else {
                token.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn redact_secrets(input: &str) -> String {
    let output = replace_known_values(input.to_owned());
    let output = redact_key_value_tokens(&output);
    redact_prefixed_tokens(&output)
}

#[cfg(test)]
mod tests {
    use super::redact_secrets;

    #[test]
    fn redacts_common_token_shapes_and_key_values() {
        let text = "OPENAI_API_KEY=super-secret-value sk-abcdefghijklmnopqrstuvwxyz ghp_abcdefghijklmnopqrstuvwxyz";
        let redacted = redact_secrets(text);
        assert!(redacted.contains("OPENAI_API_KEY=[redacted]"));
        assert!(!redacted.contains("super-secret-value"));
        assert!(!redacted.contains("sk-abcdefghijklmnopqrstuvwxyz"));
        assert!(!redacted.contains("ghp_abcdefghijklmnopqrstuvwxyz"));
    }
}
