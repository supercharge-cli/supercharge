use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentCommand {
    New,
    Start,
    Stop,
    Status,
    Help,
    Model(Option<String>),
    Format(Option<String>),
    Setup,
    ISetup,
    IModels(Option<String>),
    Update(crate::self_update::UpdateAction),
    /// `/cmd <shell>` — run a command in the sandbox workspace.
    Cmd(String),
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
        "format" => Some(AgentCommand::Format(parts.next().map(str::to_owned))),
        "setup" => Some(AgentCommand::Setup),
        "isetup" | "imagine-setup" | "image-setup" => Some(AgentCommand::ISetup),
        "imodels" | "imodel" | "imagine-model" | "image-model" => {
            Some(AgentCommand::IModels(parts.next().map(str::to_owned)))
        }
        "cmd" | "exec" | "sh" => {
            let rest = input
                .trim()
                .split_once(char::is_whitespace)
                .map(|(_, rest)| rest.trim())
                .unwrap_or("")
                .to_owned();
            Some(AgentCommand::Cmd(rest))
        }
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
    "/new — clear the stopped state for this session\n\
/start — resume this session\n\
/stop — pause agent turns for this session\n\
/status — show daemon control state\n\
/model [id] — inspect or set the chat model\n\
/format [chat_completions|messages|responses] — show or set the API format\n\
/setup — chat provider: set it in Vectra instance settings (TUI: /setup)\n\
/isetup — image provider: set it in Vectra instance settings (TUI: /isetup)\n\
/imodels [id] — show or set the image model (any custom id)\n\
/imagine <description> — generate an image (needs image provider configured)\n\
/cmd <command> — run a shell command in the sandbox workspace\n\
/update — check for a Supercharge binary update (E2B)\n\
/help — show this help"
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
        assert_eq!(parse_command("/isetup"), Some(AgentCommand::ISetup));
        assert_eq!(
            parse_command("/imodels flux"),
            Some(AgentCommand::IModels(Some("flux".to_owned())))
        );
        assert_eq!(
            parse_command("/cmd ls -la /tmp"),
            Some(AgentCommand::Cmd("ls -la /tmp".to_owned()))
        );
        assert_eq!(
            parse_command("/cmd"),
            Some(AgentCommand::Cmd(String::new()))
        );
        assert_eq!(
            parse_command("/format responses"),
            Some(AgentCommand::Format(Some("responses".to_owned())))
        );
        assert_eq!(parse_command("/FORMAT"), Some(AgentCommand::Format(None)));
    }
}
