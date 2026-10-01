import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

describe('passive observer', () => {
  it('contains no conversation deadline or remote stop control', () => {
    const bridge = readFileSync(new URL('../src/bridge.ts', import.meta.url), 'utf8');
    expect(bridge).not.toContain('hardStopTimer');
    expect(bridge).not.toContain('STOP_TURN');
    expect(bridge).not.toContain('scheduleTurnTimers');
    expect(bridge).not.toContain('/chatgpt-turn-observer/control');
  });
  it('never substitutes the page request signal or closes its sockets', () => {
    const hook = readFileSync(new URL('../src/page-hook.ts', import.meta.url), 'utf8');
    expect(hook).not.toContain('addStopSignal');
    expect(hook).not.toContain('controller.abort()');
    expect(hook).not.toContain('socket.close(');
  });
});
