import {
  classifyEndpoint,
  conversationIdFromUrl,
  generateUuid,
  parseConversationCorrelation,
  parseWebSocketFrame,
  SseStreamParser,
  type ConversationCorrelation,
  type WebSocketRouteEvidence,
} from './parsers';
import { CT_OBSERVER_MESSAGE_SOURCE, type PageHookMessage } from './types';

(function initPageHook() {
  // 防止重复初始化
  if ((window as unknown as { __CT_PAGE_HOOK_INSTALLED__?: boolean }).__CT_PAGE_HOOK_INSTALLED__) {
    return;
  }
  (window as unknown as { __CT_PAGE_HOOK_INSTALLED__?: boolean }).__CT_PAGE_HOOK_INSTALLED__ = true;

  function debugLog(...args: unknown[]) {
    try {
      if ((window as unknown as { __CT_DEBUG__?: boolean }).__CT_DEBUG__) {
        console.log('[CT Observer]', ...args);
      }
    } catch {
      // 忽略日志异常
    }
  }

  debugLog('page-hook loaded');

  function safePost(type: PageHookMessage['type'], payload: PageHookMessage['payload']) {
    try {
      const msg: PageHookMessage = {
        source: CT_OBSERVER_MESSAGE_SOURCE,
        type,
        payload,
      };
      window.postMessage(msg, '*');
    } catch {
      // 忽略 postMessage 异常
    }
  }

  // 1. URL 监听（仅同步 conversationId，绝不创建/修改 Turn）
  let lastUrl = location.href;
  function checkUrlChange() {
    try {
      const currentUrl = location.href;
      if (currentUrl !== lastUrl) {
        lastUrl = currentUrl;
        const convId = conversationIdFromUrl(currentUrl);
        debugLog('URL_CHANGE', { hasConvId: Boolean(convId) });
        safePost('URL_CHANGE', {
          url: currentUrl,
          conversationId: convId,
        });
      }
    } catch {
      // 忽略
    }
  }

  window.addEventListener('popstate', checkUrlChange);
  const rawPushState = history.pushState;
  const rawReplaceState = history.replaceState;
  history.pushState = function (...args) {
    const res = rawPushState.apply(this, args);
    checkUrlChange();
    return res;
  };
  history.replaceState = function (...args) {
    const res = rawReplaceState.apply(this, args);
    checkUrlChange();
    return res;
  };

  // 2. Pending Capture 管理（参考 chatgpt-route-inspector）
  interface PendingLiveCapture extends ConversationCorrelation {
    captureId: string;
    turnId: string;
    startedAt: number;
    expiresAt: number;
  }

  const pendingLiveCaptures = new Map<string, PendingLiveCapture>();
  const PENDING_CAPTURE_TTL_MS = 10 * 60 * 1000;

  function prunePendingCaptures(now = Date.now()): void {
    for (const [captureId, pending] of pendingLiveCaptures) {
      if (pending.expiresAt <= now) pendingLiveCaptures.delete(captureId);
    }
  }

  function registerPendingCapture(
    captureId: string,
    startedAt: number,
    turnId: string,
    correlation: ConversationCorrelation
  ): void {
    prunePendingCaptures();
    while (pendingLiveCaptures.size >= 32) {
      const oldest = pendingLiveCaptures.keys().next().value as string | undefined;
      if (!oldest) break;
      pendingLiveCaptures.delete(oldest);
    }
    pendingLiveCaptures.set(captureId, {
      captureId,
      turnId,
      startedAt,
      ...correlation,
      expiresAt: Date.now() + PENDING_CAPTURE_TTL_MS,
    });
  }

  function uniqueCandidate(candidates: PendingLiveCapture[]): PendingLiveCapture | null {
    return candidates.length === 1 ? candidates[0] ?? null : null;
  }

  function pendingCaptureFor(evidence: WebSocketRouteEvidence): PendingLiveCapture | null {
    prunePendingCaptures();
    const pending = [...pendingLiveCaptures.values()];

    // 1. 优先通过 inputMessageId 精确匹配
    const inputMatches = pending.filter(
      (c) =>
        Boolean(c.inputMessageId) &&
        (evidence.messageIds.includes(c.inputMessageId ?? '') ||
          evidence.parentIds.includes(c.inputMessageId ?? ''))
    );
    if (inputMatches.length > 0) return uniqueCandidate(inputMatches);

    // 2. 次选通过 parentMessageId 匹配
    const parentMatches = pending.filter(
      (c) =>
        Boolean(c.parentMessageId) &&
        evidence.parentIds.includes(c.parentMessageId ?? '') &&
        (evidence.conversationIds.length === 0 ||
          !c.conversationId ||
          evidence.conversationIds.includes(c.conversationId))
    );
    if (parentMatches.length > 0) return uniqueCandidate(parentMatches);

    // 3. 兜底通过 conversationId 匹配
    const conversationMatches = pending.filter(
      (c) =>
        Boolean(c.conversationId) &&
        evidence.conversationIds.includes(c.conversationId ?? '')
    );
    return uniqueCandidate(conversationMatches);
  }

  // 3. Fetch Hook
  const nativeFetch = window.fetch;

  function requestUrl(input: RequestInfo | URL): string {
    if (typeof input === 'string') return input;
    if (input instanceof URL) return input.href;
    if (typeof Request !== 'undefined' && input instanceof Request) return input.url;
    return String(input);
  }

  async function requestBody(input: RequestInfo | URL, init?: RequestInit): Promise<string | null> {
    if (typeof init?.body === 'string') return init.body;
    if (typeof Request !== 'undefined' && input instanceof Request) {
      try {
        return await input.clone().text();
      } catch {
        return null;
      }
    }
    return null;
  }

  async function inspectFetch(
    target: typeof window.fetch,
    receiver: unknown,
    input: RequestInfo | URL,
    init?: RequestInit
  ): Promise<Response> {
    const url = requestUrl(input);
    const method = (
      init?.method ||
      (typeof Request !== 'undefined' && input instanceof Request ? input.method : 'GET')
    ).toUpperCase();

    const endpoint = classifyEndpoint(url, location.href);

    debugLog('fetch observed', {
      url: url.slice(0, 60),
      method,
      endpointKind: endpoint.kind,
    });

    // 关键守卫：非对话流请求（如复制打点、上传文件、设置等）直接原生放行，绝对不进入 Turn 逻辑！
    if (endpoint.kind !== 'conversation_stream' || method !== 'POST') {
      return target.call(receiver, input as RequestInfo, init);
    }

    const captureId = generateUuid();
    const startedAt = Date.now();

    // 异步提取请求上下文
    const bodyPromise = requestBody(input, init);
    const correlationPromise = bodyPromise.then((raw) => {
      if (!raw) return null;
      const correlation = parseConversationCorrelation(raw);
      if (!correlation) return null;

      const turnId = correlation.inputMessageId || generateUuid();
      const reqConvId = correlation.conversationId || conversationIdFromUrl(location.href);

      registerPendingCapture(captureId, startedAt, turnId, correlation);

      debugLog('REQUEST_START emitted', {
        captureIdPrefix: captureId.slice(0, 8),
        turnIdPrefix: turnId.slice(0, 8),
        hasRequestedModel: Boolean(correlation.requestedModel),
        hasConvId: Boolean(reqConvId),
      });

      safePost('REQUEST_START', {
        captureId,
        turnId,
        requestedModel: correlation.requestedModel,
        conversationId: reqConvId,
        startedAt,
      });

      return { turnId, correlation };
    });

    let response: Response;
    try {
      response = await target.call(receiver, input as RequestInfo, init);
    } catch (err) {
      throw err;
    }

    if (response && response.ok) {
      try {
        const cloned = response.clone();
        const reader = cloned.body?.getReader();
        if (reader) {
          const decoder = new TextDecoder('utf-8');
          const sseParser = new SseStreamParser();

          void correlationPromise.then((ctx) => {
            const boundTurnId = ctx?.turnId || null;
            (async () => {
              try {
                while (true) {
                  const { done, value } = await reader.read();
                  if (done) {
                    const flushed = sseParser.flush();
                    safePost('SSE_CHUNK', {
                      captureId,
                      turnId: boundTurnId,
                      conversationId: flushed.conversationId,
                      requestId: flushed.requestId,
                      resolvedModelSlug: flushed.resolvedModelSlug,
                      serverModelSlug: flushed.serverModelSlug,
                      responseModelSlug: flushed.responseModelSlug,
                      isStreamDone: true,
                    });
                    break;
                  }
                  if (value) {
                    const text = decoder.decode(value, { stream: true });
                    const evidence = sseParser.feed(text);
                    if (
                      evidence.resolvedModelSlug ||
                      evidence.serverModelSlug ||
                      evidence.responseModelSlug ||
                      evidence.conversationId ||
                      evidence.requestId ||
                      evidence.isStreamDone
                    ) {
                      safePost('SSE_CHUNK', {
                        captureId,
                        turnId: boundTurnId,
                        conversationId: evidence.conversationId,
                        requestId: evidence.requestId,
                        resolvedModelSlug: evidence.resolvedModelSlug,
                        serverModelSlug: evidence.serverModelSlug,
                        responseModelSlug: evidence.responseModelSlug,
                        isStreamDone: evidence.isStreamDone,
                      });
                    }
                  }
                }
              } catch {
                // 忽略流读取异常
              }
            })();
          });
        }
      } catch {
        // 忽略 clone 异常
      }
    }

    return response;
  }

  try {
    window.fetch = function (this: unknown, input: RequestInfo | URL, init?: RequestInit) {
      return inspectFetch(nativeFetch, this ?? window, input, init);
    };
  } catch {
    // 忽略赋值异常
  }

  // 4. WebSocket Hook（严格 correlation 绑定，绝不全局广播）
  const nativeWebSocket = window.WebSocket;
  function isTargetWs(urlStr: string): boolean {
    try {
      const parsed = new URL(urlStr, location.href);
      return (
        parsed.hostname.endsWith('.chatgpt.com') ||
        parsed.hostname.endsWith('.openai.com')
      );
    } catch {
      return false;
    }
  }

  function handleWebSocketText(raw: string): void {
    const evidenceItems = parseWebSocketFrame(raw);
    for (const item of evidenceItems) {
      const pending = pendingCaptureFor(item);
      // 关键守卫：如果无法对齐到当前 pending live capture，彻底忽略，绝不广播！
      if (!pending) continue;

      const ev = item.evidence;
      if (
        ev.resolvedModelSlug ||
        ev.serverModelSlug ||
        ev.responseModelSlug ||
        ev.conversationId ||
        ev.isStreamDone ||
        item.terminal
      ) {
        safePost('WS_FRAME', {
          captureId: pending.captureId,
          turnId: pending.turnId,
          conversationId: ev.conversationId || pending.conversationId,
          resolvedModelSlug: ev.resolvedModelSlug,
          serverModelSlug: ev.serverModelSlug,
          responseModelSlug: ev.responseModelSlug,
          isStreamDone: Boolean(ev.isStreamDone || item.terminal),
        });
      }
      if (item.terminal) {
        pendingLiveCaptures.delete(pending.captureId);
      }
    }
  }

  const ProxyWebSocket = function (
    this: WebSocket,
    url: string | URL,
    protocols?: string | string[]
  ) {
    const targetUrl = typeof url === 'string' ? url : url.href;
    const socket = Reflect.construct(nativeWebSocket, protocols !== undefined ? [url, protocols] : [url]);

    if (isTargetWs(targetUrl)) {
      try {
        socket.addEventListener('message', (event: MessageEvent) => {
          if (typeof event.data === 'string') {
            queueMicrotask(() => handleWebSocketText(event.data));
          }
        });
      } catch {
        // 忽略
      }
    }
    return socket;
  } as unknown as typeof WebSocket;

  try {
    ProxyWebSocket.prototype = nativeWebSocket.prototype;
    Object.defineProperty(window, 'WebSocket', {
      value: ProxyWebSocket,
      writable: true,
      configurable: true,
    });
  } catch {
    // 忽略
  }
})();
