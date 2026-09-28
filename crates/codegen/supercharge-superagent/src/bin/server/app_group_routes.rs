use super::*;
use super::app_group::{GroupMessage, MAX_DELEGATION_DEPTH, MAX_GROUP_TEXT, group_context, group_session, mention_targets};
use hmac::{Hmac, Mac};
use sha2::Sha256;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupRequest {
    pub request_id: String,
    pub text: String,
    #[serde(default)]
    pub mentions: Vec<String>,
    #[serde(default)]
    pub sender_bot_id: Option<String>,
    #[serde(default)]
    pub sender_session_id: Option<String>,
    #[serde(default)]
    pub agent_only: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotCallerRequest {
    pub bot_id: String,
    pub session_id: String,
    pub credential: String,
}

pub fn conversation_capability(root: &str, bot_id: &str, session_id: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(root.as_bytes()).expect("HMAC accepts any key length");
    mac.update(format!("vectra-conversation-v1\n{bot_id}\n{session_id}").as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

pub async fn authenticate_caller(
    State(state): State<Arc<AppState>>, Json(request): Json<BotCallerRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    if !browser::is_e2b_runtime() { return Err(failure(StatusCode::NOT_FOUND, "E2B only")); }
    let catalog = state.bot_catalog.lock().await;
    if catalog.capability.is_empty() || catalog.get(&request.bot_id).is_none() {
        return Err(failure(StatusCode::UNAUTHORIZED, "Invalid agent identity"));
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(catalog.capability.as_bytes()).expect("HMAC accepts any key length");
    mac.update(format!("vectra-conversation-v1\n{}\n{}", request.bot_id, request.session_id).as_bytes());
    let supplied = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&request.credential)
        .map_err(|_| failure(StatusCode::UNAUTHORIZED, "Invalid agent identity"))?;
    if mac.verify_slice(&supplied).is_err() { return Err(failure(StatusCode::UNAUTHORIZED, "Invalid agent identity")); }
    drop(catalog);
    let sessions = state.sessions.lock().await;
    if sessions.get(&request.session_id).is_none_or(|session| session.bot_id != request.bot_id) {
        return Err(failure(StatusCode::UNAUTHORIZED, "Invalid agent conversation"));
    }
    Ok(Json(json!({"ok":true,"botId":request.bot_id,"sessionId":request.session_id})))
}

pub async fn snapshot(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    if !browser::is_e2b_runtime() { return Err(failure(StatusCode::NOT_FOUND, "E2B only")); }
    let queue = state.bot_queue.lock().await;
    let messages = queue.group_messages.iter().rev().take(300).cloned().collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>();
    let activity = queue.in_flight.values().map(|work| json!({
        "botId":work.bot_id,"sessionId":work.session(),"workId":work.id,"state":"working",
        "groupMessageId":work.group_message_id,"startedAt":work.started_at,
    })).chain(queue.queued.iter().map(|work| json!({
        "botId":work.bot_id,"sessionId":work.session(),"workId":work.id,"state":"queued",
        "groupMessageId":work.group_message_id,
    }))).collect::<Vec<_>>();
    Ok(Json(json!({"messages":messages,"activity":activity,"hasMore":queue.group_messages.len()>300})))
}

pub async fn send(
    State(state): State<Arc<AppState>>, Json(request): Json<GroupRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<ErrorResponse>)> {
    if !browser::is_e2b_runtime() { return Err(failure(StatusCode::NOT_FOUND, "E2B only")); }
    if request.agent_only && request.sender_bot_id.is_none() {
        return Err(failure(StatusCode::FORBIDDEN, "Agent identity is required"));
    }
    if request.text.trim().is_empty() || request.text.len() > MAX_GROUP_TEXT || !valid_id(&request.request_id, 120) {
        return Err(failure(StatusCode::BAD_REQUEST, "Group message must be 1–16,000 bytes with a valid request ID"));
    }
    if request.sender_bot_id.is_some() != request.sender_session_id.is_some() {
        return Err(failure(StatusCode::BAD_REQUEST, "Agent conversation identity is required"));
    }
    let _dispatch = state.bot_dispatch.lock().await;
    if state.app_maintenance.load(Ordering::SeqCst) { return Err(failure(StatusCode::SERVICE_UNAVAILABLE, "Sandbox is pausing; retry after resume")); }
    let bots = state.bot_catalog.lock().await.bots.values().filter(|bot| !bot.archived).cloned().collect::<Vec<_>>();
    let mut recipients = mention_targets(&request.text, &request.mentions, &bots)
        .map_err(|error| failure(StatusCode::BAD_REQUEST, &error))?;
    let mut depth = 0;
    let mut reply_to = None;
    let mut source_work = None;
    if let (Some(sender), Some(session_id)) = (&request.sender_bot_id, &request.sender_session_id) {
        if !bots.iter().any(|bot| &bot.id == sender) || state.sessions.lock().await.get(session_id).is_none_or(|session| &session.bot_id != sender) {
            return Err(failure(StatusCode::FORBIDDEN, "Agent conversation does not belong to the sender"));
        }
        let queue = state.bot_queue.lock().await;
        let work = queue.in_flight.values().find(|work| work.session() == session_id && &work.bot_id == sender)
            .ok_or_else(|| failure(StatusCode::CONFLICT, "Only an active agent turn can send a group message"))?;
        if work.group_message_id.is_none() {
            return Err(failure(StatusCode::FORBIDDEN, "Private DM turns cannot publish to the shared group; delegate privately instead"));
        }
        depth = work.depth.saturating_add(1);
        source_work = Some(work.id.clone());
        reply_to = work.group_reply_to.clone().or_else(|| work.group_message_id.clone());
        recipients.retain(|bot| bot != sender);
    } else if recipients.is_empty() {
        recipients.push(MAIN_BOT_ID.to_owned());
    }
    if depth > MAX_DELEGATION_DEPTH && !recipients.is_empty() {
        return Err(failure(StatusCode::CONFLICT, "Agent mention chain is at its limit. Summarize the current result for the user."));
    }
    let message = GroupMessage {
        id: request.request_id.clone(), sender_bot_id: request.sender_bot_id.clone(), sender_session_id: request.sender_session_id.clone(),
        text: request.text.trim().to_owned(), mentions: recipients.clone(), created_at: unix_millis(),
        reply_to, work_id: source_work, session_id: None, kind: "message".into(),
    };
    let mut sessions = state.sessions.lock().await;
    let mut works = Vec::new();
    for recipient in &recipients {
        let bot = bots.iter().find(|bot| &bot.id == recipient).ok_or_else(|| failure(StatusCode::NOT_FOUND, "Tagged agent not found"))?;
        let session_id = group_session(recipient);
        let cwd = validate_folder(&bot.default_folder).map_err(|error| failure(StatusCode::BAD_REQUEST, &error))?;
        let now = unix_seconds();
        let session = sessions.entry(session_id.clone()).or_insert_with(|| DaemonSession {
            id: session_id.clone(), created_at: now, updated_at: now, title: "Group chat".into(), status: default_session_status(),
            messages: Vec::new(), bot_id: bot.id.clone(), cwd: Some(cwd.to_string_lossy().into_owned()),
            instructions_revision: bot.revision, runtime_home: None,
        });
        if session.bot_id != *recipient { return Err(failure(StatusCode::CONFLICT, "Group agent conversation is unavailable")); }
        let work_id = format!("group-{}-{}", request.request_id, recipient);
        works.push(BotWork {
            id: work_id, chat_id: session_id.clone(), session_id, bot_id: recipient.clone(),
            prompt: message.text.clone(), job_id: None, origin_bot_id: None, origin_session_id: None, origin_work_id: None,
            group_message_id: Some(message.id.clone()), group_reply_to: Some(message.id.clone()), depth,
            queued_at: message.created_at, started_at: None,
        });
    }
    persist_daemon_sessions(&state.session_state_path, &sessions).map_err(internal_error)?;
    drop(sessions);
    let added = state.bot_queue.lock().await.enqueue_group(message.clone(), works, &state.bot_queue_path)
        .map_err(|error| failure(StatusCode::CONFLICT, &error))?;
    if added { app_emit(&state, "group", json!({"type":"group_message","message":message})); }
    drop(_dispatch);
    schedule_bot_queue(state);
    Ok((StatusCode::ACCEPTED, Json(json!({"ok":true,"messageId":request.request_id,"recipients":recipients,"duplicate":!added}))))
}

pub async fn work_prompt(state: &AppState, work: &BotWork) -> String {
    let Some(message_id) = &work.group_message_id else { return work.prompt.clone() };
    let catalog = state.bot_catalog.lock().await;
    let Some(bot) = catalog.get(&work.bot_id) else { return work.prompt.clone() };
    drop(catalog);
    let queue = state.bot_queue.lock().await;
    let context = group_context(&queue.group_messages, message_id, &bot);
    if work.id.starts_with("result-") { format!("{context}\n\n{}", work.prompt) } else { context }
}

pub async fn emit_pending_results(state: &AppState) {
    let pending = {
        let queue = state.bot_queue.lock().await;
        queue.group_messages.iter().filter(|message| message.work_id.is_some() && message.id.starts_with("result-") && !queue.group_published.contains(&message.id)).cloned().collect::<Vec<_>>()
    };
    for message in pending {
        app_emit(state, "group", json!({"type":"group_message","message":message}));
        let _ = state.bot_queue.lock().await.published_group_result(&message.id, &state.bot_queue_path);
    }
}

fn valid_id(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn failure(status: StatusCode, error: &str) -> (StatusCode, Json<ErrorResponse>) {
    (status, Json(ErrorResponse { error: error.to_owned() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_credentials_are_scoped_to_bot_and_conversation() {
        let main = conversation_capability("root-secret", "main", "chat-one");
        assert_ne!(main, conversation_capability("root-secret", "research", "chat-one"));
        assert_ne!(main, conversation_capability("root-secret", "main", "chat-two"));
        assert_eq!(main, conversation_capability("root-secret", "main", "chat-one"));
    }
}
