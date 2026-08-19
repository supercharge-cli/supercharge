//! Build-API unary calls + parallel context-upload orchestration.
//!
//! Ports `requestBuild`, `triggerBuild`, `getBuildStatus` from
//! `packages/js-sdk/src/template/buildApi.ts` and the context-upload
//! orchestration (parallel `instructionsWithHashes` + upload loop) from
//! `packages/js-sdk/src/template/index.ts`.
//!
//! All items are `pub(crate)` — they are implementation details consumed by
//! the template builder and are not part of the public API.
//!
//! These functions are called by Task 3 of Plan 5c (build orchestration).

use std::path::Path;

use futures::future::try_join_all;

use crate::api::client::ApiClient;
use crate::errors::{Error, Result};
use crate::template::archive::tar_file_stream;
use crate::template::files::{calculate_files_hash, read_dockerignore};
use crate::template::types::{Instruction, InstructionType, TemplateBuildStatusResponse};
use crate::template::upload::{FILE_UPLOAD_TIMEOUT_MS, get_file_upload_link, upload_file};

// ─── request_build ───────────────────────────────────────────────────────────

/// Request a new template build slot from the control plane.
///
/// Issues `POST /v3/templates` with a JSON body that contains `name`, optional
/// `tags` (omitted when empty), and optional `cpuCount` / `memoryMB` (omitted
/// when `None`). Returns the raw
/// [`crate::api::schema::TemplateRequestResponseV3`] wire type, which contains
/// the `templateID` and `buildID` required by subsequent calls.
///
/// # Errors
///
/// Propagates transport and API errors from [`ApiClient`].
pub(crate) async fn request_build(
    api: &ApiClient,
    name: &str,
    tags: &[String],
    cpu_count: Option<u32>,
    memory_mb: Option<u32>,
) -> Result<crate::api::schema::TemplateRequestResponseV3> {
    let mut body = serde_json::json!({ "name": name });
    if !tags.is_empty() {
        body["tags"] = serde_json::json!(tags);
    }
    if let Some(cpu) = cpu_count {
        body["cpuCount"] = serde_json::json!(cpu);
    }
    if let Some(mem) = memory_mb {
        body["memoryMB"] = serde_json::json!(mem);
    }
    api.request::<crate::api::schema::TemplateRequestResponseV3>(
        reqwest::Method::POST,
        "/v3/templates",
        &[],
        Some(&body),
    )
    .await
}

// ─── trigger_build ────────────────────────────────────────────────────────────

/// Trigger an already-requested template build.
///
/// Issues `POST /v2/templates/{template_id}/builds/{build_id}` with the
/// serialized [`crate::api::schema::TemplateBuildStartV2`] body.  A successful
/// response is `204 No Content` — the function returns `Ok(())`.
///
/// # Errors
///
/// Propagates transport and API errors from [`ApiClient`], plus
/// [`crate::Error::Internal`] if the body cannot be JSON-serialized (extremely
/// unlikely given the generated type).
pub(crate) async fn trigger_build(
    api: &ApiClient,
    template_id: &str,
    build_id: &str,
    body: &crate::api::schema::TemplateBuildStartV2,
) -> Result<()> {
    let path = format!("/v2/templates/{template_id}/builds/{build_id}");
    let body_value = serde_json::to_value(body)
        .map_err(|e| Error::Internal(format!("failed to serialize build body: {e}")))?;
    api.request_unit(reqwest::Method::POST, &path, &[], Some(&body_value))
        .await
}

// ─── get_build_status ────────────────────────────────────────────────────────

