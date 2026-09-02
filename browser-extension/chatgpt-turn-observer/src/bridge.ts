import { conversationIdFromUrl, extractActualModel, generateUuid } from './parsers';
import { normalizeObserverBaseUrl, validateObserverStatusPayload } from './observer-protocol';
import { TurnObserverOverlay } from './overlay';
import { getOrCreateObserverId, loadSettings, saveSettings } from './settings';
import {
  CT_OBSERVER_MESSAGE_SOURCE,
  EMPTY_ROUTE_EVIDENCE,
  type BrowserTurnEvent,
  type EventKind,
  type ObserverSettings,
  type PageHookControlMessage,
  CT_OBSERVER_CONTROL_SOURCE,
  type PageHookMessage,
  type TabTurnState,
} from './types';

export interface OutboxItem {
  event: BrowserTurnEvent;
  attempts: number;
  nextRetryAt: number;
}

export interface OutboxPersistence {
  load: () => Promise<OutboxItem[]>;
  save: (items: readonly OutboxItem[]) => Promise<void>;
}

function isCriticalEvent(kind: EventKind): boolean {
  return kind === 'turn_started' || kind === 'stream_completed' || kind === 'turn_closed';
}

function isNonRetryableClientStatus(status: number): boolean {
  return status === 400 || status === 401 || status === 403 || status === 409 || status === 422;
}

const DEFAULT_TURN_WARNING_MS = 23 * 60 * 1000;
const DEFAULT_TURN_HARD_STOP_MS = 25 * 60 * 1000;

/**
 * A budget stop belongs only to the turn that reached the limit.  Starting a
 * different observed turn must always clear that terminal UI state.
 */
export function resetBudgetStatusForNewTurn(tabState: TabTurnState): void {
  tabState.budgetStatus = 'normal';
}

export function startObservedTurn(
  tabState: TabTurnState,
  details: {
    captureId?: string | null;
    turnId: string;
    requestedModel?: string | null;
    conversationId?: string | null;
    startedAt?: number;
  },
): void {
  resetBudgetStatusForNewTurn(tabState);
  tabState.turnId = details.turnId;
  tabState.activeCaptureId = details.captureId || null;
  tabState.startedAt = details.startedAt || Date.now();
  tabState.completedAt = null;
  tabState.requestedModel = details.requestedModel || null;
  tabState.actualModel = null;
  tabState.state = 'turn_starting';
  if (details.conversationId) {
    tabState.conversationId = details.conversationId;
  }
}

/**
 * Apply a URL-derived conversation ID without treating a new conversation's
 * route assignment as navigation away from its active turn. ChatGPT starts a
 * new conversation before an ID exists, then changes the URL while the same
 * response is still streaming.
 */
export function applyConversationRouteChange(
  tabState: TabTurnState,
  newConversationId: string | null,
  closeCurrentTurn: () => unknown,
  reportConversationResolved: () => unknown = () => undefined,
): boolean {
  if (newConversationId === tabState.conversationId) return false;

  const isActiveNewConversationResolution =
    tabState.turnId !== null &&
    tabState.conversationId === null &&
    newConversationId !== null;

  if (!isActiveNewConversationResolution) {
    closeCurrentTurn();
  }
  tabState.conversationId = newConversationId;
  if (isActiveNewConversationResolution) {
    reportConversationResolved();
  }
  return true;
}

export class BridgeOutbox {
  private queue: OutboxItem[] = [];
  private processingPromise: Promise<void> | null = null;
  private maxAttempts = 5;
  private maxCapacity = 128;
  private onStatusChange?: (status: TabTurnState['bridgeStatus'], message: string | null) => void;
  private restored = false;
  private persistTimer: ReturnType<typeof setTimeout> | null = null;
  private persistenceTail: Promise<void> = Promise.resolve();
  private readonly persistDelayMs = 750;

  constructor(
    private sendFn: (item: OutboxItem) => Promise<{ ok: boolean; status?: number; retryable: boolean; error?: string }>,
    onStatusChange?: (status: TabTurnState['bridgeStatus'], message: string | null) => void,
    private persistence?: OutboxPersistence,
  ) {
    this.onStatusChange = onStatusChange;
  }

