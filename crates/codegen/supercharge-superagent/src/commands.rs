use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentCommand {
    New,
    Start,
    Stop,
    Status,
    Help,
    Model(Option<String>),
    Update(crate::self_update::UpdateAction),
}

pub fn parse_command(input: &str) -> Option<AgentCommand> {
    let mut parts = input.split_whitespace();
    let command = parts.next()?.strip_prefix('/')?.to_ascii_lowercase();
    match command.as_str() {
        "new" => Some(AgentCommand::New),
        "start" | "resume" => Some(AgentCommand::Start),
        "stop" | "cancel" => Some(AgentCommand::Stop),
        "status" => Some(AgentCommand::Status),
        "help" => Some(AgentCommand::Help),
        "model" => Some(AgentCommand::Model(parts.next().map(str::to_owned))),
        "update" => {
            let arg = parts.next().map(str::to_ascii_lowercase);
            let action = match arg.as_deref() {
                Some("yes") | Some("apply") => crate::self_update::UpdateAction::Apply,
                Some("no") | Some("dismiss") => crate::self_update::UpdateAction::Dismiss,
                _ => crate::self_update::UpdateAction::Check,
            };
            Some(AgentCommand::Update(action))
        }
        _ => None,
    }
}

pub fn help_text() -> &'static str {
    "/new — clear the stopped state for this session\n/start — resume this session\n/stop — pause agent turns for this session\n/status — show daemon control state\n/model [id] — inspect or set the requested model\n/update — check for a Supercharge binary update (E2B)\n/help — show this help"
}

#[cfg(test)]
mod tests {
    use super::{AgentCommand, parse_command};

    #[test]
    fn parses_shared_control_commands_case_insensitively() {
        assert_eq!(parse_command("/NEW"), Some(AgentCommand::New));
        assert_eq!(parse_command("/stop"), Some(AgentCommand::Stop));
        assert_eq!(
            parse_command("/model kimi-code"),
            Some(AgentCommand::Model(Some("kimi-code".to_owned())))
        );
        assert_eq!(parse_command("ordinary prompt"), None);
    }
}
