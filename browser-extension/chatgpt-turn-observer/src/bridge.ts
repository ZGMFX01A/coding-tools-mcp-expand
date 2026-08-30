import { conversationIdFromUrl, extractActualModel, generateUuid } from './parsers';
import { TurnObserverOverlay } from './overlay';
import { getOrCreateObserverId, loadSettings, saveSettings } from './settings';
import {
  CT_OBSERVER_MESSAGE_SOURCE,
  EMPTY_ROUTE_EVIDENCE,
  type BrowserTurnEvent,
  type EventKind,
  type ObserverSettings,
  type PageHookMessage,
  type TabTurnState,
} from './types';

export interface OutboxItem {
  event: BrowserTurnEvent;
  attempts: number;
  nextRetryAt: number;
}

function isCriticalEvent(kind: EventKind): boolean {
  return kind === 'turn_started' || kind === 'stream_completed' || kind === 'turn_closed';
}

function isNonRetryableClientStatus(status: number): boolean {
  return status === 400 || status === 401 || status === 403 || status === 409 || status === 422;
}

export class BridgeOutbox {
  private queue: OutboxItem[] = [];
  private isProcessing = false;
  private maxAttempts = 5;
  private maxCapacity = 128;
  private onStatusChange?: (status: TabTurnState['bridgeStatus'], message: string | null) => void;

  constructor(
    private sendFn: (item: OutboxItem) => Promise<{ ok: boolean; status?: number; retryable: boolean; error?: string }>,
    onStatusChange?: (status: TabTurnState['bridgeStatus'], message: string | null) => void
  ) {
    this.onStatusChange = onStatusChange;
  }

  public enqueue(event: BrowserTurnEvent): boolean {
    if (this.queue.length >= this.maxCapacity) {
      // 队列溢出时只能淘汰普通更新，不能丢失生命周期关键事件。
      const evictIndex = this.queue.findIndex((item) => !isCriticalEvent(item.event.event));
      if (evictIndex < 0) {
        this.onStatusChange?.('failed', `Outbox 已满，拒绝关键事件 ${event.event}`);
        return false;
      }
      this.queue.splice(evictIndex, 1);
    }
    this.queue.push({
      event,
      attempts: 0,
      nextRetryAt: 0,
    });
    void this.process();
    return true;
  }

  public getQueueLength(): number {
    return this.queue.length;
  }

  public getQueue(): readonly OutboxItem[] {
    return this.queue;
  }

  public clear(): void {
    this.queue = [];
  }

  public async process(): Promise<void> {
    if (this.isProcessing) return;
    this.isProcessing = true;

    try {
      while (this.queue.length > 0) {
        const item = this.queue[0];
        const now = Date.now();

        if (now < item.nextRetryAt) {
          // 未到重试时间，设置定时器后暂停
          const delay = item.nextRetryAt - now;
          setTimeout(() => void this.process(), delay);
          break;
        }

        this.onStatusChange?.('sending', null);
        item.attempts += 1;

        let result: Awaited<ReturnType<typeof this.sendFn>>;
        try {
          result = await this.sendFn(item);
        } catch (err) {
          result = {
            ok: false,
            retryable: true,
            error: err instanceof Error ? err.message : '发送器异常',
          };
        }

        if (result.ok) {
          // 发送成功，出队
          this.queue.shift();
          this.onStatusChange?.('synced', '已同步');
        } else if (!result.retryable) {
          // 不可重试的业务错误 (400, 403, 422)，直接出队并记录错误
          this.queue.shift();
          this.onStatusChange?.('failed', result.error || `HTTP ${result.status}`);
        } else {
          // 可重试错误 (网络连接断开、超时、5xx、429)
          if (item.attempts >= this.maxAttempts) {
            // 超过最大重试次数，出队丢弃
            this.queue.shift();
            this.onStatusChange?.('failed', `重试超限丢弃: ${result.error || '网络异常'}`);
          } else {
            // 指数退避：500ms, 1000ms, 2000ms, 4000ms, 最大 10s
            const backoffMs = Math.min(500 * Math.pow(2, item.attempts - 1), 10_000);
            item.nextRetryAt = Date.now() + backoffMs;
            this.onStatusChange?.('failed', `网络重试中 (${item.attempts}/${this.maxAttempts})`);
            setTimeout(() => void this.process(), backoffMs);
            break;
          }
        }
      }
    } finally {
      this.isProcessing = false;
    }
  }
}

