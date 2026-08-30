import {
  conversationIdFromUrl,
  generateUuid,
  isConversationEndpoint,
  parseConversationRequest,
  parseWebSocketFrame,
  SseStreamParser,
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

  // 1. URL 监听
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

  // 2. Fetch Hook
  const nativeFetch = window.fetch;

  function handleRequestBody(
    bodyText: string | null,
    urlStr: string,
    startedAt: number
  ): string {
    let turnId = generateUuid();
    let requestedModel: string | null = null;
    let reqConvId = conversationIdFromUrl(urlStr) || conversationIdFromUrl(location.href);

    if (bodyText) {
      try {
        const parsed = parseConversationRequest(bodyText);
        turnId = parsed.turnId;
        requestedModel = parsed.requestedModel;
        if (parsed.conversationId) {
          reqConvId = parsed.conversationId;
        }
      } catch {
        // 解析异常兜底
      }
    }

    debugLog('REQUEST_START emitted', {
      turnIdPrefix: turnId.slice(0, 8),
      hasRequestedModel: Boolean(requestedModel),
      hasConvId: Boolean(reqConvId),
    });

    safePost('REQUEST_START', {
      turnId,
      requestedModel,
      conversationId: reqConvId,
      startedAt,
    });

    return turnId;
  }

  async function inspectFetch(
    target: typeof window.fetch,
    receiver: unknown,
    input: RequestInfo | URL,
    init?: RequestInit
  ): Promise<Response> {
    const isRequestObj = typeof Request !== 'undefined' && input instanceof Request;
    const request = isRequestObj ? (input as Request) : null;

    let urlStr = '';
    if (typeof input === 'string') {
      urlStr = input;
    } else if (input instanceof URL) {
      urlStr = input.href;
    } else if (request) {
      urlStr = request.url;
    }

    const method = (
      init?.method ||
      request?.method ||
      'GET'
    ).toUpperCase();

    const isConv = isConversationEndpoint(urlStr);
    const startedAt = Date.now();
    let turnId: string | null = null;

    debugLog('fetch intercepted', {
      url: urlStr.slice(0, 64),
      method,
      isConv,
      isRequestObj,
      hasInitBody: Boolean(init?.body),
    });

    if (isConv && method === 'POST') {
      if (typeof init?.body === 'string') {
        turnId = handleRequestBody(init.body, urlStr, startedAt);
      } else if (request) {
        try {
          // 安全异步 clone Request 读取 body，绝不能直接消费原始 request
          const clonedReq = request.clone();
          clonedReq
            .text()
            .then((text) => {
              turnId = handleRequestBody(text, urlStr, startedAt);
            })
            .catch(() => {
              turnId = handleRequestBody(null, urlStr, startedAt);
            });
        } catch {
          turnId = handleRequestBody(null, urlStr, startedAt);
        }
      } else {
        turnId = handleRequestBody(null, urlStr, startedAt);
      }
    }

    let response: Response;
    try {
      response = await nativeFetch.call(receiver, input as RequestInfo, init);
    } catch (err) {
      throw err;
    }

    if (isConv && response && response.ok) {
      try {
        const cloned = response.clone();
        const reader = cloned.body?.getReader();
        if (reader) {
          const decoder = new TextDecoder('utf-8');
          const sseParser = new SseStreamParser();

          (async () => {
            try {
              while (true) {
                const { done, value } = await reader.read();
                if (done) {
                  const flushed = sseParser.flush();
                  safePost('SSE_CHUNK', {
                    turnId,
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
                      turnId,
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

  // 3. WebSocket Hook
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
            try {
              const evidence = parseWebSocketFrame(event.data);
              if (
                evidence.resolvedModelSlug ||
                evidence.serverModelSlug ||
                evidence.responseModelSlug ||
                evidence.conversationId ||
                evidence.isStreamDone
              ) {
                safePost('WS_FRAME', {
                  conversationId: evidence.conversationId,
                  resolvedModelSlug: evidence.resolvedModelSlug,
                  serverModelSlug: evidence.serverModelSlug,
                  responseModelSlug: evidence.responseModelSlug,
                  isStreamDone: evidence.isStreamDone,
                });
              }
            } catch {
              // 忽略
            }
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
