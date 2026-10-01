import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { TurnObserverOverlay } from '../src/overlay';
import type { TabTurnState } from '../src/types';

describe('overlay mounting during document_start', () => {
  let doc: EventTarget & {
    body: { appendChild: ReturnType<typeof vi.fn> } | null;
    createElement: ReturnType<typeof vi.fn>;
  };
  let render: ReturnType<typeof vi.spyOn>;
  const overlays: TurnObserverOverlay[] = [];

  beforeEach(() => {
    doc = Object.assign(new EventTarget(), {
      body: null,
      createElement: vi.fn(() => ({ style: {}, remove: vi.fn() })),
    });
    vi.stubGlobal('document', doc);
    vi.stubGlobal('window', Object.assign(new EventTarget(), {
      innerWidth: 1024,
      innerHeight: 768,
    }));
    // Isolate mounting from card rendering; use real event listener semantics.
    render = vi.spyOn(
      TurnObserverOverlay.prototype as unknown as { render: (state?: TabTurnState) => void },
      'render',
    ).mockImplementation(() => {});
  });

  afterEach(() => {
    for (const overlay of overlays.splice(0)) overlay.destroy();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  function createOverlay() {
    const overlay = new TurnObserverOverlay(null, false);
    overlays.push(overlay);
    return overlay;
  }

  it('waits for body without throwing and mounts once when DOM is ready', () => {
    expect(() => createOverlay()).not.toThrow();
    expect(doc.createElement).not.toHaveBeenCalled();
    expect(render).not.toHaveBeenCalled();

    doc.body = { appendChild: vi.fn() };
    doc.dispatchEvent(new Event('DOMContentLoaded'));
    doc.dispatchEvent(new Event('DOMContentLoaded'));

    expect(doc.body.appendChild).toHaveBeenCalledTimes(1);
    expect(doc.body.appendChild).toHaveBeenCalledWith(doc.createElement.mock.results[0].value);
    expect(render).toHaveBeenCalledTimes(1);
  });

  it('mounts immediately when body already exists', () => {
    doc.body = { appendChild: vi.fn() };
    createOverlay();

    expect(doc.body.appendChild).toHaveBeenCalledTimes(1);
    doc.dispatchEvent(new Event('DOMContentLoaded'));
    expect(doc.body.appendChild).toHaveBeenCalledTimes(1);
  });

  it('renders the latest state received before body exists', () => {
    const overlay = createOverlay();
    const state: TabTurnState = {
      tabId: 1,
      conversationId: 'conversation-1',
      turnId: 'turn-1',
      requestId: null,
      startedAt: null,
      completedAt: 1000,
      requestedModel: 'gpt-4o',
      actualModel: 'gpt-4o-mini',
      state: 'completed',
      bridgeStatus: 'synced',
      bridgeMessage: null,
      lastActiveAt: 1000,
    };
    overlay.updateState({ ...state, bridgeStatus: 'sending' });
    overlay.updateState(state);
    expect(render).not.toHaveBeenCalled();

    doc.body = { appendChild: vi.fn() };
    doc.dispatchEvent(new Event('DOMContentLoaded'));
    expect(render).toHaveBeenCalledWith(state);
  });

  it('cancels pending mounting when destroyed before DOM is ready', () => {
    createOverlay().destroy();
    doc.body = { appendChild: vi.fn() };
    doc.dispatchEvent(new Event('DOMContentLoaded'));

    expect(doc.body.appendChild).not.toHaveBeenCalled();
    expect(render).not.toHaveBeenCalled();
  });
});
