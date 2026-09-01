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
import {
  CT_OBSERVER_CONTROL_SOURCE,
  CT_OBSERVER_MESSAGE_SOURCE,
  type PageHookControlMessage,
  type PageHookMessage,
} from './types';

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
      const targetOrigin = window.location.origin;
      // opaque origin 无法安全限定接收方，宁可放弃上报也不广播到 '*'
      if (!targetOrigin || targetOrigin === 'null') return;
      window.postMessage(msg, targetOrigin);
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

  // 2. Pending Capture 与已见 Message ID LRU 管理
  interface PendingLiveCapture extends ConversationCorrelation {
    captureId: string;
    turnId: string;
    startedAt: number;
    expiresAt: number;
  }

  const pendingLiveCaptures = new Map<string, PendingLiveCapture>();
  const activeRequestControllers = new Map<string, AbortController>();
  const socketCaptures = new Map<WebSocket, Set<string>>();
  const targetSockets = new Set<WebSocket>();
  const PENDING_CAPTURE_TTL_MS = 10 * 60 * 1000;

  const seenUserMessageIds = new Set<string>();
  const MAX_SEEN_MESSAGE_IDS = 256;

  function markUserMessageIdSeen(id: string): void {
    if (!id) return;
    if (seenUserMessageIds.size >= MAX_SEEN_MESSAGE_IDS) {
      const oldest = seenUserMessageIds.values().next().value as string | undefined;
      if (oldest) seenUserMessageIds.delete(oldest);
    }
    seenUserMessageIds.add(id);
  }

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

    const inputMatches = pending.filter(
      (c) =>
        Boolean(c.inputMessageId) &&
        (evidence.messageIds.includes(c.inputMessageId ?? '') ||
          evidence.parentIds.includes(c.inputMessageId ?? ''))
    );
    if (inputMatches.length > 0) return uniqueCandidate(inputMatches);

    const parentMatches = pending.filter(
      (c) =>
        Boolean(c.parentMessageId) &&
        evidence.parentIds.includes(c.parentMessageId ?? '') &&
        (evidence.conversationIds.length === 0 ||
          !c.conversationId ||
          evidence.conversationIds.includes(c.conversationId))
    );
    if (parentMatches.length > 0) return uniqueCandidate(parentMatches);

    const conversationMatches = pending.filter(
      (c) =>
        Boolean(c.conversationId) &&
        evidence.conversationIds.includes(c.conversationId ?? '')
    );
    return uniqueCandidate(conversationMatches);
  }

  function reportTurnAborted(pending: PendingLiveCapture, reason: string): void {
    if (pending.inputMessageId) {
      // 失败/中止的请求允许 ChatGPT 重新提交同一用户消息，不把失败误记成 replay。
      seenUserMessageIds.delete(pending.inputMessageId);
    }
    safePost('TURN_ABORTED', {
      captureId: pending.captureId,
      turnId: pending.turnId,
      reason,
    });
    pendingLiveCaptures.delete(pending.captureId);
  }

  // Bridge 在预算到点或用户离开页面时通过 postMessage 请求 MAIN world 中止实际请求。
  window.addEventListener('message', (event) => {
    if (event.source !== window || event.origin !== window.location.origin || window.location.origin === 'null') {
      return;
    }
    const data = event.data as PageHookControlMessage | undefined;
    if (!data || data.source !== CT_OBSERVER_CONTROL_SOURCE || data.type !== 'STOP_TURN') return;

    const captureId = data.payload?.captureId || null;
    const turnId = data.payload?.turnId || null;
    for (const [id, controller] of activeRequestControllers) {
      const pending = pendingLiveCaptures.get(id);
      if ((!captureId && !turnId) || (captureId && id === captureId) || (turnId && pending?.turnId === turnId)) {
        controller.abort();
      }
    }
    for (const socket of targetSockets) {
      try {
        socket.close(4000, data.payload?.reason || 'turn stopped');
      } catch {
        // 忽略已关闭的 WebSocket
      }
    }
  });

  // 3. Fetch Hook (严格判定新用户 Turn，非新 Turn 绝不注册 Live Capture)
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

  function addStopSignal(
    input: RequestInfo | URL,
    init: RequestInit | undefined,
    stopController: AbortController,
  ): RequestInit {
    const existingSignal =
      init?.signal ||
      (typeof Request !== 'undefined' && input instanceof Request ? input.signal : undefined);
    if (existingSignal) {
      if (existingSignal.aborted) {
        stopController.abort();
      } else {
        existingSignal.addEventListener('abort', () => stopController.abort(), { once: true });
      }
    }
    return { ...(init || {}), signal: stopController.signal };
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

    if (endpoint.kind !== 'conversation_stream' || method !== 'POST') {
      return target.call(receiver, input as RequestInfo, init);
    }

    const captureId = generateUuid();
    const startedAt = Date.now();
    const stopController = new AbortController();
    activeRequestControllers.set(captureId, stopController);
    const requestInit = addStopSignal(input, init, stopController);

    const bodyPromise = requestBody(input, init);
    const correlationPromise = bodyPromise.then((raw) => {
      if (!raw) {
        activeRequestControllers.delete(captureId);
        return null;
      }
      const correlation = parseConversationCorrelation(raw);
      if (!correlation) {
        activeRequestControllers.delete(captureId);
        return null;
      }

      // 严格检查是否为真正的新用户发送且未被重放
      if (!correlation.isNewUserTurn) {
        debugLog('Non-turn request observed, short-circuited', { action: correlation.action });
        activeRequestControllers.delete(captureId);
        return null;
      }

      if (correlation.inputMessageId) {
        if (seenUserMessageIds.has(correlation.inputMessageId)) {
          debugLog('MessageId replayed, short-circuited', { msgId: correlation.inputMessageId });
          activeRequestControllers.delete(captureId);
          return null;
        }
        markUserMessageIdSeen(correlation.inputMessageId);
      }

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
      response = await target.call(receiver, input as RequestInfo, requestInit);
    } catch (err) {
      void correlationPromise.then((ctx) => {
        if (ctx) {
          const pending = pendingLiveCaptures.get(captureId);
          if (pending) reportTurnAborted(pending, 'request_failed');
        }
        activeRequestControllers.delete(captureId);
      });
      throw err;
    }

    if (response && !response.ok) {
      void correlationPromise.then((ctx) => {
        if (ctx) {
          const pending = pendingLiveCaptures.get(captureId);
          if (pending) reportTurnAborted(pending, `http_${response.status}`);
        }
        activeRequestControllers.delete(captureId);
      });
      return response;
    }

    if (response && response.ok) {
      try {
        const cloned = response.clone();
        const reader = cloned.body?.getReader();
        if (reader) {
          const decoder = new TextDecoder('utf-8');
          const sseParser = new SseStreamParser();

          void correlationPromise.then((ctx) => {
            if (!ctx) {
              activeRequestControllers.delete(captureId);
              return;
            }
            const boundTurnId = ctx.turnId;
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
                    pendingLiveCaptures.delete(captureId);
                    break;
                  }
                  if (value) {
                    const text = decoder.decode(value, { stream: true });
                    const ev = sseParser.feed(text);
                    if (
                      ev.resolvedModelSlug ||
                      ev.serverModelSlug ||
                      ev.responseModelSlug ||
                      ev.conversationId ||
                      ev.requestId ||
                      ev.isStreamDone
                    ) {
                      safePost('SSE_CHUNK', {
                        captureId,
                        turnId: boundTurnId,
                        conversationId: ev.conversationId,
                        requestId: ev.requestId,
                        resolvedModelSlug: ev.resolvedModelSlug,
                        serverModelSlug: ev.serverModelSlug,
                        responseModelSlug: ev.responseModelSlug,
                        isStreamDone: Boolean(ev.isStreamDone),
                      });
                    }
                  }
                }
              } catch {
                const pending = pendingLiveCaptures.get(captureId);
                if (pending) reportTurnAborted(pending, 'stream_read_failed');
              } finally {
                activeRequestControllers.delete(captureId);
              }
            })();
          });
        } else {
          void correlationPromise.then((ctx) => {
            if (ctx) {
              const pending = pendingLiveCaptures.get(captureId);
              if (pending) reportTurnAborted(pending, 'stream_body_missing');
            }
            activeRequestControllers.delete(captureId);
          });
        }
      } catch {
        void correlationPromise.then((ctx) => {
          if (ctx) {
            const pending = pendingLiveCaptures.get(captureId);
            if (pending) reportTurnAborted(pending, 'stream_clone_failed');
          }
          activeRequestControllers.delete(captureId);
        });
      }
    }

    return response;
  }

  window.fetch = function (this: unknown, input: RequestInfo | URL, init?: RequestInit) {
    return inspectFetch(nativeFetch, this, input, init);
  } as typeof window.fetch;

  // 4. WebSocket Hook
  const nativeWebSocket = window.WebSocket;

  function isTargetWs(url: string): boolean {
    try {
      const parsed = new URL(url);
      const pathname = parsed.pathname;
      return pathname.startsWith('/backend-api') || pathname.startsWith('/backend-anon');
    } catch {
      return false;
    }
  }

  function handleWebSocketText(raw: string, socket?: WebSocket): void {
    // ChatGPT keeps several backend sockets busy even when no turn is being
    // observed. Avoid decoding their frames unless there is a capture to match.
    if (pendingLiveCaptures.size === 0) return;
    const evidenceItems = parseWebSocketFrame(raw);
    for (const item of evidenceItems) {
      const pending = pendingCaptureFor(item);
      if (!pending) continue;

      if (socket) {
        const captures = socketCaptures.get(socket) || new Set<string>();
        captures.add(pending.captureId);
        socketCaptures.set(socket, captures);
      }

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
          requestId: ev.requestId || null,
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

  type QueuedWebSocketFrame = { raw: string; socket: WebSocket };
  const queuedWebSocketFrames: QueuedWebSocketFrame[] = [];
  let webSocketDrainTimer: number | null = null;
  let webSocketDraining = false;
  const WEBSOCKET_DRAIN_BUDGET_MS = 8;

  async function yieldToMainThread(): Promise<void> {
    const scheduler = (globalThis as typeof globalThis & {
      scheduler?: { yield?: () => Promise<void> };
    }).scheduler;
    if (scheduler?.yield) {
      await scheduler.yield();
      return;
    }
    await new Promise<void>((resolve) => window.setTimeout(resolve, 0));
  }

  async function drainWebSocketFrames(): Promise<void> {
    if (webSocketDraining) return;
    webSocketDraining = true;
    try {
      let sliceStartedAt = performance.now();
      const frames = queuedWebSocketFrames.splice(0);
      for (const frame of frames) {
        handleWebSocketText(frame.raw, frame.socket);
        if (performance.now() - sliceStartedAt >= WEBSOCKET_DRAIN_BUDGET_MS) {
          await yieldToMainThread();
          sliceStartedAt = performance.now();
        }
      }
    } finally {
      webSocketDraining = false;
      if (queuedWebSocketFrames.length > 0) scheduleWebSocketDrain();
    }
  }

  function scheduleWebSocketDrain(): void {
    if (webSocketDrainTimer !== null) return;
    // A microtask runs before the browser can paint. Start on a task, then yield
    // by elapsed execution time so large frames cannot monopolize the page.
    webSocketDrainTimer = window.setTimeout(() => {
      webSocketDrainTimer = null;
      void drainWebSocketFrames();
    }, 0);
  }

  function enqueueWebSocketFrame(raw: string, socket: WebSocket): void {
    if (pendingLiveCaptures.size === 0) return;
    queuedWebSocketFrames.push({ raw, socket });
    scheduleWebSocketDrain();
  }

  const ProxyWebSocket = function (
    this: WebSocket,
    url: string | URL,
    protocols?: string | string[]
  ) {
    const targetUrl = typeof url === 'string' ? url : url.href;
    const socket = Reflect.construct(nativeWebSocket, protocols !== undefined ? [url, protocols] : [url]);

    if (isTargetWs(targetUrl)) {
      targetSockets.add(socket);
      try {
        socket.addEventListener('message', (event: MessageEvent) => {
          if (typeof event.data === 'string') {
            enqueueWebSocketFrame(event.data, socket);
          }
        });
        const onSocketEnd = () => {
          targetSockets.delete(socket);
          const captures = socketCaptures.get(socket);
          socketCaptures.delete(socket);
          if (!captures) return;
          for (const captureId of captures) {
            const pending = pendingLiveCaptures.get(captureId);
            if (pending) reportTurnAborted(pending, 'websocket_closed');
          }
        };
        socket.addEventListener('error', onSocketEnd, { once: true });
        socket.addEventListener('close', onSocketEnd, { once: true });
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
