import { describe, expect, it } from 'vitest';
import { startObservedTurn } from '../src/bridge';
import type { TabTurnState } from '../src/types';

function stateWithStoppedBudget(): TabTurnState {
  return {
    tabId: 1,
    conversationId: 'previous-conversation',
    turnId: null,
    activeCaptureId: null,
    requestId: null,
    startedAt: null,
    completedAt: Date.now(),
    requestedModel: 'gpt-5.6-thinking',
    actualModel: 'gpt-5.6-thinking',
    state: 'idle',
    bridgeStatus: 'synced',
    bridgeMessage: '本轮已达到 25 分钟上限，正在停止网页生成',
    budgetStatus: 'stopped',
    lastActiveAt: Date.now(),
  };
}

describe('turn budget UI state', () => {
  it('clears a previous hard-stop status when a new conversation turn begins', () => {
    const state = stateWithStoppedBudget();

    startObservedTurn(state, {
      captureId: 'new-capture',
      turnId: 'new-turn',
      conversationId: 'new-conversation',
      requestedModel: 'gpt-5.6-thinking',
      startedAt: 123,
    });

    expect(state.budgetStatus).toBe('normal');
    expect(state.turnId).toBe('new-turn');
    expect(state.conversationId).toBe('new-conversation');
    expect(state.state).toBe('turn_starting');
  });
});
