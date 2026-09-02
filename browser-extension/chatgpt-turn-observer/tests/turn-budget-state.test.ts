import { describe, expect, it, vi } from 'vitest';
import { applyConversationRouteChange, startObservedTurn } from '../src/bridge';
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

  it('keeps an active new-conversation turn when its route receives the assigned conversation id', () => {
    const state = stateWithStoppedBudget();
    const closeCurrentTurn = vi.fn();
    const reportConversationResolved = vi.fn();
    state.conversationId = null;
    state.turnId = 'active-turn';
    state.activeCaptureId = 'active-capture';
    state.startedAt = 123;
    state.requestedModel = 'gpt-5.6-thinking';
    state.actualModel = 'gpt-5.6-sol';
    state.state = 'active';
    state.budgetStatus = 'normal';

    const changed = applyConversationRouteChange(
      state,
      'assigned-conversation',
      closeCurrentTurn,
      reportConversationResolved,
    );

    expect(changed).toBe(true);
    expect(closeCurrentTurn).not.toHaveBeenCalled();
    expect(reportConversationResolved).toHaveBeenCalledTimes(1);
    expect(state.conversationId).toBe('assigned-conversation');
    expect(state.turnId).toBe('active-turn');
    expect(state.startedAt).toBe(123);
    expect(state.requestedModel).toBe('gpt-5.6-thinking');
    expect(state.actualModel).toBe('gpt-5.6-sol');
  });

  it('closes an active turn when navigation switches between established conversations', () => {
    const state = stateWithStoppedBudget();
    const closeCurrentTurn = vi.fn();
    state.turnId = 'active-turn';
    state.state = 'active';

    applyConversationRouteChange(state, 'different-conversation', closeCurrentTurn);

    expect(closeCurrentTurn).toHaveBeenCalledTimes(1);
    expect(state.conversationId).toBe('different-conversation');
  });
});
