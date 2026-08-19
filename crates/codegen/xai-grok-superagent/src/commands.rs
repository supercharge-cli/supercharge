use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentCommand {
    New,
    Start,
    Stop,
    Status,
    Help,
    Model(Option<String>),
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
        _ => None,
    }
}

pub fn help_text() -> &'static str {
    "/new — clear the stopped state for this session\n/start — resume this session\n/stop — pause agent turns for this session\n/status — show daemon control state\n/model [id] — inspect or set the requested model\n/help — show this help"
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
