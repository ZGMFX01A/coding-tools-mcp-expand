use serde_json::{json, Value};

pub const PROTOCOL_VERSION_2024_11_05: &str = "2024-11-05";
pub const PROTOCOL_VERSION_2025_06_18: &str = "2025-06-18";
pub const PROTOCOL_VERSION_2025_11_25: &str = "2025-11-25";
pub const PROTOCOL_VERSION_2026_07_28: &str = "2026-07-28";

pub const LEGACY_PROTOCOL_VERSIONS: &[&str] = &[
    PROTOCOL_VERSION_2025_11_25,
    PROTOCOL_VERSION_2025_06_18,
    PROTOCOL_VERSION_2024_11_05,
];

pub const LATEST_LEGACY_PROTOCOL_VERSION: &str = PROTOCOL_VERSION_2025_11_25;
pub const MODERN_PROTOCOL_VERSIONS: &[&str] = &[PROTOCOL_VERSION_2026_07_28];

pub const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
pub const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolEra {
    Legacy,
    Modern,
}

#[derive(Debug, Clone)]
pub struct RequestProtocolContext {
    pub era: ProtocolEra,
    pub protocol_version: String,
}

pub fn negotiate_legacy_version(requested: Option<&str>) -> String {
    match requested {
        Some(v) if LEGACY_PROTOCOL_VERSIONS.contains(&v) => v.to_string(),
        _ => LATEST_LEGACY_PROTOCOL_VERSION.to_string(),
    }
}

pub fn detect_request_context(method: &str, params: &Value) -> RequestProtocolContext {
    if method == "server/discover" {
        return RequestProtocolContext {
            era: ProtocolEra::Modern,
            protocol_version: PROTOCOL_VERSION_2026_07_28.to_string(),
        };
    }
    if let Some(meta_version) = params
        .get("_meta")
        .and_then(|m| m.get(META_PROTOCOL_VERSION))
        .and_then(Value::as_str)
    {
        if MODERN_PROTOCOL_VERSIONS.contains(&meta_version) {
            return RequestProtocolContext {
                era: ProtocolEra::Modern,
                protocol_version: meta_version.to_string(),
            };
        }
    }
    RequestProtocolContext {
        era: ProtocolEra::Legacy,
        protocol_version: LATEST_LEGACY_PROTOCOL_VERSION.to_string(),
    }
}

pub fn shape_result(ctx: &RequestProtocolContext, mut result: Value) -> Value {
    if ctx.era == ProtocolEra::Modern {
        if let Some(obj) = result.as_object_mut() {
            obj.insert("resultType".into(), Value::String("complete".into()));
            let mut meta = obj
                .get("_meta")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            meta.insert(
                META_SERVER_INFO.into(),
                json!({
                    "name": "coding-tools-mcp",
                    "title": "Coding Tools MCP",
                    "version": env!("CARGO_PKG_VERSION")
                }),
            );
            obj.insert("_meta".into(), Value::Object(meta));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_negotiate_legacy_version() {
        assert_eq!(
            negotiate_legacy_version(Some("2025-11-25")),
            "2025-11-25"
        );
        assert_eq!(
            negotiate_legacy_version(Some("2025-06-18")),
            "2025-06-18"
        );
        assert_eq!(
            negotiate_legacy_version(Some("2024-11-05")),
            "2024-11-05"
        );
        // 未知版本降级为最新 legacy 版本
        assert_eq!(
            negotiate_legacy_version(Some("unknown-version")),
            "2025-11-25"
        );
        assert_eq!(negotiate_legacy_version(None), "2025-11-25");
    }

    #[test]
    fn test_detect_request_context() {
        let legacy_params = json!({});
        let legacy_ctx = detect_request_context("tools/list", &legacy_params);
        assert_eq!(legacy_ctx.era, ProtocolEra::Legacy);

        let modern_params = json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28"
            }
        });
        let modern_ctx = detect_request_context("tools/list", &modern_params);
        assert_eq!(modern_ctx.era, ProtocolEra::Modern);
        assert_eq!(modern_ctx.protocol_version, "2026-07-28");

        let discover_ctx = detect_request_context("server/discover", &json!({}));
        assert_eq!(discover_ctx.era, ProtocolEra::Modern);
    }
}