  public async restore(): Promise<void> {
    if (this.restored) return;
    this.restored = true;
    if (!this.persistence) return;

    try {
      const persisted = await this.persistence.load();
      const currentIds = new Set(this.queue.map((item) => item.event.event_id));
      this.queue = [
        ...persisted.filter((item) => {
          if (!item?.event?.event_id || currentIds.has(item.event.event_id)) return false;
          currentIds.add(item.event.event_id);
          return true;
        }),
        ...this.queue,
      ];
    } catch {
      // 持久化不可用时仍保留内存队列，不阻断当前页面的观测。
    }
  }

  private persistNow(): Promise<void> {
    if (!this.persistence) return Promise.resolve();
    if (this.persistTimer !== null) {
      clearTimeout(this.persistTimer);
      this.persistTimer = null;
    }
    const snapshot = this.queue.map((item) => ({ ...item, event: { ...item.event } }));
    this.persistenceTail = this.persistenceTail.then(
      () => this.persistence?.save(snapshot),
      () => this.persistence?.save(snapshot),
    ).catch(() => {
      // 持久化不可用时仍保留内存队列，不阻断当前页面的观测。
    });
    return this.persistenceTail;
  }

  private schedulePersist(): void {
    if (!this.persistence || this.persistTimer !== null) return;
    this.persistTimer = setTimeout(() => {
      this.persistTimer = null;
      void this.persistNow();
    }, this.persistDelayMs);
  }

  private persistFor(event: BrowserTurnEvent): Promise<void> | void {
    if (isCriticalEvent(event.event)) return this.persistNow();
    this.schedulePersist();
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
    void this.persistFor(event);
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
    void this.persistNow();
  }

  public process(): Promise<void> {
    if (this.processingPromise) return this.processingPromise;

    const run = (async () => {
      await this.restore();
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
          await this.persistFor(item.event);
          this.onStatusChange?.('synced', '已同步');
        } else if (!result.retryable) {
          // 不可重试的业务错误 (400, 403, 422)，直接出队并记录错误
          this.queue.shift();
          await this.persistFor(item.event);
          this.onStatusChange?.('failed', result.error || `HTTP ${result.status}`);
        } else {
          // 可重试错误 (网络连接断开、超时、5xx、429)
          if (item.attempts >= this.maxAttempts) {
            // 超过最大重试次数，出队丢弃
            this.queue.shift();
            await this.persistFor(item.event);
            this.onStatusChange?.('failed', `重试超限丢弃: ${result.error || '网络异常'}`);
          } else {
            // 指数退避：500ms, 1000ms, 2000ms, 4000ms, 最大 10s
            const backoffMs = Math.min(500 * Math.pow(2, item.attempts - 1), 10_000);
            item.nextRetryAt = Date.now() + backoffMs;
            await this.persistFor(item.event);
            this.onStatusChange?.('failed', `网络重试中 (${item.attempts}/${this.maxAttempts})`);
            setTimeout(() => void this.process(), backoffMs);
            break;
          }
        }
      }
    })();
    this.processingPromise = run.finally(() => {
      this.processingPromise = null;
    });
    return this.processingPromise;
  }
}

export function getStableTabId(): number {
  const storageKey = 'ct_observer_tab_id';
  try {
    const existing = window.sessionStorage.getItem(storageKey);
    const parsed = existing ? Number.parseInt(existing, 10) : NaN;
    if (Number.isSafeInteger(parsed) && parsed > 0) return parsed;

    const generated = Math.floor(Math.random() * 1000000) + 1;
    window.sessionStorage.setItem(storageKey, String(generated));
    return generated;
  } catch {
    // sessionStorage 被策略禁用时仍允许观测运行；后端会通过 tab_instance_id 隔离实例。
    return Math.floor(Math.random() * 1000000) + 1;
  }
}

