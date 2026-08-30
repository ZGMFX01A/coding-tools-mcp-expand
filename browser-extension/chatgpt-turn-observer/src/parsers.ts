import {
  EMPTY_ROUTE_EVIDENCE,
  type RouteEvidence,
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

export type EndpointKind = 'conversation_stream' | 'conversation_record' | 'other';

/**
 * 严格端点分类（参考 chatgpt-route-inspector 设计）：
 * 只有正向匹配到真实对话生成流端点时才判定为 conversation_stream。
 * 所有遥测、交互（如点击复制打点）、文件上传、设置等全部归为 other。
 */
export function classifyEndpoint(
  input: string,
  base = 'https://chatgpt.com/'
): { kind: EndpointKind; conversationId: string | null } {
  let url: URL;
  try {
    url = new URL(input, base);
  } catch {
    return { kind: 'other', conversationId: null };
  }

  const p = url.pathname;

  // 对话生成 SSE 流端点白名单
  if (
    /^\/backend-api\/(?:f\/)?conversations?$/.test(p) ||
    /^\/backend-anon\/(?:f\/)?conversations?$/.test(p)
  ) {
    return { kind: 'conversation_stream', conversationId: null };
  }

  // 历史对话记录端点
  const match = /^\/backend-api\/conversations?\/([^/]+)$/.exec(p);
  if (match?.[1]) {
    return { kind: 'conversation_record', conversationId: decodeURIComponent(match[1]) };
  }

  return { kind: 'other', conversationId: null };
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

export interface ConversationCorrelation {
  conversationId: string | null;
  inputMessageId: string | null;
  parentMessageId: string | null;
  requestedModel: string | null;
  action: string | null;
}

/**
 * 提取对话请求关联信息（严格要求用户消息语义）
 */
export function parseConversationCorrelation(
  raw: string | Record<string, unknown>
): ConversationCorrelation | null {
  const root = typeof raw === 'string' ? asRecord(safeJsonParse(raw)) : asRecord(raw);
  if (!root) return null;

  const messages = Array.isArray(root.messages) ? root.messages : [];
  let inputMessageId: string | null = null;

  // 倒序遍历 messages，寻找明确带有 role === 'user' 或 author.role === 'user' 的输入消息
  for (let i = messages.length - 1; i >= 0; i--) {
    const msg = asRecord(messages[i]);
    if (!msg) continue;
    const author = asRecord(msg.author);
    const role = asString(author?.role) || asString(msg.role);
    if (role === 'user') {
      inputMessageId = asString(msg.id);
      if (inputMessageId) break;
    }
  }

  // 备选：当根对象存在明确的 client_message_id / message_id 且 action 为生成类时
  if (!inputMessageId && (root.action === 'next' || root.action === 'create' || !root.action)) {
    inputMessageId = asString(root.client_message_id) || asString(root.message_id);
  }

  const conversationId = asString(root.conversation_id);
  const parentMessageId = asString(root.parent_message_id);
  const requestedModel = asString(root.model);
  const action = asString(root.action);

  // 如果连 inputMessageId 和 conversationId 都没有，不是合法的对话生成请求
  if (!inputMessageId && !conversationId) return null;

  return {
    conversationId,
    inputMessageId,
    parentMessageId,
    requestedModel,
    action,
  };
}

export interface WebSocketRouteEvidence {
  evidence: RouteEvidence;
  conversationIds: string[];
  messageIds: string[];
  parentIds: string[];
  terminal: boolean;
}

interface CorrelationAccumulator {
  conversationIds: string[];
  messageIds: string[];
  parentIds: string[];
  terminal: boolean;
  visited: number;
}

function collectCorrelation(
  value: unknown,
  result: CorrelationAccumulator,
  depth = 0
): void {
  if (depth > 8 || result.visited >= 500) return;
  result.visited += 1;
  if (Array.isArray(value)) {
    for (const item of value.slice(0, 32)) collectCorrelation(item, result, depth + 1);
    return;
  }
  const record = asRecord(value);
  if (!record) return;

  const convId = asString(record.conversation_id);
  if (convId && !result.conversationIds.includes(convId)) result.conversationIds.push(convId);

  const parentId = asString(record.parent_id) || asString(record.parent);
  if (parentId && !result.parentIds.includes(parentId)) result.parentIds.push(parentId);

  const author = asRecord(record.author);
  const msgId = asString(record.id);
  if (author && msgId && !result.messageIds.includes(msgId)) {
    result.messageIds.push(msgId);
  }
  const message = asRecord(record.message);
  const innerMsgId = asString(message?.id);
  if (innerMsgId && !result.messageIds.includes(innerMsgId)) {
    result.messageIds.push(innerMsgId);
  }

  if (record.type === 'server_ste_metadata') {
    result.terminal = true;
  }

  for (const nested of Object.values(record)) {
    if (nested && typeof nested === 'object') collectCorrelation(nested, result, depth + 1);
  }
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

export function parseWebSocketFrame(raw: string): WebSocketRouteEvidence[] {
  if (!raw || raw.length > 2 * 1024 * 1024) return [];

  const parsed = safeJsonParse(raw);
  if (!Array.isArray(parsed)) return [];

  const results: WebSocketRouteEvidence[] = [];

  for (const envelope of parsed.slice(0, 16)) {
    const envRec = asRecord(envelope);
    const outerPayload = asRecord(envRec?.payload);
    const innerPayload = asRecord(outerPayload?.payload);
    const encodedItem = asString(innerPayload?.encoded_item);
    if (!encodedItem || encodedItem.length > 1024 * 1024) continue;

    const correlation: CorrelationAccumulator = {
      conversationIds: [],
      messageIds: [],
      parentIds: [],
      terminal: false,
      visited: 0,
    };

    for (const line of encodedItem.split(/\r?\n/)) {
      if (!line.startsWith('data:')) continue;
      const payload = line.slice(5).trim();
      if (!payload) continue;
      if (payload === '[DONE]') {
        correlation.terminal = true;
        continue;
      }
      try {
        collectCorrelation(safeJsonParse(payload), correlation);
      } catch {
        // 忽略
      }
    }

    const itemEvidence = parseSseChunk(encodedItem);
    if (itemEvidence.conversationId && !correlation.conversationIds.includes(itemEvidence.conversationId)) {
      correlation.conversationIds.push(itemEvidence.conversationId);
    }

    results.push({
      evidence: itemEvidence,
      conversationIds: correlation.conversationIds,
      messageIds: correlation.messageIds,
      parentIds: correlation.parentIds,
      terminal: correlation.terminal,
    });
  }

  return results;
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
