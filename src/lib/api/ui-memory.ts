import { invoke } from "@tauri-apps/api/core";

export interface WebviewMemorySample {
  mainMb: number;
  webviewMb: number;
  webviewProcessCount: number;
  supported: boolean;
}

/** Sample UI process memory. Does not touch MCP / Actions / FRP. */
export async function getWebviewMemorySample(): Promise<WebviewMemorySample> {
  return invoke<WebviewMemorySample>("get_webview_memory_sample");
}

/**
 * Destroy and recreate the main WebView (replaces msedgewebview2 processes).
 * Does not stop MCP / Actions / FRP.
 */
export async function recreateUiWebview(): Promise<void> {
  return invoke("recreate_ui_webview");
}
