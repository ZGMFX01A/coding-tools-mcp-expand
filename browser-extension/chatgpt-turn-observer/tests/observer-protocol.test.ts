import { describe, expect, it } from 'vitest';
import { normalizeObserverBaseUrl, validateObserverStatusPayload } from '../src/observer-protocol';

describe('observer protocol helpers', () => {
  it('normalizes a copied full MCP endpoint to the internal API base', () => {
    expect(normalizeObserverBaseUrl('https://example.test/mcp/')).toBe('https://example.test');
    expect(normalizeObserverBaseUrl('https://example.test/prefix/mcp')).toBe('https://example.test/prefix');
    expect(normalizeObserverBaseUrl('http://127.0.0.1:28766')).toBe('http://127.0.0.1:28766');
  });

  it('requires the Observer service identity and workspace id', () => {
    expect(
      validateObserverStatusPayload({
        ok: true,
        service: 'chatgpt_turn_observer',
        workspace_id: 'ws-1',
        turn_budget: { warning_after_seconds: 1380, hard_stop_after_seconds: 1500 },
      }),
    ).toMatchObject({ ok: true, workspaceId: 'ws-1', warningAfterMs: 1380000, hardStopAfterMs: 1500000 });
    expect(validateObserverStatusPayload({ ok: true, workspace_id: 'ws-1' }).ok).toBe(false);
    expect(
      validateObserverStatusPayload({ ok: true, service: 'other', workspace_id: 'ws-1' }).ok,
    ).toBe(false);
  });
});
