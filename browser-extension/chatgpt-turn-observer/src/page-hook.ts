import {
  conversationIdFromUrl,
  parseConversationRequest,
  parseSseChunk,
  parseWebSocketFrame,
} from './parsers';
import { CT_OBSERVER_MESSAGE_SOURCE, type PageHookMessage } from './types';

(function initPageHook() {
  // 防止重复初始化
  if ((window as unknown as { __CT_PAGE_HOOK_INSTALLED__?: boolean }).__CT_PAGE_HOOK_INSTALLED__) {
    return;
  }
  (window as unknown as { __CT_PAGE_HOOK_INSTALLED__?: boolean }).__CT_PAGE_HOOK_INSTALLED__ = true;

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

  function isConversationEndpoint(urlStr: string): boolean {
    return (
      urlStr.includes('/backend-api/conversation') ||
      urlStr.includes('/backend-api/lat/r') ||
      urlStr.includes('/backend-api/f/r') ||
      urlStr.includes('/backend-anon/conversation')
    );
  }

  async function inspectFetch(
    target: typeof window.fetch,
    receiver: unknown,
    input: RequestInfo | URL,
    init?: RequestInit
  ): Promise<Response> {
    const urlStr = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    const isConv = isConversationEndpoint(urlStr);

    let startedAt = Date.now();
    let turnId: string | null = null;
    let requestedModel: string | null = null;
    let reqConvId: string | null = null;

    if (isConv && init && init.method && init.method.toUpperCase() === 'POST') {
      try {
        if (typeof init.body === 'string') {
          const parsed = parseConversationRequest(init.body);
          turnId = parsed.turnId;
          requestedModel = parsed.requestedModel;
          reqConvId = parsed.conversationId || conversationIdFromUrl(location.href);

          if (turnId) {
            safePost('REQUEST_START', {
              turnId,
              requestedModel,
              conversationId: reqConvId,
              startedAt,
            });
          }
        }
      } catch {
        // 解析异常绝不影响正常发送
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
          (async () => {
            try {
              while (true) {
                const { done, value } = await reader.read();
                if (done) {
                  safePost('SSE_CHUNK', {
                    turnId,
                    isStreamDone: true,
                  });
                  break;
                }
                if (value) {
                  const text = decoder.decode(value, { stream: true });
                  const evidence = parseSseChunk(text);
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
