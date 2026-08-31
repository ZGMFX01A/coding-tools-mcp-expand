export interface ObserverStatusCheck {
  ok: boolean;
  workspaceId?: string;
  warningAfterMs?: number;
  hardStopAfterMs?: number;
  error?: string;
}

/** 接受完整 MCP 地址或基准地址，统一成用于内部 Observer API 的基准地址。 */
export function normalizeObserverBaseUrl(raw: string): string {
  const trimmed = raw.trim().replace(/\/+$/, '');
  if (!trimmed) return '';

  try {
    const url = new URL(trimmed);
    const pathname = url.pathname.replace(/\/+$/, '');
    if (pathname.toLowerCase().endsWith('/mcp')) {
      url.pathname = pathname.slice(0, -4) || '/';
    }
    return url.toString().replace(/\/+$/, '');
  } catch {
    // 配置输入可能尚未完整，保留原值但仍修正最常见的完整 /mcp 地址。
    return trimmed.replace(/\/mcp$/i, '');
  }
}

export function validateObserverStatusPayload(value: unknown): ObserverStatusCheck {
  if (!value || typeof value !== 'object') {
    return { ok: false, error: '状态响应不是 JSON 对象' };
  }

  const payload = value as Record<string, unknown>;
  if (payload.ok !== true) {
    return { ok: false, error: '状态响应 ok 字段不是 true' };
  }
  if (payload.service !== 'chatgpt_turn_observer') {
    return { ok: false, error: '服务标识不匹配' };
  }
  if (typeof payload.workspace_id !== 'string' || !payload.workspace_id.trim()) {
    return { ok: false, error: '状态响应缺少 workspace_id' };
  }

  const budget = payload.turn_budget;
  const warningSeconds = budget && typeof budget === 'object'
    ? (budget as Record<string, unknown>).warning_after_seconds
    : undefined;
  const hardStopSeconds = budget && typeof budget === 'object'
    ? (budget as Record<string, unknown>).hard_stop_after_seconds
    : undefined;

  return {
    ok: true,
    workspaceId: payload.workspace_id,
    warningAfterMs: typeof warningSeconds === 'number' && Number.isFinite(warningSeconds) && warningSeconds > 0
      ? warningSeconds * 1000
      : undefined,
    hardStopAfterMs: typeof hardStopSeconds === 'number' && Number.isFinite(hardStopSeconds) && hardStopSeconds > 0
      ? hardStopSeconds * 1000
      : undefined,
  };
}
