use std::{collections::HashSet, env, io, sync::Arc};

use rustls::{ClientConfig, RootCertStore};
use rustls_native_certs::load_native_certs;
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    net::TcpStream,
};
use tokio_rustls::{TlsConnector, client::TlsStream};

#[derive(Debug, Clone)]
pub struct EmailConfig {
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_tls: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_tls: bool,
    pub username: String,
    pub password: String,
    pub from: String,
    pub allowed_senders: HashSet<String>,
    pub poll_seconds: u64,
}

impl EmailConfig {
    pub fn from_env() -> Result<Self, EmailError> {
        let username = required_env("EMAIL_USERNAME")?;
        let password = required_env("EMAIL_PASSWORD")?;
        let from = env::var("EMAIL_FROM").unwrap_or_else(|_| username.clone());
        let allowed_senders: HashSet<String> = env::var("EMAIL_ALLOWED_SENDERS")
            .unwrap_or_default()
            .split(',')
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect();
        if allowed_senders.is_empty() {
            return Err(EmailError::Configuration(
                "EMAIL_ALLOWED_SENDERS must contain at least one address".to_owned(),
            ));
        }
        Ok(Self {
            imap_host: env::var("EMAIL_IMAP_HOST").unwrap_or_else(|_| "imap.gmail.com".to_owned()),
            imap_port: env::var("EMAIL_IMAP_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(993),
            imap_tls: parse_bool("EMAIL_IMAP_TLS", true),
            smtp_host: env::var("EMAIL_SMTP_HOST").unwrap_or_else(|_| "smtp.gmail.com".to_owned()),
            smtp_port: env::var("EMAIL_SMTP_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(465),
            smtp_tls: parse_bool("EMAIL_SMTP_TLS", true),
            username,
            password,
            from,
            allowed_senders,
            poll_seconds: env::var("EMAIL_POLL_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(30)
                .max(5),
        })
    }

    pub fn is_allowed_sender(&self, sender: &str) -> bool {
        let normalized = normalize_address(sender);
        self.allowed_senders
            .iter()
            .any(|allowed| normalized == *allowed || normalized.ends_with(&format!("@{allowed}")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailMessage {
    pub uid: u64,
    pub message_id: Option<String>,
    pub sender: String,
    pub recipient: Option<String>,
    pub subject: String,
    pub text: String,
}

impl EmailMessage {
    pub fn session_key(&self) -> String {
        let normalized_sender = normalize_address(&self.sender);
        let sender = urlencoding::encode(&normalized_sender);
        let subject = urlencoding::encode(self.subject.trim());
        format!("agent:main:email:{sender}:{subject}")
    }
}

#[derive(Debug, Error)]
pub enum EmailError {
    #[error("email configuration error: {0}")]
    Configuration(String),
    #[error("email I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("email TLS error: {0}")]
    Tls(String),
    #[error("email protocol error: {0}")]
    Protocol(String),
}

#[derive(Clone, Default)]
pub struct EmailClient;

impl EmailClient {
    pub fn new() -> Self {
        Self
    }

    pub async fn fetch_unseen(
        &self,
        config: &EmailConfig,
    ) -> Result<Vec<EmailMessage>, EmailError> {
        let tcp = TcpStream::connect((config.imap_host.as_str(), config.imap_port)).await?;
        if config.imap_tls {
            let tls = tls_connector(&config.imap_host)?
                .connect(server_name(&config.imap_host)?, tcp)
                .await
                .map_err(|error| EmailError::Tls(error.to_string()))?;
            fetch_unseen_session(tls, config).await
        } else {
            fetch_unseen_session(tcp, config).await
        }
    }

    pub async fn send_text(
        &self,
        config: &EmailConfig,
        recipient: &str,
        subject: &str,
        body: &str,
    ) -> Result<(), EmailError> {
        let tcp = TcpStream::connect((config.smtp_host.as_str(), config.smtp_port)).await?;
        if config.smtp_tls {
            let tls = tls_connector(&config.smtp_host)?
                .connect(server_name(&config.smtp_host)?, tcp)
                .await
                .map_err(|error| EmailError::Tls(error.to_string()))?;
            smtp_send_session(tls, config, recipient, subject, body).await
        } else {
            smtp_send_session(tcp, config, recipient, subject, body).await
        }
    }
}

async fn fetch_unseen_session<S>(
    stream: S,
    config: &EmailConfig,
) -> Result<Vec<EmailMessage>, EmailError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut reader = BufReader::new(stream);
    expect_imap_greeting(&mut reader).await?;
    imap_command(
        &mut reader,
        "A001",
        &format!(
            "LOGIN {} {}",
            quote_atom(&config.username),
            quote_atom(&config.password)
        ),
    )
    .await?;
    imap_command(&mut reader, "A002", "SELECT INBOX").await?;
    let search = imap_command(&mut reader, "A003", "UID SEARCH UNSEEN").await?;
    let uids = search
        .lines()
        .find(|line| line.starts_with("* SEARCH"))
        .map(|line| {
            line.split_whitespace()
                .skip(2)
                .filter_map(|value| value.parse().ok())
                .collect::<Vec<u64>>()
        })
        .unwrap_or_default();
    let mut messages = Vec::new();
    for (index, uid) in uids.into_iter().enumerate() {
        let tag = format!("A{:03}", index + 4);
        let response = imap_command(
            &mut reader,
            &tag,
            &format!("UID FETCH {uid} (RFC822 BODY.PEEK[HEADER] BODY.PEEK[TEXT])"),
        )
        .await?;
        if let Some(raw) = extract_rfc822_literal(&response)
            && let Some(mut message) = parse_rfc822(uid, &raw)
            && config.is_allowed_sender(&message.sender)
        {
            message.uid = uid;
            messages.push(message);
        }
        let store_tag = format!("A{:03}", index + 104);
        let _ = imap_command(
            &mut reader,
            &store_tag,
            &format!("UID STORE {uid} +FLAGS (\\Seen)"),
        )
        .await?;
    }
    let _ = imap_command(&mut reader, "A999", "LOGOUT").await?;
    Ok(messages)
}

async fn smtp_send_session<S>(
    stream: S,
    config: &EmailConfig,
    recipient: &str,
    subject: &str,
    body: &str,
) -> Result<(), EmailError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut reader = BufReader::new(stream);
    expect_smtp_code(&mut reader, 220).await?;
    smtp_command(&mut reader, "EHLO superagent", 250).await?;
    smtp_command(
        &mut reader,
        &format!(
            "AUTH PLAIN {}",
            base64_auth(&config.username, &config.password)
        ),
        235,
    )
    .await?;
    smtp_command(
        &mut reader,
        &format!("MAIL FROM:<{}>", clean_address(&config.from)),
        250,
    )
    .await?;
    smtp_command(
        &mut reader,
        &format!("RCPT TO:<{}>", clean_address(recipient)),
        250,
    )
    .await?;
    smtp_command(&mut reader, "DATA", 354).await?;
    let message = format!(
        "From: {}\r\nTo: {}\r\nSubject: {}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{}\r\n.",
        clean_address(&config.from),
        clean_address(recipient),
        subject.replace(['\r', '\n'], " "),
        body.replace("\n.", "\n.."),
    );
    write_command(&mut reader, &message).await?;
    expect_smtp_code(&mut reader, 250).await?;
    smtp_command(&mut reader, "QUIT", 221).await?;
    Ok(())
}

async fn expect_imap_greeting<R>(reader: &mut BufReader<R>) -> Result<(), EmailError>
where
    R: AsyncRead + Unpin,
{
    let line = read_line(reader).await?;
    if line.starts_with("* OK") || line.starts_with("* PREAUTH") {
        Ok(())
    } else {
        Err(EmailError::Protocol(format!(
            "unexpected IMAP greeting: {line}"
        )))
    }
}

async fn imap_command<R>(
    reader: &mut BufReader<R>,
    tag: &str,
    command: &str,
) -> Result<String, EmailError>
where
    R: AsyncRead + AsyncWrite + Unpin,
{
    write_command(reader, &format!("{tag} {command}")).await?;
    let mut response = String::new();
    loop {
        let line = read_line(reader).await?;
        response.push_str(&line);
        response.push('\n');
        if line.starts_with(tag) {
            if !line.contains(" OK") {
                return Err(EmailError::Protocol(line));
            }
            return Ok(response);
        }
    }
}

async fn smtp_command<R>(
    reader: &mut BufReader<R>,
    command: &str,
    expected: u16,
) -> Result<(), EmailError>
where
    R: AsyncRead + AsyncWrite + Unpin,
{
    write_command(reader, command).await?;
    expect_smtp_code(reader, expected).await
}

async fn expect_smtp_code<R>(reader: &mut BufReader<R>, expected: u16) -> Result<(), EmailError>
where
    R: AsyncRead + Unpin,
{
    let line = read_line(reader).await?;
    let code = line.get(..3).and_then(|value| value.parse::<u16>().ok());
    if code == Some(expected) {
        Ok(())
    } else {
        Err(EmailError::Protocol(format!(
            "expected SMTP {expected}, got {line}"
        )))
    }
}

async fn write_command<R>(reader: &mut BufReader<R>, command: &str) -> Result<(), EmailError>
where
    R: AsyncRead + AsyncWrite + Unpin,
{
    let stream = reader.get_mut();
    stream.write_all(command.as_bytes()).await?;
    stream.write_all(b"\r\n").await?;
    stream.flush().await?;
    Ok(())
}

async fn read_line<R>(reader: &mut BufReader<R>) -> Result<String, EmailError>
where
    R: AsyncRead + Unpin,
{
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    if line.is_empty() {
        return Err(EmailError::Protocol("connection closed".to_owned()));
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

fn extract_rfc822_literal(response: &str) -> Option<String> {
    let marker = response.find('{')?;
    let end = response[marker..].find('}')? + marker;
    let length = response[marker + 1..end].parse::<usize>().ok()?;
    let start = response[end + 1..].find("\n")? + end + 1;
    let body_start = start + 1;
    response
        .get(body_start..body_start + length)
        .map(str::to_owned)
}

pub fn parse_rfc822(uid: u64, raw: &str) -> Option<EmailMessage> {
    let (headers, body) = raw.split_once("\n\n").unwrap_or((raw, ""));
    let mut message_id = None;
    let mut sender = None;
    let mut recipient = None;
    let mut subject = None;
    let mut current = String::new();
    for line in headers.lines() {
        if line.starts_with([' ', '\t']) {
            current.push_str(line.trim());
            continue;
        }
        if !current.is_empty() {
            assign_header(
                &current,
                &mut message_id,
                &mut sender,
                &mut recipient,
                &mut subject,
            );
        }
        current = line.to_owned();
    }
    if !current.is_empty() {
        assign_header(
            &current,
            &mut message_id,
            &mut sender,
            &mut recipient,
            &mut subject,
        );
    }
    let sender = sender.filter(|value| !value.is_empty())?;
    Some(EmailMessage {
        uid,
        message_id,
        sender,
        recipient,
        subject: subject.unwrap_or_else(|| "(no subject)".to_owned()),
        text: extract_text_body(body),
    })
}

fn assign_header(
    line: &str,
    message_id: &mut Option<String>,
    sender: &mut Option<String>,
    recipient: &mut Option<String>,
    subject: &mut Option<String>,
) {
    let Some((name, value)) = line.split_once(':') else {
        return;
    };
    let value = value.trim().to_owned();
    match name.to_ascii_lowercase().as_str() {
        "message-id" => *message_id = Some(value),
        "from" => *sender = Some(extract_address(&value)),
        "to" => *recipient = Some(extract_address(&value)),
        "subject" => *subject = Some(value),
        _ => {}
    }
}

fn extract_text_body(body: &str) -> String {
    if let Some(boundary) = body.lines().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        lower
            .strip_prefix("--boundary=")
            .map(|value| value.trim_matches('"').to_owned())
    }) {
        let marker = format!("--{boundary}");
        for part in body.split(&marker) {
            if part
                .to_ascii_lowercase()
                .contains("content-type: text/plain")
                && let Some((_, text)) = part.split_once("\n\n")
            {
                return decode_transfer(text.trim());
            }
        }
    }
    decode_transfer(body.trim())
}

fn decode_transfer(text: &str) -> String {
    text.lines()
        .filter(|line| !line.to_ascii_lowercase().starts_with("content-"))
        .map(decode_quoted_printable)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

fn decode_quoted_printable(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'='
            && index + 2 < bytes.len()
            && let Ok(value) = u8::from_str_radix(&line[index + 1..index + 3], 16)
        {
            output.push(value);
            index += 3;
            continue;
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn extract_address(value: &str) -> String {
    value
        .split('<')
        .nth(1)
        .and_then(|part| part.split('>').next())
        .unwrap_or(value)
        .trim()
        .to_owned()
}

fn normalize_address(value: &str) -> String {
    extract_address(value).to_ascii_lowercase()
}

fn clean_address(value: &str) -> String {
    extract_address(value).replace(['\r', '\n', '<', '>', ' '], "")
}

fn quote_atom(value: &str) -> String {
    format!("\"{}\"", value.replace(['\\', '"', '\r', '\n'], ""))
}

fn base64_auth(username: &str, password: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(format!("\0{username}\0{password}"))
}

fn required_env(key: &str) -> Result<String, EmailError> {
    env::var(key).map_err(|_| EmailError::Configuration(format!("{key} is required")))
}

fn parse_bool(key: &str, default: bool) -> bool {
    env::var(key)
        .ok()
        .and_then(|value| match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}

fn tls_connector(host: &str) -> Result<TlsConnector, EmailError> {
    let native = load_native_certs();
    let mut roots = RootCertStore::empty();
    for certificate in native.certs {
        roots.add(certificate).map_err(|error| {
            EmailError::Tls(format!("failed to add native certificate: {error}"))
        })?;
    }
    if roots.is_empty() {
        return Err(EmailError::Tls(format!(
            "no native root certificates available for {host}"
        )));
    }
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

fn server_name(host: &str) -> Result<rustls::pki_types::ServerName<'static>, EmailError> {
    rustls::pki_types::ServerName::try_from(host.to_owned())
        .map_err(|error| EmailError::Tls(format!("invalid email host {host}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::{EmailConfig, parse_rfc822};
    use std::collections::HashSet;

    #[test]
    fn parses_allowlisted_plain_message_and_subject_session() {
        let raw = "From: Alice <alice@example.com>\nTo: bot@example.com\nSubject: Build status\nMessage-ID: <m-1>\n\nhello\n";
        let message = parse_rfc822(7, raw).unwrap();
        assert_eq!(message.sender, "alice@example.com");
        assert_eq!(message.subject, "Build status");
        assert!(message.session_key().contains("Build%20status"));
        let config = EmailConfig {
            imap_host: "imap.example.com".into(),
            imap_port: 993,
            imap_tls: true,
            smtp_host: "smtp.example.com".into(),
            smtp_port: 465,
            smtp_tls: true,
            username: "bot@example.com".into(),
            password: "secret".into(),
            from: "bot@example.com".into(),
            allowed_senders: HashSet::from(["alice@example.com".into()]),
            poll_seconds: 30,
        };
        assert!(config.is_allowed_sender(&message.sender));
    }
}

#[allow(dead_code)]
type _TlsStream = TlsStream<TcpStream>;
