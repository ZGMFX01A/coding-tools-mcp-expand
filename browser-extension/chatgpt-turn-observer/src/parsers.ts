import {
  EMPTY_ROUTE_EVIDENCE,
  type RouteEvidence,
  type TurnStartDecision,
} from './types';

const STANDARD_CONVERSATION_PATH = /^\/c\/([^/?#]+)(?=[/?#]|$)/;
const GIZMO_CONVERSATION_PATH = /^\/g\/[^/?#]+\/c\/([^/?#]+)(?=[/?#]|$)/;

export function conversationIdFromUrl(urlOrPathname: string): string | null {
  try {
    let pathname = urlOrPathname;
    if (urlOrPathname.startsWith('http://') || urlOrPathname.startsWith('https://')) {
      pathname = new URL(urlOrPathname).pathname;
    }
    const match =
      STANDARD_CONVERSATION_PATH.exec(pathname) ||
      GIZMO_CONVERSATION_PATH.exec(pathname);
    if (match && match[1]) {
      return decodeURIComponent(match[1]) || null;
    }
  } catch {
    // URL 解析异常兜底返回 null
  }
  return null;
}

export function isConversationEndpoint(urlStr: string): boolean {
  if (!urlStr) return false;
  try {
    const parsed = new URL(urlStr, typeof location !== 'undefined' ? location.origin : 'https://chatgpt.com');
    const p = parsed.pathname.toLowerCase();

    // 排除明确的非对话交互端点（遥测、文件上传、语音合成等）
    if (
      p.includes('/files') ||
      p.includes('/synthesize') ||
      p.includes('/telemetry') ||
      p.includes('/analytics') ||
      p.includes('/ces/') ||
      p.includes('/settings') ||
      p.includes('/models')
    ) {
      return false;
    }

    return (
      p.includes('/backend-api/conversation') ||
      p.includes('/backend-api/conversations') ||
      p.includes('/backend-api/lat/r') ||
      p.includes('/backend-api/f/r') ||
      p.includes('/backend-api/f/conversation') ||
      p.includes('/backend-anon/conversation') ||
      p.includes('/backend-anon/conversations')
    );
  } catch {
    const lower = urlStr.toLowerCase();
    if (
      lower.includes('/files') ||
      lower.includes('/synthesize') ||
      lower.includes('/telemetry') ||
      lower.includes('/ces/')
    ) {
      return false;
    }
    return (
      lower.includes('/backend-api/conversation') ||
      lower.includes('/backend-api/lat/r') ||
      lower.includes('/backend-api/f/r') ||
      lower.includes('/backend-anon/conversation')
    );
  }
}

export function generateUuid(): string {
  if (typeof crypto !== 'undefined' && crypto.randomUUID) {
    return crypto.randomUUID();
  }
  return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === 'x' ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}

function safeJsonParse(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

function asRecord(val: unknown): Record<string, unknown> | null {
  return val !== null && typeof val === 'object' && !Array.isArray(val)
    ? (val as Record<string, unknown>)
    : null;
}

function asString(val: unknown): string | null {
  return typeof val === 'string' && val.trim().length > 0 ? val.trim() : null;
}

/**
 * 扫描 messages 数组，找到明确 role === 'user' 且带有有效 id 的最新用户消息
 */
export function findNewestUserMessage(messages: unknown): { id: string } | null {
  if (!Array.isArray(messages)) return null;
  for (let i = messages.length - 1; i >= 0; i--) {
    const msg = asRecord(messages[i]);
    if (!msg) continue;
    const author = asRecord(msg.author);
    const role = asString(author?.role) || asString(msg.role);
    if (role === 'user') {
      const id = asString(msg.id);
      if (id) {
        return { id };
      }
    }
  }
  return null;
}

/**
 * 严格消息语义判定：仅当检测到明确的 user-authored message 时才允许 NEW_USER_TURN。
 * 杜绝图片上传预请求、复制文本遥测请求、后台轮询等造成的 False Positive。
 */
export function classifyUserTurnStart(
  urlStr: string,
  method: string,
  rawBody: string | Record<string, unknown> | null,
  currentTurnId: string | null
): TurnStartDecision {
  const upperMethod = (method || 'GET').toUpperCase();
  if (upperMethod !== 'POST') {
    return { type: 'NON_TURN_REQUEST', reason: 'UNRELATED_ENDPOINT' };
  }

  // 1. 拦截明确的遥测/上传/非对话接口
  const lowerUrl = urlStr.toLowerCase();
  if (lowerUrl.includes('/files') || lowerUrl.includes('/attachment') || lowerUrl.includes('/upload')) {
    return { type: 'NON_TURN_REQUEST', reason: 'UPLOAD_ONLY' };
  }
  if (lowerUrl.includes('/ces/') || lowerUrl.includes('/telemetry') || lowerUrl.includes('/analytics')) {
    return { type: 'NON_TURN_REQUEST', reason: 'COPY_TELEMETRY' };
  }

  if (!isConversationEndpoint(urlStr)) {
    return { type: 'NON_TURN_REQUEST', reason: 'UNRELATED_ENDPOINT' };
  }

  // 2. 检查 Request Body 结构
  if (!rawBody) {
    return { type: 'NON_TURN_REQUEST', reason: 'INVALID_STRUCTURE' };
  }

  const root = typeof rawBody === 'string' ? asRecord(safeJsonParse(rawBody)) : asRecord(rawBody);
  if (!root) {
    return { type: 'NON_TURN_REQUEST', reason: 'INVALID_STRUCTURE' };
  }

  // 3. 严格扫描 user-authored message
  const userMsg = findNewestUserMessage(root.messages);
  let userMessageId: string | null = userMsg?.id ?? null;

  // 兼容根字段上的 client_message_id / message_id（仅当 action 为 next/create 时）
  if (!userMessageId && (root.action === 'next' || root.action === 'create' || !root.action)) {
    userMessageId =
      asString(root.client_message_id) ||
      asString(root.message_id) ||
      asString(root.user_message_id);
  }

  if (!userMessageId) {
    return { type: 'NON_TURN_REQUEST', reason: 'NO_USER_MESSAGE' };
  }

  const conversationId = asString(root.conversation_id);
  const requestedModel = asString(root.model);
  const parentMessageId = asString(root.parent_message_id);

  // 4. 比对当前 TurnId
  if (currentTurnId && userMessageId === currentTurnId) {
    return {
      type: 'SAME_TURN_CONTINUATION',
      userMessageId,
      conversationId,
    };
  }

  return {
    type: 'NEW_USER_TURN',
    userMessageId,
    requestedModel,
    conversationId,
    parentMessageId,
  };
}

export function mergeEvidence(
  base: RouteEvidence,
  incoming: Partial<RouteEvidence>
): RouteEvidence {
  return {
    requestedModel: incoming.requestedModel ?? base.requestedModel,
    resolvedModelSlug: incoming.resolvedModelSlug ?? base.resolvedModelSlug,
    serverModelSlug: incoming.serverModelSlug ?? base.serverModelSlug,
    responseModelSlug: incoming.responseModelSlug ?? base.responseModelSlug,
    conversationId: incoming.conversationId ?? base.conversationId,
    requestId: incoming.requestId ?? base.requestId,
    turnId: incoming.turnId ?? base.turnId,
    captureId: incoming.captureId ?? base.captureId,
    isStreamDone: incoming.isStreamDone ?? base.isStreamDone,
  };
}

function walkRecordForModelEvidence(
  value: unknown,
  evidence: RouteEvidence,
  depth = 0
): RouteEvidence {
  if (depth > 8 || !value || typeof value !== 'object') return evidence;

  if (Array.isArray(value)) {
    let acc = evidence;
    for (const item of value.slice(0, 32)) {
      acc = walkRecordForModelEvidence(item, acc, depth + 1);
    }
    return acc;
  }

  const rec = asRecord(value);
  if (!rec) return evidence;

  let acc = evidence;

  if (typeof rec.conversation_id === 'string' && !acc.conversationId) {
    acc = mergeEvidence(acc, { conversationId: asString(rec.conversation_id) });
  }

  const metadata = asRecord(rec.metadata);
  if (metadata) {
    if (typeof metadata.resolved_model_slug === 'string') {
      acc = mergeEvidence(acc, { resolvedModelSlug: asString(metadata.resolved_model_slug) });
    }
    if (typeof metadata.request_id === 'string') {
      acc = mergeEvidence(acc, { requestId: asString(metadata.request_id) });
    }
    if (typeof metadata.conversation_id === 'string') {
      acc = mergeEvidence(acc, { conversationId: asString(metadata.conversation_id) });
    }

    const serverSte = asRecord(metadata.server_ste_metadata);
    if (serverSte && typeof serverSte.model_slug === 'string') {
      acc = mergeEvidence(acc, { serverModelSlug: asString(serverSte.model_slug) });
    }
  }

  if (rec.type === 'server_ste_metadata') {
    if (metadata && typeof metadata.model_slug === 'string') {
      acc = mergeEvidence(acc, { serverModelSlug: asString(metadata.model_slug) });
    }
  }

  const author = asRecord(rec.author);
  if (author && author.role === 'assistant') {
    if (metadata && typeof metadata.model_slug === 'string') {
      acc = mergeEvidence(acc, { responseModelSlug: asString(metadata.model_slug) });
    }
  }

  for (const [k, nested] of Object.entries(rec)) {
    if (k === 'server_ste_metadata' || (rec.type === 'server_ste_metadata' && k === 'metadata')) {
      continue;
    }
    acc = walkRecordForModelEvidence(nested, acc, depth + 1);
  }

  return acc;
}

export function parseSseChunk(chunk: string): RouteEvidence {
  let evidence: RouteEvidence = { ...EMPTY_ROUTE_EVIDENCE };

  const lines = chunk.split(/\r?\n/);
  for (const line of lines) {
    if (!line.startsWith('data:')) continue;
    const payloadStr = line.slice(5).trim();
    if (!payloadStr) continue;

    if (payloadStr === '[DONE]') {
      evidence = mergeEvidence(evidence, { isStreamDone: true });
      continue;
    }

    const parsed = safeJsonParse(payloadStr);
    if (parsed) {
      evidence = walkRecordForModelEvidence(parsed, evidence);
    }
  }

  return evidence;
}

/**
 * 流式 SSE 解析器，支持跨分块残片累积，防止 JSON 被拆成多个 reader.read() 导致解析失败
 */
export class SseStreamParser {
  private buffer = '';

  public feed(chunk: string): RouteEvidence {
    this.buffer += chunk;
    let evidence: RouteEvidence = { ...EMPTY_ROUTE_EVIDENCE };

    const lines = this.buffer.split(/\r?\n/);
    // 最后一行可能是不完整行，保留在 buffer 中
    this.buffer = lines.pop() ?? '';

    for (const line of lines) {
      if (!line.startsWith('data:')) continue;
      const payloadStr = line.slice(5).trim();
      if (!payloadStr) continue;

      if (payloadStr === '[DONE]') {
        evidence = mergeEvidence(evidence, { isStreamDone: true });
        continue;
      }

      const parsed = safeJsonParse(payloadStr);
      if (parsed) {
        evidence = walkRecordForModelEvidence(parsed, evidence);
      }
    }

    return evidence;
  }

  public flush(): RouteEvidence {
    let evidence: RouteEvidence = { ...EMPTY_ROUTE_EVIDENCE };
    const remaining = this.buffer.trim();
    if (remaining.startsWith('data:')) {
      const payloadStr = remaining.slice(5).trim();
      if (payloadStr === '[DONE]') {
        evidence = mergeEvidence(evidence, { isStreamDone: true });
      } else if (payloadStr) {
        const parsed = safeJsonParse(payloadStr);
        if (parsed) {
          evidence = walkRecordForModelEvidence(parsed, evidence);
        }
      }
    }
    this.buffer = '';
    return evidence;
  }
}

export function parseWebSocketFrame(raw: string): RouteEvidence {
  let evidence: RouteEvidence = { ...EMPTY_ROUTE_EVIDENCE };
  if (!raw || raw.length > 2 * 1024 * 1024) return evidence;

  const parsed = safeJsonParse(raw);
  if (!Array.isArray(parsed)) return evidence;

  for (const envelope of parsed.slice(0, 16)) {
    const envRec = asRecord(envelope);
    const outerPayload = asRecord(envRec?.payload);
    const innerPayload = asRecord(outerPayload?.payload);
    const encodedItem = asString(innerPayload?.encoded_item);
    if (encodedItem) {
      const itemEvidence = parseSseChunk(encodedItem);
      evidence = mergeEvidence(evidence, itemEvidence);
    }
  }

  return evidence;
}

/**
 * 严格判定实际模型：
 * 只有当响应侧存在明确证据（resolvedModelSlug || serverModelSlug || responseModelSlug）时才返回。
 * 绝不能使用 requestedModel 冒充！
 */
export function extractActualModel(evidence: RouteEvidence): string | null {
  if (evidence.resolvedModelSlug) return evidence.resolvedModelSlug;
  if (evidence.serverModelSlug) return evidence.serverModelSlug;
  if (evidence.responseModelSlug) return evidence.responseModelSlug;
  return null;
}

/**
 * 将模型 slug 格式化为美观的展示名称
 */
export function formatModelDisplayName(slug: string | null): string {
  if (!slug) return '—';
  const lower = slug.toLowerCase();
  if (lower.includes('gpt-5.6') || lower.includes('sol')) return 'GPT-5.6 Sol';
  if (lower.includes('o3-mini')) return 'o3-mini';
  if (lower.includes('o3')) return 'o3';
  if (lower.includes('o1-mini')) return 'o1-mini';
  if (lower.includes('o1-preview')) return 'o1-preview';
  if (lower.includes('o1')) return 'o1';
  if (lower.includes('gpt-4o-mini')) return 'GPT-4o mini';
  if (lower.includes('gpt-4o')) return 'GPT-4o';
  if (lower.includes('gpt-4')) return 'GPT-4';
  return slug;
}
