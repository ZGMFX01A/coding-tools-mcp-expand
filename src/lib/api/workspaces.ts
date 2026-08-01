import { invoke } from "@tauri-apps/api/core";
import type { RuntimeStatus, WorkspaceProfile } from "$lib/types";

export async function listWorkspaces(): Promise<WorkspaceProfile[]> {
  return invoke<WorkspaceProfile[]>("list_workspaces");
}

export async function createWorkspace(
  path: string,
  name?: string,
): Promise<WorkspaceProfile> {
  return invoke<WorkspaceProfile>("create_workspace", { path, name });
}

export async function updateWorkspace(profile: WorkspaceProfile): Promise<void> {
  return invoke("update_workspace", { profile });
}

/** 修改工作区 ID，返回迁移后的 profile（调用方需先停止 MCP 与 Actions 服务） */
export async function updateWorkspaceId(
  currentId: string,
  newId: string,
): Promise<WorkspaceProfile> {
  return invoke<WorkspaceProfile>("update_workspace_id", { currentId, newId });
}

export async function openWorkspaceDirectory(path: string): Promise<void> {
  return invoke("open_workspace_directory", { path });
}

export async function deleteWorkspace(id: string): Promise<void> {
  return invoke("delete_workspace", { id });
}

export async function startRuntime(id: string): Promise<RuntimeStatus> {
  return invoke<RuntimeStatus>("start_runtime", { id });
}

export async function stopRuntime(id: string): Promise<RuntimeStatus> {
  return invoke<RuntimeStatus>("stop_runtime", { id });
}

export async function getRuntimeStatus(id: string): Promise<RuntimeStatus> {
  return invoke<RuntimeStatus>("get_runtime_status", { id });
}

export async function startActionsRuntime(id: string): Promise<RuntimeStatus> {
  return invoke<RuntimeStatus>("start_actions_runtime", { id });
}

export async function stopActionsRuntime(id: string): Promise<RuntimeStatus> {
  return invoke<RuntimeStatus>("stop_actions_runtime", { id });
}

export async function getActionsRuntimeStatus(id: string): Promise<RuntimeStatus> {
  return invoke<RuntimeStatus>("get_actions_runtime_status", { id });
}

export async function restartRuntime(id: string): Promise<RuntimeStatus> {
  return invoke<RuntimeStatus>("restart_runtime", { id });
}

export async function restartActionsRuntime(id: string): Promise<RuntimeStatus> {
  return invoke<RuntimeStatus>("restart_actions_runtime", { id });
}
