//! Real agent/ACP integration with isolated state; no provider account or network tools.
mod acp_harness;
use acp_harness::{AutoApproveClient, connect_and_auth, ext_method, new_session, run_agent_test};
use serde_json::{Value, json};
fn result(value: Value) -> Value {
    assert!(
        value.get("error").is_none_or(Value::is_null),
        "ACP operation failed: {value}"
    );
    value.get("result").cloned().unwrap_or(value)
}
#[test]
fn staged_plugin_and_accounts_survive_agent_recreation_without_exposing_secrets() {
    run_agent_test(|cwd, _server| async move {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        let package = cwd.join("fixture");
        std::fs::create_dir_all(package.join("skills/release-brief")).unwrap();
        std::fs::write(
            package.join("plugin.json"),
            r#"{"name":"fixture","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            package.join("mcp.json"),
            r#"{"servers":{"fixture-api":{"url":"https://example.invalid/mcp"}}}"#,
        )
        .unwrap();
        std::fs::write(package.join("skills/release-brief/SKILL.md"), "---\nname: release-brief\ndescription: Prepare release briefs\n---\nDraft a sourced release brief.").unwrap();
        let (conn, init) = connect_and_auth(AutoApproveClient, "plugin-management-test").await;
        let init = serde_json::to_value(init).unwrap();
        assert_eq!(init["_meta"]["_supercharge/plugins"]["version"], 1);
        let sid = new_session(&conn, &cwd).await;
        let response = result(
            ext_method(
                &conn,
                "supercharge/plugins/action",
                json!({"sessionId":sid,"action":{"type":"install","source":package}}),
            )
            .await,
        );
        assert_eq!(response["status"], "success");
        result(
            ext_method(
                &conn,
                "supercharge/plugins/action",
                json!({"sessionId":sid,"action":{"type":"reload"}}),
            )
            .await,
        );
        let listed =
            result(ext_method(&conn, "supercharge/plugins/list", json!({"sessionId":sid})).await);
        let plugin = listed["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "fixture")
            .unwrap();
        assert_eq!(
            plugin["enabled"], false,
            "installation must not authorize execution"
        );
        assert_eq!(plugin["mcpServerCount"], 1);
        assert_eq!(plugin["skillCount"], 1);
        let details = result(
            ext_method(
                &conn,
                "supercharge/plugins/details",
                json!({"id":plugin["id"]}),
            )
            .await,
        );
        assert_eq!(
            details["servers"][0]["endpoint"],
            "https://example.invalid/mcp"
        );
        for account in ["work", "personal"] {
            let configured = result(ext_method(&conn, "supercharge/connections/configure", json!({"sessionId":sid,"plugin":"fixture","server":"fixture-api","account":account,"secretKind":"bearer","secret":"test-secret-never-returned"})).await);
            assert_eq!(configured["status"], "success");
        }
        let (other, _) = connect_and_auth(AutoApproveClient, "second-interface-test").await;
        let other_sid = new_session(&other, &cwd).await;
        let accounts = result(
            ext_method(
                &other,
                "supercharge/connections/list",
                json!({"sessionId":other_sid}),
            )
            .await,
        );
        assert!(!accounts.to_string().contains("test-secret-never-returned"));
        let list = accounts["accounts"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list.iter().filter(|a| a["active"] == true).count(), 1);
        let work = list.iter().find(|a| a["account"] == "work").unwrap();
        assert_eq!(
            result(
                ext_method(
                    &other,
                    "supercharge/connections/select",
                    json!({"sessionId":other_sid,"id":work["id"]})
                )
                .await
            )["status"],
            "success"
        );
        let disconnected = result(
            ext_method(
                &other,
                "supercharge/connections/disconnect",
                json!({"sessionId":other_sid,"id":work["id"]}),
            )
            .await,
        );
        assert_eq!(disconnected["status"], "success");
        let remaining = result(
            ext_method(
                &conn,
                "supercharge/connections/list",
                json!({"sessionId":sid}),
            )
            .await,
        );
        assert_eq!(remaining["accounts"].as_array().unwrap().len(), 1);
        assert!(
            !std::fs::read_to_string(package.join("mcp.json"))
                .unwrap()
                .contains("test-secret")
        );
    });
}
