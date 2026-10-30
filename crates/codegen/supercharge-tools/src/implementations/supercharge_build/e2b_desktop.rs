//! X11 tools for the existing E2B desktop. Never start or target a local desktop.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use supercharge_tool_runtime::{ToolCallContext, ToolError};

use super::e2b_browser::{BrowserToolOutput, call_sidecar, e2b_enabled};
use crate::types::tool::{ToolKind, ToolNamespace};

pub fn available() -> bool {
    e2b_enabled() && std::env::var("SUPERCHARGE_DESKTOP_PROTOCOL").as_deref() == Ok("1")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct DesktopObserveInput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopActionInput {
    /// Frame returned by the latest desktop_observe. Coordinates refer to that delivered image.
    pub frame_id: String,
    /// Ordered sequence of 1-24 actions, executed without another bot interleaving input.
    pub actions: Vec<DesktopAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopAction {
    Move {
        x: u32,
        y: u32,
    },
    Click {
        x: u32,
        y: u32,
        #[serde(default)]
        button: MouseButton,
        #[serde(default = "one")]
        count: u8,
    },
    Drag {
        x: u32,
        y: u32,
        #[serde(rename = "toX")]
        to_x: u32,
        #[serde(rename = "toY")]
        to_y: u32,
        #[serde(default)]
        button: MouseButton,
        #[serde(default = "drag_duration", rename = "durationMs")]
        duration_ms: u32,
    },
    ButtonDown {
        #[serde(default)]
        button: MouseButton,
    },
    ButtonUp {
        #[serde(default)]
        button: MouseButton,
    },
    Scroll {
        direction: ScrollDirection,
        #[serde(default = "three")]
        amount: u8,
    },
    /// Named key or shortcut, for example Enter, Ctrl+L, Alt+Tab.
    Key {
        key: String,
    },
    KeyDown {
        key: String,
    },
    KeyUp {
        key: String,
    },
    /// Paste literal Unicode text into the focused application. Never include secrets.
    Type {
        text: String,
    },
    Focus {
        #[serde(rename = "windowId")]
        window_id: String,
    },
    /// `terminal`, `files`, `browser`, or a `.desktop` ID listed in desktop_observe (e.g. `discord.desktop`).
    Launch {
        application: String,
    },
    /// Open an HTTP(S) URL or an absolute file path under /home/user using its default app.
    Open {
        target: String,
    },
    Release,
}

fn one() -> u8 {
    1
}
fn three() -> u8 {
    3
}
fn drag_duration() -> u32 {
    400
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    #[default]
    Left,
    Middle,
    Right,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}
async fn invoke(
    ctx: ToolCallContext,
    path: &str,
    payload: Option<serde_json::Value>,
) -> Result<BrowserToolOutput, ToolError> {
    if !available() {
        return Err(ToolError::custom(
            "desktop_unavailable",
            "Desktop tools require an explicitly enabled E2B desktop runtime. They cannot control the local computer.",
        ));
    }
    let cancellation = ctx.get::<supercharge_tool_runtime::Cancellation>();
    let request = call_sidecar(
        if payload.is_some() {
            reqwest::Method::POST
        } else {
            reqwest::Method::GET
        },
        path,
        payload,
    );
    if let Some(cancellation) = cancellation {
        tokio::select! {
            result = request => result,
            _ = cancellation.0.cancelled() => Err(ToolError::custom("desktop_cancelled", "Desktop action cancelled; observe again before acting")),
        }
    } else {
        request.await
    }
}

macro_rules! desktop_tool {
    ($tool:ident, $input:ty, $id:literal, $description:literal, $readonly:expr, $path:literal, $payload:expr) => {
        #[derive(Debug, Default)]
        pub struct $tool;
        impl crate::types::tool_metadata::ToolMetadata for $tool {
            fn kind(&self) -> ToolKind {
                ToolKind::Other
            }
            fn tool_namespace(&self) -> ToolNamespace {
                ToolNamespace::SuperchargeBuild
            }
            fn description_template(&self) -> &str {
                $description
            }
            fn is_read_only(&self) -> bool {
                $readonly
            }
        }
        impl supercharge_tool_runtime::Tool for $tool {
            type Args = $input;
            type Output = BrowserToolOutput;
            fn id(&self) -> supercharge_tool_protocol::ToolId {
                supercharge_tool_protocol::ToolId::new($id).expect("valid desktop tool ID")
            }
            fn description(
                &self,
                _: &supercharge_tool_runtime::ListToolsContext,
            ) -> supercharge_tool_types::ToolDescription {
                supercharge_tool_types::ToolDescription::new($id, $description)
            }
            fn capabilities(&self) -> supercharge_tool_protocol::ToolCapabilities {
                supercharge_tool_protocol::ToolCapabilities {
                    is_read_only: $readonly,
                    tool_scope: Some(if $readonly {
                        supercharge_tool_protocol::ToolScope::Read
                    } else {
                        supercharge_tool_protocol::ToolScope::Write
                    }),
                    ..Default::default()
                }
            }
            async fn run(
                &self,
                ctx: ToolCallContext,
                input: $input,
            ) -> Result<BrowserToolOutput, ToolError> {
                let payload: Result<Option<serde_json::Value>, ToolError> = $payload(input);
                invoke(ctx, $path, payload?).await
            }
        }
    };
}

desktop_tool!(
    DesktopObserveTool,
    DesktopObserveInput,
    "desktop_observe",
    "Observe the SAME Ubuntu desktop shown to the user inside the E2B VM: screenshot image, windows, pointer, installed applications, display geometry and frameId. Coordinates for desktop_action use the delivered image, not the original display. Launch installed apps by their listed .desktop ID, such as discord.desktop if installed. Prefer browser_snapshot for DOM tasks. No observations are permitted during human takeover. Never use shell screenshots to bypass a handoff.",
    true,
    "/desktop/observe",
    |_: DesktopObserveInput| Ok(None)
);

desktop_tool!(
    DesktopActionTool,
    DesktopActionInput,
    "desktop_action",
    "Control the actual mouse, keyboard and application windows of the existing Ubuntu E2B desktop, never the user's local computer. Supply the latest desktop_observe frameId and 1-24 ordered actions. Use image-pixel coordinates. Launch any installed GUI app by its listed .desktop ID; if not installed, install it in the VM first and observe again. Observe after each sequence. Use native browser tools for DOM interactions. Request manual browser_handoff for CAPTCHA, MFA, login or security challenges; do not bypass ownership locks or security checks.",
    false,
    "/desktop/action",
    |input: DesktopActionInput| {
        if input.frame_id.is_empty()
            || input.frame_id.len() > 128
            || input.actions.is_empty()
            || input.actions.len() > 24
        {
            return Err(ToolError::invalid_arguments(
                "A frameId and 1-24 actions are required",
            ));
        }
        serde_json::to_value(input)
            .map(Some)
            .map_err(|error| ToolError::invalid_arguments(error.to_string()))
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::tool_metadata::ToolMetadata;

    #[test]
    fn desktop_permissions_distinguish_observation_from_input() {
        assert!(ToolMetadata::is_read_only(&DesktopObserveTool));
        assert!(!ToolMetadata::is_read_only(&DesktopActionTool));
        assert!(supercharge_tool_runtime::Tool::capabilities(&DesktopObserveTool).is_read_only);
        assert!(!supercharge_tool_runtime::Tool::capabilities(&DesktopActionTool).is_read_only);
    }

    #[test]
    fn desktop_wire_shape_is_typed_and_rejects_unknown_actions() {
        let input: DesktopActionInput = serde_json::from_value(serde_json::json!({"frameId":"frame", "actions":[{"action":"drag","x":1,"y":2,"toX":3,"toY":4}]})).unwrap();
        assert_eq!(serde_json::to_value(input).unwrap()["actions"][0]["toX"], 3);
        assert!(serde_json::from_value::<DesktopActionInput>(serde_json::json!({"frameId":"f", "actions":[{"action":"shell","command":"anything"}]})).is_err());
    }
}
