import { describe, it, expect, vi, beforeEach } from 'vitest';
import { DEFAULT_SETTINGS, type BrowserTurnEvent, type ObserverSettings } from '../src/types';

describe('bridge dispatch logic', () => {
  let mockFetch: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockFetch = vi.fn();
    global.fetch = mockFetch;
  });

  const sampleEvent: BrowserTurnEvent = {
    schema_version: 1,
    observer_id: 'obs-001',
    tab_id: 101,
    event: 'turn_started',
    conversation_id: 'conv-001',
    turn_id: 'turn-001',
    request_id: null,
    started_at: 1000,
    completed_at: null,
    requested_model: 'gpt-4o',
    actual_model: null,
  };

  async function simulateDispatch(
    settings: ObserverSettings,
    event: BrowserTurnEvent
  ): Promise<{ status: string; message: string | null }> {
    if (!settings.bridgeToken) {
      return { status: 'not_configured', message: '未配置 Token' };
    }

    const postTo = async (baseUrl: string, timeoutMs: number) => {
      const url = `${baseUrl.trim().replace(/\/+$/, '')}/internal/chatgpt-turn-event`;
      return fetch(url, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          Authorization: `Bearer ${settings.bridgeToken}`,
        },
        body: JSON.stringify(event),
      });
    };

    if (settings.bridgeMode === 'local') {
      const resp = await postTo(settings.localBaseUrl, 3000);
      if (resp.ok) return { status: 'synced', message: '已同步 · Local' };
      return { status: 'failed', message: resp.status === 401 ? '401 认证失败' : `HTTP ${resp.status}` };
    } else if (settings.bridgeMode === 'remote') {
      const resp = await postTo(settings.remoteBaseUrl, 4000);
      if (resp.ok) return { status: 'synced', message: '已同步 · Remote' };
      return { status: 'failed', message: resp.status === 401 ? '401 认证失败' : `HTTP ${resp.status}` };
    } else {
      // Auto
      let localOk = false;
      try {
        const resp = await postTo(settings.localBaseUrl, 600);
        if (resp.ok) {
          return { status: 'synced', message: '已同步 · Local' };
        } else if (resp.status === 401 || resp.status === 403) {
          // 401 严禁 fallback
          return { status: 'failed', message: '401 认证失败' };
        }
      } catch {
        // network error / timeout -> fallback to remote
      }

      if (settings.remoteBaseUrl) {
        try {
          const resp = await postTo(settings.remoteBaseUrl, 4000);
          if (resp.ok) return { status: 'synced', message: '已同步 · Remote' };
          return { status: 'failed', message: resp.status === 401 ? '401 认证失败' : `Remote HTTP ${resp.status}` };
        } catch {
          return { status: 'failed', message: 'Remote 网络错误' };
        }
      }
      return { status: 'failed', message: 'Local 不可用且未配置 Remote' };
    }
  }

  it('handles local mode success', async () => {
    mockFetch.mockResolvedValueOnce(new Response(JSON.stringify({ ok: true }), { status: 200 }));
    const res = await simulateDispatch(
      { ...DEFAULT_SETTINGS, bridgeMode: 'local', bridgeToken: 'secret' },
      sampleEvent
    );
    expect(res.status).toBe('synced');
    expect(res.message).toBe('已同步 · Local');
  });

  it('handles auto mode fallback to remote when local fails with connection error', async () => {
    mockFetch
      .mockRejectedValueOnce(new Error('Connection refused'))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true }), { status: 200 }));

    const res = await simulateDispatch(
      {
        ...DEFAULT_SETTINGS,
        bridgeMode: 'auto',
        bridgeToken: 'secret',
        remoteBaseUrl: 'https://tunnel.example.com',
      },
      sampleEvent
    );
    expect(res.status).toBe('synced');
    expect(res.message).toBe('已同步 · Remote');
    expect(mockFetch).toHaveBeenCalledTimes(2);
  });

  it('does NOT fallback to remote when local returns 401 Unauthorized', async () => {
    mockFetch.mockResolvedValueOnce(new Response('Unauthorized', { status: 401 }));

    const res = await simulateDispatch(
      {
        ...DEFAULT_SETTINGS,
        bridgeMode: 'auto',
        bridgeToken: 'wrong_secret',
        remoteBaseUrl: 'https://tunnel.example.com',
      },
      sampleEvent
    );
    expect(res.status).toBe('failed');
    expect(res.message).toBe('401 认证失败');
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });
});
