import { afterEach, describe, it, expect, vi } from 'vitest';
import { BridgeOutbox, type OutboxItem } from '../src/bridge';
import { type BrowserTurnEvent } from '../src/types';

describe('BridgeOutbox real queue behavior and retry logic', () => {
  const sampleEvent: BrowserTurnEvent = {
    schema_version: 1,
    event_id: 'evt-test-1234',
    tab_instance_id: 'tab-inst-999',
    observer_id: 'obs-001',
    tab_id: 101,
    sequence: 1,
    event: 'turn_started',
    workspace_id: 'ws_demo',
    conversation_id: 'conv-001',
    turn_id: 'turn-001',
    request_id: null,
    started_at: 1000,
    completed_at: null,
    requested_model: 'gpt-4o',
    actual_model: null,
  };

  afterEach(() => {
    vi.useRealTimers();
  });

  it('dequeues item when sendFn succeeds with 200', async () => {
    const sendFn = vi.fn().mockResolvedValue({ ok: true, status: 200, retryable: false });
    const outbox = new BridgeOutbox(sendFn);

    outbox.enqueue(sampleEvent);
    expect(outbox.getQueueLength()).toBe(1);

    await outbox.process();
    expect(sendFn).toHaveBeenCalledTimes(1);
    expect(outbox.getQueueLength()).toBe(0);
  });

  it('dequeues item immediately without retry on non-retryable 400/403/422 business rejection', async () => {
    const sendFn = vi.fn().mockResolvedValue({ ok: false, status: 400, retryable: false, error: 'Bad Request' });
    const outbox = new BridgeOutbox(sendFn);

    outbox.enqueue(sampleEvent);
    await outbox.process();

    expect(sendFn).toHaveBeenCalledTimes(1);
    expect(outbox.getQueueLength()).toBe(0);
  });

  it('retains head item and reuses same event_id/sequence upon retryable network error', async () => {
    const sendFn = vi.fn().mockResolvedValue({ ok: false, status: 500, retryable: true, error: 'Network timeout' });
    const outbox = new BridgeOutbox(sendFn);

    outbox.enqueue(sampleEvent);
    await outbox.process();

    expect(sendFn).toHaveBeenCalledTimes(1);
    // 可重试错误保留在队头
    expect(outbox.getQueueLength()).toBe(1);
    const head = outbox.getQueue()[0];
    expect(head.attempts).toBe(1);
    expect(head.event.event_id).toBe('evt-test-1234');
    expect(head.event.sequence).toBe(1);
    expect(head.nextRetryAt).toBeGreaterThan(Date.now() - 10);
  });

  it('drops item after exceeding maximum 5 retry attempts', async () => {
    const sendFn = vi.fn().mockResolvedValue({ ok: false, status: 503, retryable: true, error: 'Service Unavailable' });
    const outbox = new BridgeOutbox(sendFn);

    outbox.enqueue(sampleEvent);

    for (let i = 1; i <= 5; i++) {
      // 模拟时间到达 nextRetryAt
      const q = outbox.getQueue();
      if (q.length > 0) {
        (q[0] as { nextRetryAt: number }).nextRetryAt = 0;
      }
      await outbox.process();
    }

    expect(sendFn).toHaveBeenCalledTimes(5);
    // 超过 5 次后出队丢弃
    expect(outbox.getQueueLength()).toBe(0);
  });

  it('respects maximum queue capacity (128) by evicting oldest item', () => {
    const sendFn = vi.fn().mockResolvedValue({ ok: false, retryable: true });
    const outbox = new BridgeOutbox(sendFn);

    for (let i = 1; i <= 130; i++) {
      outbox.enqueue({
        ...sampleEvent,
        event_id: `evt-${i}`,
        sequence: i,
        event: 'turn_updated',
      });
    }

    expect(outbox.getQueueLength()).toBe(128);
    expect(outbox.getQueue()[0].event.event_id).toBe('evt-3');
  });

  it('does not evict lifecycle events when the queue is full', () => {
    const sendFn = vi.fn().mockResolvedValue({ ok: false, retryable: true });
    const outbox = new BridgeOutbox(sendFn);

    for (let i = 1; i <= 128; i++) {
      outbox.enqueue({
        ...sampleEvent,
        event_id: `evt-${i}`,
        sequence: i,
        event: 'turn_started',
      });
    }

    expect(outbox.enqueue({ ...sampleEvent, event_id: 'evt-overflow', sequence: 129 })).toBe(false);
    expect(outbox.getQueueLength()).toBe(128);
    expect(outbox.getQueue()[0].event.event_id).toBe('evt-1');
  });

  it('converts an unexpected sender exception into a retryable result', async () => {
    const sendFn = vi.fn().mockRejectedValue(new Error('transport crashed'));
    const outbox = new BridgeOutbox(sendFn);

    outbox.enqueue(sampleEvent);
    await outbox.process();

    expect(outbox.getQueueLength()).toBe(1);
    expect(outbox.getQueue()[0].attempts).toBe(1);
  });

  it('restores pending events and persists removal after a successful send', async () => {
    const saved: OutboxItem[][] = [];
    const persistence = {
      load: vi.fn().mockResolvedValue([{ event: sampleEvent, attempts: 1, nextRetryAt: 0 }]),
      save: vi.fn(async (items: readonly OutboxItem[]) => {
        saved.push([...items]);
      }),
    };
    const sendFn = vi.fn().mockResolvedValue({ ok: true, status: 200, retryable: false });
    const outbox = new BridgeOutbox(sendFn, undefined, persistence);

    await outbox.restore();
    await outbox.process();

    expect(persistence.load).toHaveBeenCalledTimes(1);
    expect(sendFn).toHaveBeenCalledTimes(1);
    expect(saved.at(-1)).toEqual([]);
  });

  it('batches persistence for ordinary stream updates while retaining the final queue state', async () => {
    vi.useFakeTimers();
    const saved: OutboxItem[][] = [];
    const persistence = {
      load: vi.fn().mockResolvedValue([]),
      save: vi.fn(async (items: readonly OutboxItem[]) => {
        saved.push([...items]);
      }),
    };
    const sendFn = vi.fn().mockResolvedValue({ ok: true, status: 200, retryable: false });
    const outbox = new BridgeOutbox(sendFn, undefined, persistence);

    outbox.enqueue({ ...sampleEvent, event: 'turn_updated' });
    await outbox.process();

    expect(persistence.save).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(749);
    expect(persistence.save).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(saved.at(-1)).toEqual([]);
  });

  it('persists critical lifecycle events immediately', async () => {
    const persistence = {
      load: vi.fn().mockResolvedValue([]),
      save: vi.fn().mockResolvedValue(undefined),
    };
    const sendFn = vi.fn().mockResolvedValue({ ok: false, status: 500, retryable: true });
    const outbox = new BridgeOutbox(sendFn, undefined, persistence);

    outbox.enqueue(sampleEvent);
    await outbox.process();

    expect(persistence.save).toHaveBeenCalled();
  });
});
