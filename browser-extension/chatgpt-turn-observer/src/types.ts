export type BridgeMode = 'auto' | 'local' | 'remote';

export const DEFAULT_LOCAL_PORT = 28766;

export type BridgeStatus =
  | 'idle'
  | 'not_configured'
  | 'connecting'
  | 'sending'
  | 'synced'
  | 'failed';

export type TurnLifecycleState =
  | 'idle'
  | 'turn_starting'
  | 'active'
  | 'stream_idle'
  | 'completed';

export type EventKind =
  | 'turn_started'
  | 'turn_updated'
  | 'stream_completed'
  | 'conversation_resolved'
  | 'turn_closed';

export interface BrowserTurnEvent {
  schema_version: 1;
  event_id: string;
  tab_instance_id: string;
  observer_id: string;
  tab_id: number;
  sequence: number;
  event: EventKind;
  workspace_id: string;
  conversation_id: string | null;
  turn_id: string;
  request_id: string | null;
  started_at: number;
  completed_at: number | null;
  requested_model: string | null;
  actual_model: string | null;
}

export interface ObserverSettings {
  schemaVersion: number;
  bridgeMode: BridgeMode;
  localBaseUrl: string;
  remoteBaseUrl: string;
  bridgeToken: string;
  overlayPosition: { x: number; y: number } | null;
  overlayCollapsed: boolean;
}

export const DEFAULT_SETTINGS: ObserverSettings = {
  schemaVersion: 1,
  bridgeMode: 'auto',
  localBaseUrl: `http://127.0.0.1:${DEFAULT_LOCAL_PORT}`,
  remoteBaseUrl: '',
  bridgeToken: '',
  overlayPosition: null,
  overlayCollapsed: false,
};

export interface RouteEvidence {
  requestedModel: string | null;
  resolvedModelSlug: string | null;
  serverModelSlug: string | null;
  responseModelSlug: string | null;
  conversationId: string | null;
  requestId: string | null;
  turnId: string | null;
  captureId?: string | null;
  isStreamDone: boolean;
}

export const EMPTY_ROUTE_EVIDENCE: RouteEvidence = {
  requestedModel: null,
  resolvedModelSlug: null,
  serverModelSlug: null,
  responseModelSlug: null,
  conversationId: null,
  requestId: null,
  turnId: null,
  captureId: null,
  isStreamDone: false,
};

export type TurnStartDecision =
  | {
      type: 'NEW_USER_TURN';
      userMessageId: string;
      requestedModel: string | null;
      conversationId: string | null;
      parentMessageId: string | null;
    }
  | {
      type: 'SAME_TURN_CONTINUATION';
      userMessageId: string;
      conversationId: string | null;
    }
  | {
      type: 'NON_TURN_REQUEST';
      reason:
        | 'UPLOAD_ONLY'
        | 'COPY_TELEMETRY'
        | 'NO_USER_MESSAGE'
        | 'SAME_USER_MESSAGE'
        | 'UNRELATED_ENDPOINT'
        | 'INVALID_STRUCTURE';
    };

export interface TabTurnState {
  tabId: number;
  conversationId: string | null;
  turnId: string | null;
  activeCaptureId?: string | null;
  requestId: string | null;
  startedAt: number | null;
  completedAt: number | null;
  requestedModel: string | null;
  actualModel: string | null;
  state: TurnLifecycleState;
  bridgeStatus: BridgeStatus;
  bridgeMessage: string | null;
  lastActiveAt: number;
}

/** postMessage 从 MAIN world 到 ISOLATED world 的通信消息体 */
export const CT_OBSERVER_MESSAGE_SOURCE = 'CT_TURN_OBSERVER_PAGE_HOOK';

export interface PageHookMessage {
  source: typeof CT_OBSERVER_MESSAGE_SOURCE;
  type: 'REQUEST_START' | 'SSE_CHUNK' | 'WS_FRAME' | 'URL_CHANGE';
  payload: {
    captureId?: string | null;
    turnId?: string | null;
    conversationId?: string | null;
    requestId?: string | null;
    requestedModel?: string | null;
    resolvedModelSlug?: string | null;
    serverModelSlug?: string | null;
    responseModelSlug?: string | null;
    isStreamDone?: boolean;
    url?: string;
    startedAt?: number;
  };
}