function createChromeOutboxPersistence(tabId: number): OutboxPersistence {
  const storageKey = `ct_observer_outbox_${tabId}`;
  return {
    load: () =>
      new Promise((resolve) => {
        chrome.storage.local.get([storageKey], (result) => {
          const value = result[storageKey];
          resolve(Array.isArray(value) ? (value as OutboxItem[]) : []);
        });
      }),
    save: (items) =>
      new Promise((resolve) => {
        chrome.storage.local.set({ [storageKey]: items }, () => resolve());
      }),
  };
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
  // sessionStorage 在同一浏览器 Tab 的 reload 中保持不变，避免后端无法识别“刷新后的同一 Tab”。
  const tabId = getStableTabId();
  let sequence = 0;

  let localWorkspaceId: string | null = null;
  let remoteWorkspaceId: string | null = null;
  let localHandshakeDone = false;
  let remoteHandshakeDone = false;
  let localWarningAfterMs = DEFAULT_TURN_WARNING_MS;
  let localHardStopAfterMs = DEFAULT_TURN_HARD_STOP_MS;
  let remoteWarningAfterMs = DEFAULT_TURN_WARNING_MS;
  let remoteHardStopAfterMs = DEFAULT_TURN_HARD_STOP_MS;
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
    budgetStatus: 'normal',
    lastActiveAt: Date.now(),
  };

  let overlay: TurnObserverOverlay | null = null;
  let quietTimer: number | null = null;
  let warningTimer: number | null = null;
  let hardStopTimer: number | null = null;
  let controlPollTimer: number | null = null;
  let uiFrame: number | null = null;
  let controlPollInFlight = false;
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
    if (uiFrame !== null) return;
    uiFrame = window.requestAnimationFrame(() => {
      uiFrame = null;
      overlay?.updateState(tabState);
    });
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

  async function queryStatus(baseUrl: string, timeoutMs = 2000): Promise<{
    ok: boolean;
    workspaceId?: string;
    warningAfterMs?: number;
    hardStopAfterMs?: number;
    status?: number;
  }> {
    const normalizedBaseUrl = normalizeObserverBaseUrl(baseUrl);
    if (!normalizedBaseUrl) return { ok: false };
    const url = `${normalizedBaseUrl}/internal/chatgpt-turn-observer/status`;
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
      if (resp.ok) {
        const check = validateObserverStatusPayload(await resp.json());
        return check.ok
          ? {
              ok: true,
              workspaceId: check.workspaceId,
              warningAfterMs: check.warningAfterMs,
              hardStopAfterMs: check.hardStopAfterMs,
            }
          : { ok: false, status: resp.status };
      }
      return { ok: false, status: resp.status };
    } catch {
      return { ok: false };
    } finally {
      clearTimeout(timer);
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
    localWarningAfterMs = DEFAULT_TURN_WARNING_MS;
    localHardStopAfterMs = DEFAULT_TURN_HARD_STOP_MS;
    remoteWarningAfterMs = DEFAULT_TURN_WARNING_MS;
    remoteHardStopAfterMs = DEFAULT_TURN_HARD_STOP_MS;

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
        localWarningAfterMs = localResult.warningAfterMs || DEFAULT_TURN_WARNING_MS;
        localHardStopAfterMs = localResult.hardStopAfterMs || DEFAULT_TURN_HARD_STOP_MS;
      } else {
        localWorkspaceId = null;
        localHandshakeDone = false;
      }

      if (remoteResult.ok && remoteResult.workspaceId) {
        remoteWorkspaceId = remoteResult.workspaceId;
        remoteHandshakeDone = true;
        remoteWarningAfterMs = remoteResult.warningAfterMs || DEFAULT_TURN_WARNING_MS;
        remoteHardStopAfterMs = remoteResult.hardStopAfterMs || DEFAULT_TURN_HARD_STOP_MS;
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

  async function postEvent(
    baseUrl: string,
    event: BrowserTurnEvent,
    timeoutMs = 2500,
    keepalive = false,
  ): Promise<{ response: Response; accepted: boolean; message?: string }> {
    const normalizedBaseUrl = normalizeObserverBaseUrl(baseUrl);
    if (!normalizedBaseUrl) {
      throw new Error('Observer Base URL 为空');
    }
    const url = `${normalizedBaseUrl}/internal/chatgpt-turn-event`;
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
        keepalive,
      });
      if (!resp.ok) {
        let message: string | undefined;
        try {
          const payload = await resp.clone().json() as {
            error?: unknown;
          };
          if (typeof payload.error === 'string') {
            message = payload.error;
          } else if (payload.error && typeof payload.error === 'object') {
            const detail = payload.error as { code?: unknown; reason?: unknown };
            message = [detail.code, detail.reason]
              .filter((value): value is string => typeof value === 'string' && value.length > 0)
              .join(': ') || undefined;
          }
        } catch {
          // 保留 HTTP 状态作为错误信息。
        }
        return { response: resp, accepted: false, message };
      }

      try {
        const payload = await resp.json() as { ok?: boolean; applied?: boolean; duplicate?: boolean; error?: unknown };
        const accepted = payload.ok === true && (payload.applied === true || payload.duplicate === true);
        const error = payload.error && typeof payload.error === 'object'
          ? payload.error as { code?: unknown; reason?: unknown }
          : null;
        const message = typeof payload.error === 'string'
          ? payload.error
          : [error?.code, error?.reason]
            .filter((value): value is string => typeof value === 'string' && value.length > 0)
            .join(': ') || undefined;
        return {
          response: resp,
          accepted,
          message: accepted
            ? undefined
            : message || '服务端未确认事件已应用',
        };
      } catch {
        return { response: resp, accepted: false, message: '服务端 ACK 不是有效 JSON' };
      }
    } catch (err) {
      throw err;
    } finally {
      clearTimeout(timer);
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
        const ack = await postEvent(settings.localBaseUrl, item.event, 3000);
        if (ack.accepted) return { ok: true, status: ack.response.status, retryable: false };
        const isClientError = isNonRetryableClientStatus(ack.response.status) || ack.response.ok;
        return {
          ok: false,
          status: ack.response.status,
          retryable: !isClientError,
          error: ack.message || `Local HTTP ${ack.response.status}`,
        };
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
        const ack = await postEvent(settings.remoteBaseUrl, item.event, 4000);
        if (ack.accepted) return { ok: true, status: ack.response.status, retryable: false };
        const isClientError = isNonRetryableClientStatus(ack.response.status) || ack.response.ok;
        return {
          ok: false,
          status: ack.response.status,
          retryable: !isClientError,
          error: ack.message || `Remote HTTP ${ack.response.status}`,
        };
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
            const ack = await postEvent(settings.localBaseUrl, item.event, 800);
            if (ack.accepted) return { ok: true, status: ack.response.status, retryable: false };
            const isClientError = isNonRetryableClientStatus(ack.response.status) || ack.response.ok;
            if (isClientError) {
              return {
                ok: false,
                status: ack.response.status,
                retryable: false,
                error: ack.message || `Local HTTP ${ack.response.status}`,
              };
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
            const ack = await postEvent(settings.remoteBaseUrl, item.event, 4000);
            if (ack.accepted) return { ok: true, status: ack.response.status, retryable: false };
            const isClientError = isNonRetryableClientStatus(ack.response.status) || ack.response.ok;
            return {
              ok: false,
              status: ack.response.status,
              retryable: !isClientError,
              error: ack.message || `Remote HTTP ${ack.response.status}`,
            };
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
  }, createChromeOutboxPersistence(tabId));

  // 先恢复持久化队列，再开始接收页面事件，避免刷新时旧生命周期事件被新事件覆盖。
  await outbox.restore();

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
    if (!tabState.turnId) return null;

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
    return event;
  }

  function clearTurnTimers(): void {
    if (warningTimer !== null) {
      clearTimeout(warningTimer);
      warningTimer = null;
    }
    if (hardStopTimer !== null) {
      clearTimeout(hardStopTimer);
      hardStopTimer = null;
    }
  }

  function resetTurnState(budgetStatus: 'normal' | 'warning' | 'stopped' = 'normal'): void {
    tabState.turnId = null;
    tabState.activeCaptureId = null;
    tabState.requestId = null;
    tabState.startedAt = null;
    tabState.completedAt = null;
    tabState.requestedModel = null;
    tabState.actualModel = null;
    tabState.state = 'idle';
    tabState.budgetStatus = budgetStatus;
    completedStreamIds.clear();
    clearTurnTimers();
    stopControlPolling();
  }

  function requestPageHookStop(reason: string): void {
    const targetOrigin = window.location.origin;
    if (!targetOrigin || targetOrigin === 'null') return;
    const message: PageHookControlMessage = {
      source: CT_OBSERVER_CONTROL_SOURCE,
      type: 'STOP_TURN',
      payload: {
        captureId: tabState.activeCaptureId,
        turnId: tabState.turnId,
        reason,
      },
    };
    try {
      window.postMessage(message, targetOrigin);
    } catch {
      // 页面即将终止或 origin 不可用时，仍由 turn_closed 事件告知后端。
    }
  }

  function closeCurrentTurn(reason?: string): BrowserTurnEvent | null {
    if (!tabState.turnId) return null;
    if (reason) requestPageHookStop(reason);

    const completedAt = Date.now();
    tabState.completedAt = completedAt;
    const event = dispatchTurnEvent('turn_closed', { completed_at: completedAt });
    resetTurnState(reason === 'turn_budget_hard_stop' ? 'stopped' : 'normal');
    return event;
  }

  function getTurnBudgetDurations(): { warningAfterMs: number; hardStopAfterMs: number } {
    if (settings.bridgeMode === 'local') {
      return { warningAfterMs: localWarningAfterMs, hardStopAfterMs: localHardStopAfterMs };
    }
    if (settings.bridgeMode === 'remote') {
      return { warningAfterMs: remoteWarningAfterMs, hardStopAfterMs: remoteHardStopAfterMs };
    }
    if (localHandshakeDone) {
      return { warningAfterMs: localWarningAfterMs, hardStopAfterMs: localHardStopAfterMs };
    }
    if (remoteHandshakeDone) {
      return { warningAfterMs: remoteWarningAfterMs, hardStopAfterMs: remoteHardStopAfterMs };
    }
    return { warningAfterMs: DEFAULT_TURN_WARNING_MS, hardStopAfterMs: DEFAULT_TURN_HARD_STOP_MS };
  }

  function scheduleTurnTimers(): void {
    clearTurnTimers();
    if (!tabState.turnId || !tabState.startedAt) return;

    const scheduledTurnId = tabState.turnId;
    const startedAt = tabState.startedAt;
    const { warningAfterMs, hardStopAfterMs } = getTurnBudgetDurations();
    const warnIn = Math.max(0, startedAt + warningAfterMs - Date.now());
    const stopIn = Math.max(0, startedAt + hardStopAfterMs - Date.now());

    warningTimer = window.setTimeout(() => {
      if (tabState.turnId !== scheduledTurnId) return;
      tabState.budgetStatus = 'warning';
      tabState.bridgeMessage = '本轮接近 25 分钟上限，将在到点自动停止';
      updateUi();
    }, warnIn);

    hardStopTimer = window.setTimeout(() => {
      if (tabState.turnId !== scheduledTurnId) return;
      tabState.budgetStatus = 'stopped';
      tabState.bridgeMessage = '本轮已达到 25 分钟上限，正在停止网页生成';
      closeCurrentTurn('turn_budget_hard_stop');
      updateUi();
    }, stopIn);
  }

  async function queryTurnControl(baseUrl: string, turnId: string): Promise<{ command?: string; reason?: string } | null> {
    const normalizedBaseUrl = normalizeObserverBaseUrl(baseUrl);
    if (!normalizedBaseUrl || !settings.bridgeToken.trim()) return null;

    const query = new URLSearchParams({
      observer_id: observerId,
      tab_instance_id: tabInstanceId,
      tab_id: String(tabId),
      turn_id: turnId,
    });
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 1500);
    try {
      const resp = await fetch(`${normalizedBaseUrl}/internal/chatgpt-turn-observer/control?${query.toString()}`, {
        method: 'GET',
        headers: { Authorization: `Bearer ${settings.bridgeToken.trim()}` },
        signal: controller.signal,
      });
      if (!resp.ok) return null;
      const data = await resp.json() as { ok?: boolean; command?: unknown; reason?: unknown };
      if (data.ok !== true) return null;
      return {
        command: typeof data.command === 'string' ? data.command : undefined,
        reason: typeof data.reason === 'string' ? data.reason : undefined,
      };
    } catch {
      return null;
    } finally {
      clearTimeout(timer);
    }
  }

  async function pollTurnControl(expectedTurnId: string): Promise<void> {
    if (controlPollInFlight || tabState.turnId !== expectedTurnId) return;
    const endpoint = getReadyEndpoint();
    if (!endpoint) return;

    controlPollInFlight = true;
    try {
      const command = await queryTurnControl(endpoint.baseUrl, expectedTurnId);
      if (tabState.turnId !== expectedTurnId || !command) return;
      if (command.command === 'warn') {
        tabState.budgetStatus = 'warning';
        tabState.bridgeMessage = '后端报告本轮接近时间上限';
        updateUi();
      } else if (command.command === 'stop_turn') {
        tabState.budgetStatus = 'stopped';
        tabState.bridgeMessage = '后端已通知停止网页生成';
        closeCurrentTurn('turn_budget_hard_stop');
        updateUi();
      }
    } finally {
      controlPollInFlight = false;
    }
  }

  function stopControlPolling(): void {
    if (controlPollTimer !== null) {
      clearTimeout(controlPollTimer);
      controlPollTimer = null;
    }
  }

  function scheduleControlPolling(): void {
    stopControlPolling();
    if (!tabState.turnId) return;

    const expectedTurnId = tabState.turnId;
    const tick = () => {
      if (tabState.turnId !== expectedTurnId) return;
      void pollTurnControl(expectedTurnId).finally(() => {
        if (tabState.turnId === expectedTurnId) {
          controlPollTimer = window.setTimeout(tick, 2000);
        }
      });
    };
    tick();
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
        clearTurnTimers();
        stopControlPolling();
        updateUi();
      }
    }, 1000);
  }

  // 页面关闭时先进入持久化 Outbox，再用 keepalive 尽力发送一次；两条路径使用同一 event_id，后端幂等去重。
  window.addEventListener('pagehide', () => {
    const closeEvent = closeCurrentTurn();
    const endpoint = getReadyEndpoint();
    if (closeEvent && settings.bridgeToken && endpoint) {
      void postEvent(endpoint.baseUrl, { ...closeEvent, workspace_id: endpoint.workspaceId }, 1000, true).catch(() => {
        // Outbox 已保留事件；页面卸载时 keepalive 失败无需再次阻塞。
      });
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
        if (applyConversationRouteChange(
          tabState,
          newConvId,
          closeCurrentTurn,
          () => dispatchTurnEvent('conversation_resolved'),
        )) {
          updateUi();
        }
        break;
      }
      case 'REQUEST_START': {
        const { captureId, turnId, requestedModel, conversationId, startedAt } = data.payload || {};
        if (!turnId) return;

        if (tabState.turnId === turnId) return;
        closeCurrentTurn();

        completedStreamIds.clear();
        if (settings.bridgeToken) tabState.bridgeMessage = null;
        startObservedTurn(tabState, { captureId, turnId, requestedModel, conversationId, startedAt });

        updateUi();
        dispatchTurnEvent('turn_started');
        scheduleTurnTimers();
        scheduleControlPolling();
        break;
      }
      case 'TURN_ABORTED': {
        const { captureId, turnId, reason } = data.payload || {};
        if (captureId && tabState.activeCaptureId && captureId !== tabState.activeCaptureId) return;
        if (turnId && tabState.turnId && turnId !== tabState.turnId) return;
        if (!tabState.turnId) return;

        tabState.bridgeMessage = `网页生成已中止${reason ? `: ${reason}` : ''}`;
        closeCurrentTurn();
        updateUi();
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
        let uiChanged = false;
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
          uiChanged = tabState.state !== 'stream_idle';
          tabState.state = 'stream_idle';
          const streamKey = requestId || captureId || 'sse_stream';
          if (!completedStreamIds.has(streamKey)) {
            completedStreamIds.add(streamKey);
            dispatchTurnEvent('stream_completed');
          }
          handleQuietWindow(streamKey);
        } else {
          uiChanged = tabState.state !== 'active';
          tabState.state = 'active';
        }

        if (stateChanged && !isStreamDone) {
          dispatchTurnEvent('turn_updated');
        }

        if (stateChanged || uiChanged) updateUi();
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
        let uiChanged = false;
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
          uiChanged = tabState.state !== 'stream_idle';
          tabState.state = 'stream_idle';
          const streamKey = requestId || captureId || 'ws_stream';
          if (!completedStreamIds.has(streamKey)) {
            completedStreamIds.add(streamKey);
            dispatchTurnEvent('stream_completed');
          }
          handleQuietWindow(streamKey);
        } else {
          uiChanged = tabState.state !== 'active';
          tabState.state = 'active';
        }

        if (stateChanged && !isStreamDone) {
          dispatchTurnEvent('turn_updated');
        }

        if (stateChanged || uiChanged) updateUi();
        break;
      }
    }
  });
}

if (typeof window !== 'undefined' && typeof chrome !== 'undefined' && chrome?.storage?.local) {
  void initBridge();
}
