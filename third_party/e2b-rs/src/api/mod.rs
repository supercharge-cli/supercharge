//! Control-plane REST API client and generated schema types. The `ApiClient`
//! and per-endpoint calls land in Plan 2b.

pub(crate) mod client;
pub(crate) mod schema;

#[cfg(test)]
mod tests {
    use super::schema as api_gen;

    #[test]
    fn control_plane_error_round_trips() {
        // The control-plane Error uses an integer `code` (distinct from the
        // volume content Error which uses a string code).
        let json = r#"{"code": 404, "message": "sandbox not found"}"#;
        let err: api_gen::Error = serde_json::from_str(json).expect("deserialize Error");
        assert_eq!(err.code, 404);
        assert_eq!(err.message, "sandbox not found");
    }
}