/// Poll the build-status endpoint for an in-progress or completed build.
///
/// Issues `GET /templates/{template_id}/builds/{build_id}/status` with the
/// `logsOffset` query parameter, decodes the wire
/// [`crate::api::schema::TemplateBuildInfo`] response, and maps it to a
/// [`TemplateBuildStatusResponse`] via
/// [`TemplateBuildStatusResponse::from_wire`].
///
/// # Errors
///
/// Propagates transport and API errors from [`ApiClient`].
pub(crate) async fn get_build_status(
    api: &ApiClient,
    template_id: &str,
    build_id: &str,
    logs_offset: usize,
) -> Result<TemplateBuildStatusResponse> {
    let path = format!("/templates/{template_id}/builds/{build_id}/status");
    let wire = api
        .request::<crate::api::schema::TemplateBuildInfo>(
            reqwest::Method::GET,
            &path,
            &[("logsOffset", logs_offset.to_string())],
            None,
        )
        .await?;
    Ok(TemplateBuildStatusResponse::from_wire(wire))
}

// ─── instructions_with_hashes ─────────────────────────────────────────────────

/// Return a clone of `instructions` where each `COPY` instruction has its
/// `files_hash` field populated.
///
/// For each [`InstructionType::Copy`] instruction the hash is computed by
/// [`calculate_files_hash`] using:
/// - `src` = `args[0]` (the glob pattern stored by the builder).
/// - `dest` = `args[1]` (the destination path in the image).
/// - `ignore` = patterns from the `.dockerignore` file at `context`.
/// - `resolve_symlinks` = `instruction.resolve_symlinks`.
///
/// Instructions whose `args` lack a `src` or `dest` pass through unchanged.
/// Non-`COPY` instructions are also passed through unchanged.
///
/// # Errors
///
/// Propagates [`crate::Error::Internal`] / [`crate::Error::InvalidArgument`]
/// from [`calculate_files_hash`] (e.g. no files found matching the glob).
pub(crate) fn instructions_with_hashes(
    instructions: &[Instruction],
    context: &Path,
) -> Result<Vec<Instruction>> {
    let ignore = read_dockerignore(context);
    let mut result = Vec::with_capacity(instructions.len());
    for instr in instructions {
        if instr.instruction_type == InstructionType::Copy {
            match (instr.args.first(), instr.args.get(1)) {
                (Some(src), Some(dest)) => {
                    let hash =
                        calculate_files_hash(src, dest, context, &ignore, instr.resolve_symlinks)?;
                    let mut enriched = instr.clone();
                    enriched.files_hash = Some(hash);
                    result.push(enriched);
                }
                _ => result.push(instr.clone()),
            }
        } else {
            result.push(instr.clone());
        }
    }
    Ok(result)
}

// ─── upload_build_context ─────────────────────────────────────────────────────

