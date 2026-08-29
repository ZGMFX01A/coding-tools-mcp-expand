mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use coding_tools_mcp_desktop_lib::mcp::turn_budget::{
    AgentTurnBudgetConfig, AgentTurnBudgetManager,
};
use coding_tools_mcp_desktop_lib::mcp::handle_request;
use coding_tools_mcp_desktop_lib::tools::ToolContext;
use serde_json::json;

#[test]
fn test_live_turn_budget_60s_acceptance() {
    println!("\n=== [ACCEPTANCE] 启动 60 秒轻量端到端真实时钟验收测试 ===");

    // 1. 配置测试阈值
    let config = AgentTurnBudgetConfig {
        enabled: true,
        warning_after: Duration::from_secs(20),
        wrap_up_after: Duration::from_secs(35),
        finalization_after: Duration::from_secs(45),
        hard_stop_after: Duration::from_secs(60),
        deadline_reserve: Duration::from_secs(5), // 55s dispatch cutoff
        early_idle_reset: Duration::from_secs(10),
        mid_idle_reset: Duration::from_secs(15),
        platform_turn_limit: Duration::from_secs(65),
        post_limit_safety_margin: Duration::from_secs(5), // 70s 后恢复
        state_ttl: Duration::from_secs(300),
        max_states: 128,
    };

    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let harness = tempfile::tempdir().expect("harness tempdir");
    // 使用真实的 SystemBudgetClock
    let manager = Arc::new(AgentTurnBudgetManager::new(config));
    let ctx = ToolContext::for_test(workspace.path().to_path_buf(), harness.path().to_path_buf())
        .expect("tool context")
        .with_turn_budget_manager(manager.clone());
    let shared_ctx = Arc::new(ctx);

    let session_meta = json!({
        "openai/session": "chatgpt-live-acceptance-session-001"
    });

    let t0 = Instant::now();
    println!("[0.0s] 发送首个 tools/call 请求 (调用 read_file)...");

    // ==========================================
    // 阶段 1: 0s - Normal
    // ==========================================
    let req0 = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "get_default_cwd",
            "arguments": {},
            "_meta": session_meta
        }
    });
    let res0 = handle_request(&shared_ctx, &req0);
    assert_eq!(res0["id"], 1);
    assert_eq!(res0["result"]["isError"], false);
    let meta0 = &res0["result"]["_meta"]["coding-tools/agentTurnBudget"];
    assert_eq!(meta0["status"], "normal");
    assert_eq!(meta0["shouldWrapUp"], false);
    assert_eq!(meta0["shouldStopToolCalls"], false);
    println!("  -> 阶段 1 (Normal) 验证通过: status=normal, isError=false");

    // ==========================================
    // 阶段 2: 21s+ - Warning
    // ==========================================
    println!("[等待真实时间到达 21s 触发 Warning 阶段...]");
    let elapsed_so_far = t0.elapsed();
    if elapsed_so_far < Duration::from_secs(21) {
        std::thread::sleep(Duration::from_secs(21) - elapsed_so_far);
    }
    println!("[{:.1}s] 发送 Warning 阶段 tools/call 请求...", t0.elapsed().as_secs_f64());

    let req_warn = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "get_default_cwd",
            "arguments": {},
            "_meta": session_meta
        }
    });
    let res_warn = handle_request(&shared_ctx, &req_warn);
    assert_eq!(res_warn["id"], 2);
    assert_eq!(res_warn["result"]["isError"], false);
    let meta_warn = &res_warn["result"]["_meta"]["coding-tools/agentTurnBudget"];
    assert_eq!(meta_warn["status"], "warning");
    assert_eq!(meta_warn["shouldWrapUp"], true);
    assert_eq!(meta_warn["shouldStopToolCalls"], false);
    let text_warn = res_warn["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text_warn.contains("[TURN BUDGET WARNING]"), "必须置顶包含 WARNING 文本");
    println!("  -> 阶段 2 (Warning) 验证通过: 成功注入 [TURN BUDGET WARNING] 且 status=warning");

    // ==========================================
    // 阶段 3: 36s+ - WrapUp (27m 对应)
    // ==========================================
    println!("[等待真实时间到达 36s 触发 WrapUp 阶段...]");
    let elapsed_so_far = t0.elapsed();
    if elapsed_so_far < Duration::from_secs(36) {
        std::thread::sleep(Duration::from_secs(36) - elapsed_so_far);
    }
    println!("[{:.1}s] 发送 WrapUp 阶段 search_text (应被拒绝)...", t0.elapsed().as_secs_f64());

    let req_wrap = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "search_text",
            "arguments": { "query": "test" },
            "_meta": session_meta
        }
    });
    let res_wrap = handle_request(&shared_ctx, &req_wrap);
    assert_eq!(res_wrap["id"], 3);
    assert_eq!(res_wrap["result"]["isError"], true);
    assert_eq!(
        res_wrap["result"]["structuredContent"]["error"]["code"],
        "AGENT_TURN_WRAP_UP_RESTRICTED"
    );
    assert_eq!(
        res_wrap["result"]["structuredContent"]["error"]["category"],
        "turn_budget"
    );
    assert_eq!(
        res_wrap["result"]["structuredContent"]["error"]["retryable"],
        false
    );
    println!("  -> 阶段 3 (WrapUp) 验证通过: 成功拦截 investigation 工具 search_text");

    // ==========================================
    // 阶段 4: 46s+ - Finalization (28m 对应)
    // ==========================================
    println!("[等待真实时间到达 46s 触发 Finalization 阶段...]");
    let elapsed_so_far = t0.elapsed();
    if elapsed_so_far < Duration::from_secs(46) {
        std::thread::sleep(Duration::from_secs(46) - elapsed_so_far);
    }
    println!("[{:.1}s] 发送 Finalization 阶段 get_default_cwd (允许)...", t0.elapsed().as_secs_f64());

    let req_fin = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "get_default_cwd",
            "arguments": {},
            "_meta": session_meta
        }
    });
    let res_fin = handle_request(&shared_ctx, &req_fin);
    assert_eq!(res_fin["id"], 4);
    assert_eq!(res_fin["result"]["isError"], false);
    let meta_fin = &res_fin["result"]["_meta"]["coding-tools/agentTurnBudget"];
    assert_eq!(meta_fin["status"], "finalization");
    println!("  -> 阶段 4 (Finalization) 验证通过: 允许收尾工具执行");

    // ==========================================
    // 阶段 5: 56s+ - Dispatch Cutoff (55s ~ 60s)
    // ==========================================
    println!("[等待真实时间到达 56s 触发 Dispatch Cutoff 阶段...]");
    let elapsed_so_far = t0.elapsed();
    if elapsed_so_far < Duration::from_secs(56) {
        std::thread::sleep(Duration::from_secs(56) - elapsed_so_far);
    }
    println!("[{:.1}s] 发送 Dispatch Cutoff 阶段 tools/call 请求...", t0.elapsed().as_secs_f64());

    let req_cutoff = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "get_default_cwd",
            "arguments": {},
            "_meta": session_meta
        }
    });
    let res_cutoff = handle_request(&shared_ctx, &req_cutoff);
    assert_eq!(res_cutoff["id"], 5);
    assert_eq!(res_cutoff["result"]["isError"], true);
    assert_eq!(
        res_cutoff["result"]["structuredContent"]["error"]["code"],
        "AGENT_TURN_BUDGET_DISPATCH_CUTOFF"
    );
    println!("  -> 阶段 5 (Dispatch Cutoff) 验证通过: 新工具被正确阻断");

    // ==========================================
    // 阶段 6: 61s+ - Hard Stop (>= 60s)
    // ==========================================
    println!("[等待真实时间到达 61s 触发 Hard Stop 阶段...]");
    let elapsed_so_far = t0.elapsed();
    if elapsed_so_far < Duration::from_secs(61) {
        std::thread::sleep(Duration::from_secs(61) - elapsed_so_far);
    }
    println!("[{:.1}s] 发送 Hard Stop 阶段 tools/call 请求...", t0.elapsed().as_secs_f64());

    let req_hs = json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {
            "name": "get_default_cwd",
            "arguments": {},
            "_meta": session_meta
        }
    });
    let res_hs = handle_request(&shared_ctx, &req_hs);
    assert_eq!(res_hs["id"], 6);
    assert_eq!(res_hs["result"]["isError"], true);
    assert_eq!(
        res_hs["result"]["structuredContent"]["error"]["code"],
        "AGENT_TURN_BUDGET_EXHAUSTED"
    );
    assert_eq!(
        res_hs["result"]["structuredContent"]["error"]["retryable"],
        false
    );
    assert_eq!(
        res_hs["result"]["structuredContent"]["error"]["category"],
        "turn_budget"
    );
    println!("  -> 阶段 6 (Hard Stop) 验证通过: 工具被短路拒绝 (retryable=false, category=turn_budget)");

    // ==========================================
    // 阶段 7: 71s+ - 跨越 65s + 5s 平台隔离线自动恢复新 Turn
    // ==========================================
    println!("[等待真实时间到达 71s 跨越平台隔离线...]");
    let elapsed_so_far = t0.elapsed();
    if elapsed_so_far < Duration::from_secs(71) {
        std::thread::sleep(Duration::from_secs(71) - elapsed_so_far);
    }
    println!("[{:.1}s] 发送隔离期过后 tools/call 请求...", t0.elapsed().as_secs_f64());

    let req_recovery = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": "get_default_cwd",
            "arguments": {},
            "_meta": session_meta
        }
    });
    let res_recovery = handle_request(&shared_ctx, &req_recovery);
    assert_eq!(res_recovery["id"], 7);
    assert_eq!(res_recovery["result"]["isError"], false);
    let meta_recovery = &res_recovery["result"]["_meta"]["coding-tools/agentTurnBudget"];
    assert_eq!(meta_recovery["status"], "normal");
    assert_eq!(meta_recovery["elapsedSeconds"], 0);
    println!("  -> 阶段 7 (平台隔离线恢复) 验证通过: 自动重置为新 Turn 并恢复正常执行");

    println!("\n=== [ACCEPTANCE] 60 秒轻量端到端真实时钟验收测试全部 100% 通过 ===\n");
}
