import { conversationIdFromUrl, extractActualModel } from './parsers';
import { TurnObserverOverlay } from './overlay';
import { getOrCreateObserverId, loadSettings, saveSettings } from './settings';
import {
  CT_OBSERVER_MESSAGE_SOURCE,
  type BrowserTurnEvent,
  type ObserverSettings,
  type PageHookMessage,
  type TabTurnState,
} from './types';

(async function initBridge() {
  const observerId = await getOrCreateObserverId();
  let settings: ObserverSettings = await loadSettings();

  // Tab 级独立 ID 与状态
  const tabId = Math.floor(Math.random() * 1000000) + 1;
  const tabState: TabTurnState = {
    tabId,
    conversationId: conversationIdFromUrl(location.href),
    turnId: null,
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

  function updateUi() {
    if (!overlay) {
      overlay = new TurnObserverOverlay(
        settings.overlayPosition,
        settings.overlayCollapsed,
        (pos) => saveSettings({ overlayPosition: pos }),
        (collapsed) => saveSettings({ overlayCollapsed: collapsed })
      );
    }
    overlay.updateState(tabState);
  }

  // 监听 Storage 变化动态刷新配置
  chrome.storage.onChanged.addListener((changes, area) => {
    if (area === 'local' && changes.ct_observer_settings) {
      settings = changes.ct_observer_settings.newValue;
      if (!settings.bridgeToken) {
        tabState.bridgeStatus = 'not_configured';
        tabState.bridgeMessage = '未配置 Token';
      }
      updateUi();
    }
  });

  // 核心 HTTP 事件派发器
  async function postEvent(baseUrl: string, event: BrowserTurnEvent, timeoutMs = 2000): Promise<Response> {
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

  async function dispatchTurnEvent(event: BrowserTurnEvent) {
    if (!settings.bridgeToken) {
      tabState.bridgeStatus = 'not_configured';
      tabState.bridgeMessage = '未配置 Token';
      updateUi();
      return;
    }

    tabState.bridgeStatus = 'sending';
    tabState.bridgeMessage = null;
    updateUi();

    const mode = settings.bridgeMode;

    try {
      if (mode === 'local') {
        if (!settings.localBaseUrl) throw new Error('未配置 Local Base URL');
        const resp = await postEvent(settings.localBaseUrl, event, 3000);
        if (resp.ok) {
          tabState.bridgeStatus = 'synced';
          tabState.bridgeMessage = '已同步 · Local';
        } else {
          tabState.bridgeStatus = 'failed';
          tabState.bridgeMessage = resp.status === 401 ? '401 认证失败' : `HTTP ${resp.status}`;
        }
      } else if (mode === 'remote') {
        if (!settings.remoteBaseUrl) throw new Error('未配置 Remote Base URL');
        const resp = await postEvent(settings.remoteBaseUrl, event, 4000);
        if (resp.ok) {
          tabState.bridgeStatus = 'synced';
          tabState.bridgeMessage = '已同步 · Remote';
        } else {
          tabState.bridgeStatus = 'failed';
          tabState.bridgeMessage = resp.status === 401 ? '401 认证失败' : `HTTP ${resp.status}`;
        }
      } else {
        // Auto 模式：优先 Local，短超时 600ms 后 fallback Remote
        let localSuccess = false;
        if (settings.localBaseUrl) {
          try {
            const resp = await postEvent(settings.localBaseUrl, event, 600);
            if (resp.ok) {
              localSuccess = true;
              tabState.bridgeStatus = 'synced';
              tabState.bridgeMessage = '已同步 · Local';
            } else if (resp.status === 401 || resp.status === 403) {
              // 鉴权失败不 fallback 远程
              tabState.bridgeStatus = 'failed';
              tabState.bridgeMessage = '401 认证失败';
              updateUi();
              return;
            }
          } catch {
            // Local 连接失败/超时，准备 fallback Remote
          }
        }

        if (!localSuccess) {
          if (settings.remoteBaseUrl) {
            try {
              const resp = await postEvent(settings.remoteBaseUrl, event, 4000);
              if (resp.ok) {
                tabState.bridgeStatus = 'synced';
                tabState.bridgeMessage = '已同步 · Remote';
              } else {
                tabState.bridgeStatus = 'failed';
                tabState.bridgeMessage = resp.status === 401 ? '401 认证失败' : `Remote HTTP ${resp.status}`;
              }
            } catch (err: unknown) {
              tabState.bridgeStatus = 'failed';
              tabState.bridgeMessage = (err instanceof Error && err.name === 'AbortError') ? 'Remote 超时' : 'Remote 网络错误';
            }
          } else {
            tabState.bridgeStatus = 'failed';
            tabState.bridgeMessage = 'Local 不可用且未配置 Remote';
          }
        }
      }
    } catch (err: unknown) {
      tabState.bridgeStatus = 'failed';
      tabState.bridgeMessage = err instanceof Error ? err.message : '发送异常';
    }

    updateUi();
  }

  function handleQuietWindow() {
    if (quietTimer !== null) {
      clearTimeout(quietTimer);
    }
    // 10 秒静默窗口无新流/工具活动，标记本轮完成
    quietTimer = window.setTimeout(() => {
      if (tabState.state === 'stream_idle') {
        tabState.state = 'completed';
        tabState.completedAt = Date.now();
        updateUi();
      }
    }, 10000);
  }

  // 监听来自 MAIN world 的 postMessage
  window.addEventListener('message', (event) => {
    if (event.source !== window || !event.data || event.data.source !== CT_OBSERVER_MESSAGE_SOURCE) {
      return;
    }

    const msg = event.data as PageHookMessage;
    const { type, payload } = msg;

    if (type === 'REQUEST_START') {
      const newTurnId = payload.turnId;
      if (!newTurnId) return;

      if (quietTimer !== null) {
        clearTimeout(quietTimer);
        quietTimer = null;
      }

      // 新 Turn 开始
      tabState.turnId = newTurnId;
      tabState.startedAt = payload.startedAt || Date.now();
      tabState.completedAt = null;
      tabState.requestedModel = payload.requestedModel || null;
      tabState.actualModel = null;
      tabState.state = 'active';
      tabState.lastActiveAt = Date.now();

      if (payload.conversationId) {
        tabState.conversationId = payload.conversationId;
      }

      updateUi();

      dispatchTurnEvent({
        schema_version: 1,
        observer_id: observerId,
        tab_id: tabId,
        event: 'turn_started',
        conversation_id: tabState.conversationId,
        turn_id: newTurnId,
        request_id: null,
        started_at: tabState.startedAt,
        completed_at: null,
        requested_model: tabState.requestedModel,
        actual_model: null,
      });
    } else if (type === 'SSE_CHUNK' || type === 'WS_FRAME') {
      tabState.lastActiveAt = Date.now();

      // 若处于 stream_idle，有新数据到来则恢复 active
      if (tabState.state === 'stream_idle' || tabState.state === 'completed') {
        tabState.state = 'active';
        if (quietTimer !== null) {
          clearTimeout(quietTimer);
          quietTimer = null;
        }
      }

      let hasUpdate = false;

      if (payload.conversationId && payload.conversationId !== tabState.conversationId) {
        tabState.conversationId = payload.conversationId;
        hasUpdate = true;
      }

      if (payload.requestId && payload.requestId !== tabState.requestId) {
        tabState.requestId = payload.requestId;
      }

      // 严格判定 actualModel 证据
      const modelEvidence = extractActualModel({
        requestedModel: tabState.requestedModel,
        resolvedModelSlug: payload.resolvedModelSlug || null,
        serverModelSlug: payload.serverModelSlug || null,
        responseModelSlug: payload.responseModelSlug || null,
        conversationId: payload.conversationId || null,
        requestId: payload.requestId || null,
        turnId: payload.turnId || null,
        isStreamDone: Boolean(payload.isStreamDone),
      });

      if (modelEvidence && modelEvidence !== tabState.actualModel) {
        tabState.actualModel = modelEvidence;
        hasUpdate = true;
      }

      if (hasUpdate && tabState.turnId) {
        updateUi();
        dispatchTurnEvent({
          schema_version: 1,
          observer_id: observerId,
          tab_id: tabId,
          event: 'turn_updated',
          conversation_id: tabState.conversationId,
          turn_id: tabState.turnId,
          request_id: tabState.requestId,
          started_at: tabState.startedAt || Date.now(),
          completed_at: null,
          requested_model: tabState.requestedModel,
          actual_model: tabState.actualModel,
        });
      }

      if (payload.isStreamDone) {
        tabState.state = 'stream_idle';
        handleQuietWindow();
        updateUi();
      }
    } else if (type === 'URL_CHANGE') {
      if (payload.conversationId && payload.conversationId !== tabState.conversationId) {
        tabState.conversationId = payload.conversationId;
        if (tabState.turnId) {
          dispatchTurnEvent({
            schema_version: 1,
            observer_id: observerId,
            tab_id: tabId,
            event: 'conversation_resolved',
            conversation_id: tabState.conversationId,
            turn_id: tabState.turnId,
            request_id: tabState.requestId,
            started_at: tabState.startedAt || Date.now(),
            completed_at: null,
            requested_model: tabState.requestedModel,
            actual_model: tabState.actualModel,
          });
        }
      }
    }
  });

  // 初次加载挂载 UI
  updateUi();
})();
