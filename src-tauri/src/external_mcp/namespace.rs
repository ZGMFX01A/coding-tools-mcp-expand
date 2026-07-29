/// 将外部 MCP 服务名称规范化为合法的命名空间前缀
pub fn normalize_server_name(name: &str, fallback_id: &str) -> String {
    let lower = name.trim().to_lowercase();
    let mut normalized = String::new();
    let mut last_was_sep = false;

    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            normalized.push(ch);
            last_was_sep = false;
        } else if ch == ' ' || ch == '.' || ch == '/' || ch == '\\' {
            if !last_was_sep {
                normalized.push('-');
                last_was_sep = true;
            }
        } else {
            if !last_was_sep {
                normalized.push('-');
                last_was_sep = true;
            }
        }
    }

    let trimmed = normalized.trim_matches(|c| c == '-' || c == '_').to_string();
    if trimmed.is_empty() {
        normalize_server_name(fallback_id, "mcp")
    } else {
        trimmed
    }
}

/// 生成对外暴露的公网工具名
pub fn make_public_tool_name(server_name: &str, fallback_id: &str, original_tool_name: &str) -> String {
    let norm = normalize_server_name(server_name, fallback_id);
    format!("{norm}__{original_tool_name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_server_name() {
        assert_eq!(normalize_server_name("fast-context", "id1"), "fast-context");
        assert_eq!(normalize_server_name("Fast Context 1.0", "id1"), "fast-context-1-0");
        assert_eq!(normalize_server_name("My_Tool@Server!", "id1"), "my_tool-server");
        assert_eq!(normalize_server_name("  ", "fallback-id"), "fallback-id");
    }

    #[test]
    fn test_make_public_tool_name() {
        assert_eq!(
            make_public_tool_name("fast-context", "id1", "fast_context_search"),
            "fast-context__fast_context_search"
        );
        assert_eq!(
            make_public_tool_name("fast-context", "id1", "extract_windsurf_key"),
            "fast-context__extract_windsurf_key"
        );
    }
}
