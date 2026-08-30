import { EMPTY_ROUTE_EVIDENCE, type RouteEvidence } from './types';

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

export function parseConversationRequest(rawBody: string): {
  turnId: string | null;
  conversationId: string | null;
  requestedModel: string | null;
  parentMessageId: string | null;
} {
  const root = asRecord(safeJsonParse(rawBody));
  if (!root) {
    return {
      turnId: null,
      conversationId: null,
      requestedModel: null,
      parentMessageId: null,
    };
  }

  const messages = Array.isArray(root.messages) ? root.messages : [];
  const firstMsg = asRecord(messages[0]);
  const turnId = asString(firstMsg?.id);
  const conversationId = asString(root.conversation_id);
  const requestedModel = asString(root.model);
  const parentMessageId = asString(root.parent_message_id);

  return {
    turnId,
    conversationId,
    requestedModel,
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
    if (typeof rec.id === 'string' && !acc.turnId) {
      // 记录助手消息 ID
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
  if (!slug) return '未知';
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
