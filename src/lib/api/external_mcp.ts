import { invoke } from "@tauri-apps/api/core";
import type {
  ExternalMcpConfig,
  ExternalMcpStatus,
  FastContextDetectionResult,
  McpTool,
  TestConnectionResult,
} from "$lib/types";

export async function detectFastContextEnv(): Promise<FastContextDetectionResult> {
  return invoke<FastContextDetectionResult>("detect_fast_context_env");
}

export async function listExternalMcps(workspaceId: string): Promise<ExternalMcpStatus[]> {
  return invoke<ExternalMcpStatus[]>("list_external_mcps", { workspaceId });
}

export async function saveExternalMcp(
  workspaceId: string,
  config: ExternalMcpConfig,
): Promise<ExternalMcpStatus> {
  return invoke<ExternalMcpStatus>("save_external_mcp", { workspaceId, config });
}

export async function deleteExternalMcp(
  workspaceId: string,
  configId: string,
): Promise<void> {
  return invoke("delete_external_mcp", { workspaceId, configId });
}

export async function testExternalMcpConnection(
  workspaceId: string,
  config: ExternalMcpConfig,
): Promise<TestConnectionResult> {
  return invoke<TestConnectionResult>("test_external_mcp_connection", { workspaceId, config });
}

export async function reconnectExternalMcp(
  workspaceId: string,
  configId: string,
): Promise<ExternalMcpStatus> {
  return invoke<ExternalMcpStatus>("reconnect_external_mcp", { workspaceId, configId });
}

export async function getExternalMcpDiscoveredTools(
  workspaceId: string,
  configId: string,
): Promise<McpTool[]> {
  return invoke<McpTool[]>("get_external_mcp_discovered_tools", { workspaceId, configId });
}