export async function initBridge() {
  function debugLog(...args: unknown[]) {
    try {
      if ((window as unknown as { __CT_DEBUG__?: boolean }).__CT_DEBUG__) {
        console.log('[CT Observer Bridge]', ...args);
      }
    } catch {
      // 忽略
    }
  }

  debugLog('bridge loaded');

  const observerId = await getOrCreateObserverId();
  let settings: ObserverSettings = await loadSettings();

  const tabInstanceId = generateUuid();
  const tabId = Math.floor(Math.random() * 1000000) + 1;
  let sequence = 0;

  let localWorkspaceId: string | null = null;
  let remoteWorkspaceId: string | null = null;
  let localHandshakeDone = false;
  let remoteHandshakeDone = false;
  let handshakePromise: Promise<void> | null = null;
  let handshakeRevision = 0;

  const tabState: TabTurnState = {
    tabId,
    conversationId: conversationIdFromUrl(location.href),
    turnId: null,
    activeCaptureId: null,
    requestId: null,
    startedAt: null,
    completedAt: null,
    requestedModel: null,
    actualModel: null,
    state: 'idle',
    bridgeStatus: settings.bridgeToken ? 'idle' : 'not_configured',
    bridgeMessage: settings.bridgeToken ? null : '未配置 Token',
    lastActiveAt: Date.now(),
  };

  let overlay: TurnObserverOverlay | null = null;
  let quietTimer: number | null = null;
  const completedStreamIds = new Set<string>();

  function updateUi() {
    if (!overlay) {
      overlay = new TurnObserverOverlay(
        settings.overlayPosition,
        settings.overlayCollapsed,
        (pos) => saveSettings({ overlayPosition: pos }),
        (collapsed) => saveSettings({ overlayCollapsed: collapsed }),
        async () => {
          settings = await loadSettings();
          if (settings.bridgeToken) {
            tabState.bridgeStatus = 'idle';
            tabState.bridgeMessage = null;
          }
           await requestHandshake();
          updateUi();
        }
      );
    }
    overlay.updateState(tabState);
  }

  updateUi();

  chrome.storage.onChanged.addListener((changes, area) => {
    if (area === 'local' && changes.ct_observer_settings) {
      settings = changes.ct_observer_settings.newValue;
      if (!settings.bridgeToken) {
        tabState.bridgeStatus = 'not_configured';
        tabState.bridgeMessage = '未配置 Token';
      }
       void requestHandshake();
      updateUi();
    }
  });

  async function queryStatus(baseUrl: string, timeoutMs = 2000): Promise<{ ok: boolean; workspaceId?: string; status?: number }> {
    const url = `${baseUrl.trim().replace(/\/+$/, '')}/internal/chatgpt-turn-observer/status`;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    try {
      const resp = await fetch(url, {
        method: 'GET',
        headers: {
          Authorization: `Bearer ${settings.bridgeToken.trim()}`,
        },
        signal: controller.signal,
      });
      clearTimeout(timer);
      if (resp.ok) {
        const data = await resp.json() as { workspace_id?: string };
        return { ok: true, workspaceId: data.workspace_id };
      }
      return { ok: false, status: resp.status };
    } catch {
      clearTimeout(timer);
      return { ok: false };
    }
  }

  async function handshakeWorkspaces(): Promise<void> {
    if (handshakePromise) return handshakePromise;

    const snapshot = settings;
    const revisionAtStart = handshakeRevision;
    const token = snapshot.bridgeToken.trim();
    const localUrl = snapshot.localBaseUrl.trim();
    const remoteUrl = snapshot.remoteBaseUrl.trim();

    // 在重新握手完成前，旧 endpoint/workspace 绑定一律失效，避免配置变更后
    // 把旧 workspace_id 发往新地址。
    localWorkspaceId = null;
    localHandshakeDone = false;
    remoteWorkspaceId = null;
    remoteHandshakeDone = false;

    handshakePromise = (async () => {
      if (!token) return;

      const [localResult, remoteResult] = await Promise.all([
        localUrl ? queryStatus(localUrl, 1500) : Promise.resolve({ ok: false } as const),
        remoteUrl ? queryStatus(remoteUrl, 2500) : Promise.resolve({ ok: false } as const),
      ]);

      // 配置可能在握手期间变化；旧 endpoint 的结果不能污染新配置。
      if (settings.bridgeToken.trim() !== token || settings.localBaseUrl.trim() !== localUrl || settings.remoteBaseUrl.trim() !== remoteUrl) {
        return;
      }

      if (localResult.ok && localResult.workspaceId) {
        localWorkspaceId = localResult.workspaceId;
        localHandshakeDone = true;
      } else {
        localWorkspaceId = null;
        localHandshakeDone = false;
      }

      if (remoteResult.ok && remoteResult.workspaceId) {
        remoteWorkspaceId = remoteResult.workspaceId;
        remoteHandshakeDone = true;
      } else {
        remoteWorkspaceId = null;
        remoteHandshakeDone = false;
      }
    })().finally(() => {
      handshakePromise = null;
      if (handshakeRevision !== revisionAtStart) {
        void handshakeWorkspaces();
      }
    });

    return handshakePromise;
  }

  function requestHandshake(): Promise<void> {
    handshakeRevision += 1;
    return handshakeWorkspaces();
  }

  void handshakeWorkspaces();

  async function postEvent(baseUrl: string, event: BrowserTurnEvent, timeoutMs = 2500): Promise<Response> {
    const url = `${baseUrl.trim().replace(/\/+$/, '')}/internal/chatgpt-turn-event`;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);

    try {
      const resp = await fetch(url, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          Authorization: `Bearer ${settings.bridgeToken.trim()}`,
        },
        body: JSON.stringify(event),
        signal: controller.signal,
      });
      clearTimeout(timer);
      return resp;
    } catch (err) {
      clearTimeout(timer);
      throw err;
    }
  }

  // 创建真实 Outbox 队列实例
  const outbox = new BridgeOutbox(async (item) => {
    if (!settings.bridgeToken) {
      return { ok: false, retryable: false, error: '未配置 Token' };
    }

    const mode = settings.bridgeMode;

    if (mode === 'local') {
      if (!settings.localBaseUrl) return { ok: false, retryable: false, error: '未配置 Local Base URL' };
      if (!localHandshakeDone) {
        await handshakeWorkspaces();
        if (!localHandshakeDone) return { ok: false, retryable: true, error: 'Local 尚未握手就绪' };
      }
      if (!localWorkspaceId) return { ok: false, retryable: true, error: 'Local workspace 未完成握手' };
      item.event.workspace_id = localWorkspaceId;
      try {
        const resp = await postEvent(settings.localBaseUrl, item.event, 3000);
        if (resp.ok) return { ok: true, status: 200, retryable: false };
        const isClientError = isNonRetryableClientStatus(resp.status);
        return { ok: false, status: resp.status, retryable: !isClientError, error: `Local HTTP ${resp.status}` };
      } catch (err) {
        return { ok: false, retryable: true, error: err instanceof Error ? err.message : 'Local 网络错误' };
      }
    } else if (mode === 'remote') {
      if (!settings.remoteBaseUrl) return { ok: false, retryable: false, error: '未配置 Remote Base URL' };
      if (!remoteHandshakeDone) {
        await handshakeWorkspaces();
        if (!remoteHandshakeDone) return { ok: false, retryable: true, error: 'Remote 尚未握手就绪' };
      }
      if (!remoteWorkspaceId) return { ok: false, retryable: true, error: 'Remote workspace 未完成握手' };
      item.event.workspace_id = remoteWorkspaceId;
      try {
        const resp = await postEvent(settings.remoteBaseUrl, item.event, 4000);
        if (resp.ok) return { ok: true, status: 200, retryable: false };
        const isClientError = isNonRetryableClientStatus(resp.status);
        return { ok: false, status: resp.status, retryable: !isClientError, error: `Remote HTTP ${resp.status}` };
      } catch (err) {
        return { ok: false, retryable: true, error: err instanceof Error ? err.message : 'Remote 网络错误' };
      }
    } else {
      // Auto 模式
      let localFailedWithNetwork = false;
      if (settings.localBaseUrl) {
        if (!localHandshakeDone) await handshakeWorkspaces();
        if (localHandshakeDone && localWorkspaceId) {
          item.event.workspace_id = localWorkspaceId;
          try {
            const resp = await postEvent(settings.localBaseUrl, item.event, 800);
            if (resp.ok) return { ok: true, status: 200, retryable: false };
            const isClientError = isNonRetryableClientStatus(resp.status);
            if (isClientError) {
              return { ok: false, status: resp.status, retryable: false, error: `Local HTTP ${resp.status}` };
            }
            // Local 网关故障也允许 Auto 尝试 Remote；业务 4xx 已在上面终止。
            localFailedWithNetwork = true;
          } catch {
            localFailedWithNetwork = true;
          }
        } else {
          localFailedWithNetwork = true;
        }
      } else {
        localFailedWithNetwork = true;
      }

      if (localFailedWithNetwork && settings.remoteBaseUrl) {
        if (!remoteHandshakeDone) await handshakeWorkspaces();
        if (remoteHandshakeDone && remoteWorkspaceId) {
          item.event.workspace_id = remoteWorkspaceId;
          try {
            const resp = await postEvent(settings.remoteBaseUrl, item.event, 4000);
            if (resp.ok) return { ok: true, status: 200, retryable: false };
            const isClientError = isNonRetryableClientStatus(resp.status);
            return { ok: false, status: resp.status, retryable: !isClientError, error: `Remote HTTP ${resp.status}` };
          } catch (err) {
            return { ok: false, retryable: true, error: err instanceof Error ? err.message : 'Remote 网络错误' };
          }
        }
      }

      return { ok: false, retryable: true, error: 'Local/Remote 均不可用' };
    }
  }, (status, msg) => {
    tabState.bridgeStatus = status;
    tabState.bridgeMessage = msg;
    updateUi();
  });

  function getReadyEndpoint(): { baseUrl: string; workspaceId: string } | null {
    if (settings.bridgeMode === 'local') {
      return localHandshakeDone && localWorkspaceId && settings.localBaseUrl
        ? { baseUrl: settings.localBaseUrl, workspaceId: localWorkspaceId }
        : null;
    }
    if (settings.bridgeMode === 'remote') {
      return remoteHandshakeDone && remoteWorkspaceId && settings.remoteBaseUrl
        ? { baseUrl: settings.remoteBaseUrl, workspaceId: remoteWorkspaceId }
        : null;
    }
    if (localHandshakeDone && localWorkspaceId && settings.localBaseUrl) {
      return { baseUrl: settings.localBaseUrl, workspaceId: localWorkspaceId };
    }
    if (remoteHandshakeDone && remoteWorkspaceId && settings.remoteBaseUrl) {
      return { baseUrl: settings.remoteBaseUrl, workspaceId: remoteWorkspaceId };
    }
    return null;
  }

  function dispatchTurnEvent(kind: EventKind, extra?: Partial<BrowserTurnEvent>) {
    if (!tabState.turnId && kind !== 'turn_closed') return;

    sequence += 1;
    const event: BrowserTurnEvent = {
      schema_version: 1,
      event_id: generateUuid(),
      tab_instance_id: tabInstanceId,
      observer_id: observerId,
      tab_id: tabId,
      sequence,
      event: kind,
      workspace_id: getReadyEndpoint()?.workspaceId || '',
      conversation_id: tabState.conversationId,
      turn_id: tabState.turnId || 'none',
      request_id: tabState.requestId,
      started_at: tabState.startedAt || Date.now(),
      completed_at: tabState.completedAt,
      requested_model: tabState.requestedModel,
      actual_model: tabState.actualModel,
      ...extra,
    };

    outbox.enqueue(event);
  }

  function handleQuietWindow(streamId?: string | null) {
    if (quietTimer !== null) {
      clearTimeout(quietTimer);
    }
    quietTimer = window.setTimeout(() => {
      if (tabState.state === 'stream_idle') {
        tabState.state = 'completed';
        tabState.completedAt = Date.now();
        const key = streamId || tabState.requestId || tabState.activeCaptureId || 'default_stream';
        if (!completedStreamIds.has(key)) {
          completedStreamIds.add(key);
          dispatchTurnEvent('stream_completed');
        }
        updateUi();
      }
    }, 1000);
  }

  // 页面关闭时使用 fetch keepalive 派发认证关闭事件
  window.addEventListener('pagehide', () => {
    const endpoint = getReadyEndpoint();
    if (tabState.turnId && settings.bridgeToken && endpoint) {
      const closeEvent: BrowserTurnEvent = {
        schema_version: 1,
        event_id: generateUuid(),
        tab_instance_id: tabInstanceId,
        observer_id: observerId,
        tab_id: tabId,
        sequence: ++sequence,
        event: 'turn_closed',
        workspace_id: endpoint.workspaceId,
        conversation_id: tabState.conversationId,
        turn_id: tabState.turnId,
        request_id: tabState.requestId,
        started_at: tabState.startedAt || Date.now(),
        completed_at: Date.now(),
        requested_model: tabState.requestedModel,
        actual_model: tabState.actualModel,
      };

      if (endpoint) {
        const url = `${endpoint.baseUrl.trim().replace(/\/+$/, '')}/internal/chatgpt-turn-event`;
        try {
          void fetch(url, {
            method: 'POST',
            headers: {
              'Content-Type': 'application/json',
              Authorization: `Bearer ${settings.bridgeToken.trim()}`,
            },
            body: JSON.stringify(closeEvent),
            keepalive: true,
          });
        } catch {
          // 忽略关闭发送异常
        }
      }
    }
  });

  // 监听来自 page-hook 的消息 (严格 origin 校验)
  window.addEventListener('message', (event) => {
    if (event.source !== window || event.origin !== window.location.origin || window.location.origin === 'null') {
      return;
    }
    const data = event.data as PageHookMessage | undefined;
    if (!data || data.source !== CT_OBSERVER_MESSAGE_SOURCE) return;

    tabState.lastActiveAt = Date.now();

    switch (data.type) {
      case 'URL_CHANGE': {
        const newConvId = data.payload?.conversationId || null;
        if (newConvId && newConvId !== tabState.conversationId) {
          if (tabState.turnId) {
            dispatchTurnEvent('turn_closed', { completed_at: Date.now() });
          }
          tabState.conversationId = newConvId;
          tabState.turnId = null;
          tabState.activeCaptureId = null;
          tabState.requestId = null;
          tabState.startedAt = null;
          tabState.completedAt = null;
          tabState.requestedModel = null;
          tabState.actualModel = null;
          tabState.state = 'idle';
          completedStreamIds.clear();
          updateUi();
        }
        break;
      }
      case 'REQUEST_START': {
        const { captureId, turnId, requestedModel, conversationId, startedAt } = data.payload || {};
        if (!turnId) return;

        completedStreamIds.clear();
        tabState.turnId = turnId;
        tabState.activeCaptureId = captureId || null;
        tabState.startedAt = startedAt || Date.now();
        tabState.completedAt = null;
        tabState.requestedModel = requestedModel || null;
        tabState.actualModel = null;
        tabState.state = 'turn_starting';
        if (conversationId) {
          tabState.conversationId = conversationId;
        }

        updateUi();
        dispatchTurnEvent('turn_started');
        break;
      }
      case 'SSE_CHUNK': {
        const { captureId, turnId, conversationId, requestId, resolvedModelSlug, serverModelSlug, responseModelSlug, isStreamDone } =
          data.payload || {};

        if (captureId && tabState.activeCaptureId && captureId !== tabState.activeCaptureId) {
          return;
        }
        if (turnId && tabState.turnId && turnId !== tabState.turnId) {
          return;
        }

        let stateChanged = false;
        if (requestId && requestId !== tabState.requestId) {
          tabState.requestId = requestId;
        }

        const evidence = {
          ...EMPTY_ROUTE_EVIDENCE,
          resolvedModelSlug: resolvedModelSlug || null,
          serverModelSlug: serverModelSlug || null,
          responseModelSlug: responseModelSlug || null,
        };
        const actual = extractActualModel(evidence);
        if (actual && actual !== tabState.actualModel) {
          tabState.actualModel = actual;
          stateChanged = true;
        }

        if (conversationId && !tabState.conversationId) {
          tabState.conversationId = conversationId;
          stateChanged = true;
          dispatchTurnEvent('conversation_resolved');
        }

        if (isStreamDone) {
          tabState.state = 'stream_idle';
          const streamKey = requestId || captureId || 'sse_stream';
          if (!completedStreamIds.has(streamKey)) {
            completedStreamIds.add(streamKey);
            dispatchTurnEvent('stream_completed');
          }
          handleQuietWindow(streamKey);
        } else {
          tabState.state = 'active';
        }

        if (stateChanged && !isStreamDone) {
          dispatchTurnEvent('turn_updated');
        }

        updateUi();
        break;
      }
      case 'WS_FRAME': {
        const { captureId, turnId, conversationId, requestId, resolvedModelSlug, serverModelSlug, responseModelSlug, isStreamDone } =
          data.payload || {};

        if (captureId && tabState.activeCaptureId && captureId !== tabState.activeCaptureId) {
          return;
        }
        if (turnId && tabState.turnId && turnId !== tabState.turnId) {
          return;
        }

        let stateChanged = false;
        if (requestId && requestId !== tabState.requestId) {
          tabState.requestId = requestId;
        }

        const actual = extractActualModel({
          ...EMPTY_ROUTE_EVIDENCE,
          resolvedModelSlug: resolvedModelSlug || null,
          serverModelSlug: serverModelSlug || null,
          responseModelSlug: responseModelSlug || null,
        });
        if (actual && actual !== tabState.actualModel) {
          tabState.actualModel = actual;
          stateChanged = true;
        }

        if (conversationId && !tabState.conversationId) {
          tabState.conversationId = conversationId;
          stateChanged = true;
          dispatchTurnEvent('conversation_resolved');
        }

        if (isStreamDone) {
          tabState.state = 'stream_idle';
          const streamKey = requestId || captureId || 'ws_stream';
          if (!completedStreamIds.has(streamKey)) {
            completedStreamIds.add(streamKey);
            dispatchTurnEvent('stream_completed');
          }
          handleQuietWindow(streamKey);
        } else {
          tabState.state = 'active';
        }

        if (stateChanged && !isStreamDone) {
          dispatchTurnEvent('turn_updated');
        }

        updateUi();
        break;
      }
    }
  });
}

if (typeof window !== 'undefined' && typeof chrome !== 'undefined' && chrome?.storage?.local) {
  void initBridge();
}