/// Upload file-context archives for all `COPY` instructions not yet cached.
///
/// For each [`InstructionType::Copy`] instruction that has both a `files_hash`
/// and a `src` (`args[0]`):
///
/// 1. [`get_file_upload_link`] checks whether the archive is already in the
///    build cache.
/// 2. If `!present` **and** `url` is `Some`: stream a gzip-tar archive with
///    [`tar_file_stream`] and upload it via [`upload_file`].
///
/// All uploads run **concurrently** via [`try_join_all`]; the first error
/// aborts the whole operation.
///
/// # Errors
///
/// Returns the first error encountered (transport, API, archive, or upload
/// failure).
pub(crate) async fn upload_build_context(
    api: &ApiClient,
    http: &reqwest::Client,
    template_id: &str,
    instructions: &[Instruction],
    context: &Path,
) -> Result<()> {
    let ignore = read_dockerignore(context);

    // Collect the data needed for each upload so that the async blocks below
    // can capture owned values (hashes, src strings) without borrowing from
    // `instructions` across await points.
    struct WorkItem {
        files_hash: String,
        src: String,
        resolve_symlinks: bool,
        force_upload: bool,
    }

    let work_items: Vec<WorkItem> = instructions
        .iter()
        .filter(|i| i.instruction_type == InstructionType::Copy)
        .filter_map(|i| {
            let files_hash = i.files_hash.as_ref()?.clone();
            let src = i.args.first()?.clone();
            Some(WorkItem {
                files_hash,
                src,
                resolve_symlinks: i.resolve_symlinks,
                force_upload: i.force_upload.unwrap_or(false),
            })
        })
        .collect();

    let upload_futures = work_items.iter().map(|item| async {
        let link = get_file_upload_link(api, template_id, &item.files_hash).await?;
        // Upload when the context is absent from the cache OR when the
        // instruction forces it (`force_upload`) — matches the JS condition
        // `(forceUpload && url) || (!present && url)` (index.ts:1182).
        if (item.force_upload || !link.present())
            && let Some(url) = link.url().map(|u| u.to_owned())
        {
            let (archive, size) =
                tar_file_stream(&item.src, context, &ignore, item.resolve_symlinks)?;
            upload_file(http, &url, archive, size, FILE_UPLOAD_TIMEOUT_MS).await?;
        }
        Ok::<(), Error>(())
    });

    try_join_all(upload_futures).await?;
    Ok(())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::client::ApiClient;
    use crate::connection_config::{ConnectionConfig, ConnectionConfigOpts};
    use crate::template::files::calculate_files_hash;
    use crate::template::types::{BuildStatus, InstructionType};
    use wiremock::matchers::{body_partial_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ── test helpers ─────────────────────────────────────────────────────────

    fn api_client_for(server: &MockServer) -> ApiClient {
        let config = ConnectionConfig::new(ConnectionConfigOpts {
            api_key: Some("e2b_0123456789abcdef".to_string()),
            api_url: Some(server.uri()),
            ..Default::default()
        });
        ApiClient::new(&config, true).expect("construct ApiClient")
    }

    // ── request_build_posts ───────────────────────────────────────────────────

    /// `request_build` must POST to `/v3/templates` with the `name` field in
    /// the body and the `X-API-KEY` header, then decode the returned JSON into
    /// `TemplateRequestResponseV3`.
    #[tokio::test]
    async fn request_build_posts() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v3/templates"))
            .and(header("X-API-KEY", "e2b_0123456789abcdef"))
            .and(body_partial_json(serde_json::json!({ "name": "t" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "templateID": "tpl_1",
                "buildID": "bld_1",
                "aliases": [],
                "names": [],
                "public": false,
                "tags": []
            })))
            .mount(&server)
            .await;

        let api = api_client_for(&server);
        let result = request_build(&api, "t", &[], None, None)
            .await
            .expect("request_build must succeed");

        assert_eq!(result.template_id, "tpl_1");
        assert_eq!(result.build_id, "bld_1");
    }

    /// Optional `cpuCount` and `memoryMB` fields are serialized when provided.
    #[tokio::test]
    async fn request_build_includes_optional_fields() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v3/templates"))
            .and(body_partial_json(
                serde_json::json!({ "name": "t", "cpuCount": 2, "memoryMB": 512 }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "templateID": "tpl_2",
                "buildID": "bld_2",
                "aliases": [],
                "names": [],
                "public": false,
                "tags": []
            })))
            .mount(&server)
            .await;

        let api = api_client_for(&server);
        let result = request_build(&api, "t", &[], Some(2), Some(512))
            .await
            .expect("request_build with options must succeed");

        assert_eq!(result.template_id, "tpl_2");
    }

    // ── trigger_build_posts ───────────────────────────────────────────────────

    /// `trigger_build` must POST to the correct path with the body derived from
    /// `TemplateBuildStartV2` and succeed on a 204 response.
    #[tokio::test]
    async fn trigger_build_posts() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v2/templates/tpl_1/builds/bld_1"))
            .and(header("X-API-KEY", "e2b_0123456789abcdef"))
            .and(body_partial_json(
                serde_json::json!({ "fromImage": "node:20" }),
            ))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let api = api_client_for(&server);
        let body = crate::api::schema::TemplateBuildStartV2 {
            from_image: Some("node:20".to_string()),
            ..Default::default()
        };
        trigger_build(&api, "tpl_1", "bld_1", &body)
            .await
            .expect("trigger_build must succeed on 204");
    }

    // ── get_build_status_maps ─────────────────────────────────────────────────

    /// `get_build_status` must GET the correct path with `logsOffset` as a
    /// query parameter, decode the honest `TemplateBuildInfo` wire JSON, and
    /// map it to `TemplateBuildStatusResponse` with the expected status and log
    /// entries.
    #[tokio::test]
    async fn get_build_status_maps() {
        let server = MockServer::start().await;

        // HONEST wire JSON: lowercase status, camelCase keys, logEntries with
        // lowercase level strings — matching the real API response format.
        Mock::given(method("GET"))
            .and(path("/templates/tpl_1/builds/bld_1/status"))
            .and(query_param("logsOffset", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "templateID": "tpl_1",
                "buildID": "bld_1",
                "status": "building",
                "logs": [],
                "logEntries": [
                    {
                        "level": "info",
                        "message": "x",
                        "timestamp": "2024-01-01T00:00:00Z"
                    }
                ]
            })))
            .mount(&server)
            .await;

        let api = api_client_for(&server);
        let response = get_build_status(&api, "tpl_1", "bld_1", 0)
            .await
            .expect("get_build_status must succeed");

        assert_eq!(response.status, BuildStatus::Building);
        assert_eq!(response.template_id, "tpl_1");
        assert_eq!(response.build_id, "bld_1");
        assert_eq!(response.log_entries.len(), 1);
        assert_eq!(response.log_entries[0].message(), "x");
    }

    // ── instructions_with_hashes ──────────────────────────────────────────────

    /// `instructions_with_hashes` fills `files_hash` on COPY instructions and
    /// leaves other instruction types unchanged.
    #[test]
    fn instructions_with_hashes_fills_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let context = dir.path();

        // Create a file the COPY instruction can match.
        std::fs::write(context.join("app.js"), b"console.log('hi')").expect("write app.js");

        let copy_instr = Instruction {
            instruction_type: InstructionType::Copy,
            // args layout: [src, dest, user_or_empty, mode_or_empty]
            args: vec![
                "app.js".to_string(),
                "/app/app.js".to_string(),
                String::new(),
                String::new(),
            ],
            force: false,
            force_upload: None,
            files_hash: None,
            resolve_symlinks: false,
        };
        let run_instr = Instruction {
            instruction_type: InstructionType::Run,
            args: vec!["echo hi".to_string()],
            force: false,
            force_upload: None,
            files_hash: None,
            resolve_symlinks: false,
        };

        let enriched =
            instructions_with_hashes(&[copy_instr, run_instr], context).expect("should succeed");

        assert_eq!(enriched.len(), 2);
        // COPY instruction must have a hash now.
        assert!(
            enriched[0].files_hash.is_some(),
            "COPY instruction must have files_hash"
        );
        // RUN instruction must be passed through unchanged.
        assert!(
            enriched[1].files_hash.is_none(),
            "RUN instruction must not have files_hash"
        );

        // The hash must be deterministic — calling again yields the same value.
        let enriched2 =
            instructions_with_hashes(&[enriched[0].clone()], context).expect("should succeed");
        assert_eq!(enriched[0].files_hash, enriched2[0].files_hash);
    }

    // ── upload_build_context — absent (upload expected) ────────────────────────

    /// When the file-upload-link endpoint returns `present: false`, the archive
    /// must be PUT to the presigned URL exactly once.
    #[tokio::test]
    async fn upload_build_context_uploads_when_absent() {
        let server = MockServer::start().await;

        // Create a context dir with a real file.
        let dir = tempfile::tempdir().expect("tempdir");
        let context = dir.path();
        std::fs::write(context.join("data.txt"), b"upload me").expect("write data.txt");

        // Pre-compute the real hash for the COPY instruction.
        let files_hash = calculate_files_hash("data.txt", "/app/data.txt", context, &[], false)
            .expect("compute hash");

        let upload_path = format!("/upload/{files_hash}");

        // File-upload-link mock: present: false, url points to the PUT mock.
        let get_url = format!("/templates/tpl_1/files/{files_hash}");
        let put_url = format!("{}{upload_path}", server.uri());
        Mock::given(method("GET"))
            .and(path(get_url))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "present": false,
                "url": put_url,
            })))
            .mount(&server)
            .await;

        // PUT mock — expect exactly one call.
        Mock::given(method("PUT"))
            .and(path(&upload_path))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let api = api_client_for(&server);
        let http = reqwest::Client::new();

        let instr = Instruction {
            instruction_type: InstructionType::Copy,
            args: vec![
                "data.txt".to_string(),
                "/app/data.txt".to_string(),
                String::new(),
                String::new(),
            ],
            force: false,
            force_upload: None,
            files_hash: Some(files_hash),
            resolve_symlinks: false,
        };

        upload_build_context(&api, &http, "tpl_1", &[instr], context)
            .await
            .expect("upload_build_context must succeed when absent");

        // wiremock asserts the expect(1) automatically on server drop.
    }

    // ── upload_build_context — present (upload skipped) ────────────────────────

    /// When the file-upload-link endpoint returns `present: true`, no PUT must
    /// be issued.
    #[tokio::test]
    async fn upload_build_context_skips_when_present() {
        let server = MockServer::start().await;

        // Context dir (files don't need to exist — hash is pre-filled).
        let dir = tempfile::tempdir().expect("tempdir");
        let context = dir.path();

        let files_hash = "deadbeef00112233".to_string();
        let get_url = format!("/templates/tpl_1/files/{files_hash}");

        // File-upload-link mock: present: true (no url needed).
        Mock::given(method("GET"))
            .and(path(get_url))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "present": true
            })))
            .mount(&server)
            .await;

        // PUT mock with expect(0) — must NOT receive any requests.
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let api = api_client_for(&server);
        let http = reqwest::Client::new();

        let instr = Instruction {
            instruction_type: InstructionType::Copy,
            args: vec![
                "app.js".to_string(),
                "/app/app.js".to_string(),
                String::new(),
                String::new(),
            ],
            force: false,
            force_upload: None,
            files_hash: Some(files_hash),
            resolve_symlinks: false,
        };

        upload_build_context(&api, &http, "tpl_1", &[instr], context)
            .await
            .expect("upload_build_context must succeed when present");

        // wiremock asserts the expect(0) automatically on server drop.
    }

    // ── upload_build_context — force_upload overrides present ──────────────────

    /// When the cache reports `present: true` but the instruction sets
    /// `force_upload`, the archive must STILL be uploaded — matching the JS
    /// `(forceUpload && url) || (!present && url)` condition.
    #[tokio::test]
    async fn upload_build_context_force_upload_uploads_even_when_present() {
        let server = MockServer::start().await;

        // Real context dir + file so the archive can be built.
        let dir = tempfile::tempdir().expect("tempdir");
        let context = dir.path();
        std::fs::write(context.join("app.js"), b"console.log('hi')").expect("write app.js");

        let files_hash = "deadbeef00112233".to_string();
        let get_url = format!("/templates/tpl_1/files/{files_hash}");
        let put_path = "/upload/forced";

        // present: true, but a url IS supplied — force_upload must override.
        Mock::given(method("GET"))
            .and(path(get_url))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "present": true,
                "url": format!("{}{put_path}", server.uri())
            })))
            .mount(&server)
            .await;

        // PUT mock with expect(1) — the forced upload MUST be issued.
        Mock::given(method("PUT"))
            .and(path(put_path))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let api = api_client_for(&server);
        let http = reqwest::Client::new();

        let instr = Instruction {
            instruction_type: InstructionType::Copy,
            args: vec![
                "app.js".to_string(),
                "/app/app.js".to_string(),
                String::new(),
                String::new(),
            ],
            force: false,
            force_upload: Some(true),
            files_hash: Some(files_hash),
            resolve_symlinks: false,
        };

        upload_build_context(&api, &http, "tpl_1", &[instr], context)
            .await
            .expect("upload_build_context must succeed with force_upload");

        // wiremock asserts the expect(1) automatically on server drop.
    }
}
