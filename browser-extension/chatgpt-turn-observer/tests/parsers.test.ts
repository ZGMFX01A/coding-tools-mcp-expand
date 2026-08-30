import { describe, it, expect } from 'vitest';
import {
  conversationIdFromUrl,
  extractActualModel,
  formatModelDisplayName,
  isConversationEndpoint,
  parseConversationRequest,
  parseSseChunk,
  parseWebSocketFrame,
  SseStreamParser,
} from '../src/parsers';
import { EMPTY_ROUTE_EVIDENCE } from '../src/types';

describe('parsers', () => {
  describe('conversationIdFromUrl', () => {
    it('extracts conversation id from standard url', () => {
      expect(conversationIdFromUrl('https://chatgpt.com/c/67b93198-abc-123')).toBe('67b93198-abc-123');
      expect(conversationIdFromUrl('/c/67b93198-abc-123?model=gpt-4o')).toBe('67b93198-abc-123');
    });

    it('extracts conversation id from gizmo url', () => {
      expect(conversationIdFromUrl('https://chatgpt.com/g/g-abc1234/c/67b93198-abc-123')).toBe('67b93198-abc-123');
    });

    it('returns null for non-conversation urls', () => {
      expect(conversationIdFromUrl('https://chatgpt.com/')).toBeNull();
      expect(conversationIdFromUrl('https://chatgpt.com/g/g-abc1234')).toBeNull();
      expect(conversationIdFromUrl('not a url')).toBeNull();
    });
  });

  describe('isConversationEndpoint', () => {
    it('matches standard and current chatgpt conversation endpoints', () => {
      expect(isConversationEndpoint('/backend-api/conversation')).toBe(true);
      expect(isConversationEndpoint('/backend-api/conversations')).toBe(true);
      expect(isConversationEndpoint('/backend-api/lat/r')).toBe(true);
      expect(isConversationEndpoint('/backend-api/f/r')).toBe(true);
      expect(isConversationEndpoint('/backend-api/f/conversation')).toBe(true);
      expect(isConversationEndpoint('/backend-anon/conversation')).toBe(true);
      expect(isConversationEndpoint('https://chatgpt.com/backend-api/conversation')).toBe(true);
      expect(isConversationEndpoint('https://chatgpt.com/backend-api/lat/r?v=1')).toBe(true);
    });

    it('rejects unrelated endpoints', () => {
      expect(isConversationEndpoint('/backend-api/me')).toBe(false);
      expect(isConversationEndpoint('/backend-api/models')).toBe(false);
      expect(isConversationEndpoint('/backend-api/settings')).toBe(false);
      expect(isConversationEndpoint('')).toBe(false);
    });
  });

  describe('parseConversationRequest', () => {
    it('extracts turnId, conversationId, and requestedModel', () => {
      const payload = JSON.stringify({
        action: 'next',
        messages: [
          {
            id: 'msg-turn-001',
            author: { role: 'user' },
            content: { content_type: 'text', parts: ['hello'] },
          },
        ],
        conversation_id: 'conv-123',
        parent_message_id: 'parent-000',
        model: 'gpt-4o',
      });

      const res = parseConversationRequest(payload);
      expect(res.turnId).toBe('msg-turn-001');
      expect(res.conversationId).toBe('conv-123');
      expect(res.requestedModel).toBe('gpt-4o');
      expect(res.parentMessageId).toBe('parent-000');
    });

    it('handles new conversation with null conversation_id', () => {
      const payload = JSON.stringify({
        action: 'next',
        messages: [{ id: 'msg-turn-new' }],
        model: 'o3-mini',
      });

      const res = parseConversationRequest(payload);
      expect(res.turnId).toBe('msg-turn-new');
      expect(res.conversationId).toBeNull();
      expect(res.requestedModel).toBe('o3-mini');
    });

    it('extracts turnId from alternative fields if messages array is empty', () => {
      const payload = JSON.stringify({
        client_message_id: 'client-msg-777',
        conversation_id: 'conv-456',
        model: 'o3',
      });

      const res = parseConversationRequest(payload);
      expect(res.turnId).toBe('client-msg-777');
      expect(res.conversationId).toBe('conv-456');
      expect(res.requestedModel).toBe('o3');
    });

    it('generates a valid fallback UUID turnId if no ID field exists', () => {
      const payload = JSON.stringify({
        model: 'gpt-4o',
      });

      const res = parseConversationRequest(payload);
      expect(typeof res.turnId).toBe('string');
      expect(res.turnId.length).toBeGreaterThan(10);
      expect(res.requestedModel).toBe('gpt-4o');
    });

    it('handles invalid json gracefully by returning a non-null generated turnId', () => {
      const res = parseConversationRequest('invalid json');
      expect(typeof res.turnId).toBe('string');
      expect(res.turnId.length).toBeGreaterThan(10);
      expect(res.conversationId).toBeNull();
    });
  });

  describe('parseSseChunk & actual model extraction', () => {
    it('parses resolved_model_slug and [DONE]', () => {
      const chunk = `
data: {"message": {"id": "resp-1", "author": {"role": "assistant"}}, "conversation_id": "conv-real-999"}
data: {"metadata": {"resolved_model_slug": "gpt-5.6-sol-preview", "request_id": "req-999"}}
data: [DONE]
`;
      const evidence = parseSseChunk(chunk);
      expect(evidence.conversationId).toBe('conv-real-999');
      expect(evidence.resolvedModelSlug).toBe('gpt-5.6-sol-preview');
      expect(evidence.requestId).toBe('req-999');
      expect(evidence.isStreamDone).toBe(true);

      const actual = extractActualModel(evidence);
      expect(actual).toBe('gpt-5.6-sol-preview');
      expect(formatModelDisplayName(actual)).toBe('GPT-5.6 Sol');
    });

    it('parses server_ste_metadata model_slug', () => {
      const chunk = `
data: {"type": "server_ste_metadata", "metadata": {"model_slug": "o3-mini"}}
`;
      const evidence = parseSseChunk(chunk);
      expect(evidence.serverModelSlug).toBe('o3-mini');
      expect(extractActualModel(evidence)).toBe('o3-mini');
      expect(formatModelDisplayName(extractActualModel(evidence))).toBe('o3-mini');
    });

    it('parses assistant message author metadata model_slug', () => {
      const chunk = `
data: {"author": {"role": "assistant"}, "metadata": {"model_slug": "gpt-4o"}}
`;
      const evidence = parseSseChunk(chunk);
      expect(evidence.responseModelSlug).toBe('gpt-4o');
      expect(extractActualModel(evidence)).toBe('gpt-4o');
    });

    it('never allows requestedModel to spoof actualModel', () => {
      const evidence = {
        ...EMPTY_ROUTE_EVIDENCE,
        requestedModel: 'gpt-4o',
        resolvedModelSlug: null,
        serverModelSlug: null,
        responseModelSlug: null,
      };
      expect(extractActualModel(evidence)).toBeNull();
    });

    it('strictly respects evidence priority: resolved > server_ste > response', () => {
      const evidence = {
        ...EMPTY_ROUTE_EVIDENCE,
        resolvedModelSlug: 'model-resolved',
        serverModelSlug: 'model-server-ste',
        responseModelSlug: 'model-response',
      };
      expect(extractActualModel(evidence)).toBe('model-resolved');

      const evidence2 = {
        ...EMPTY_ROUTE_EVIDENCE,
        serverModelSlug: 'model-server-ste',
        responseModelSlug: 'model-response',
      };
      expect(extractActualModel(evidence2)).toBe('model-server-ste');
    });
  });

  describe('SseStreamParser (Cross-chunk streaming)', () => {
    it('correctly parses SSE event split across 2 chunks', () => {
      const parser = new SseStreamParser();

      // Chunk 1 包含不完整 JSON
      const chunk1 = 'data: {"metadata": {"resolved_model_';
      const ev1 = parser.feed(chunk1);
      expect(ev1.resolvedModelSlug).toBeNull();

      // Chunk 2 补齐 JSON 并换行
      const chunk2 = 'slug": "o3-mini", "request_id": "req-split-1"}}\n';
      const ev2 = parser.feed(chunk2);
      expect(ev2.resolvedModelSlug).toBe('o3-mini');
      expect(ev2.requestId).toBe('req-split-1');
    });

    it('correctly parses SSE event split across 3 chunks', () => {
      const parser = new SseStreamParser();

      const chunk1 = 'data: {"conversation_id": "conv-split-';
      const ev1 = parser.feed(chunk1);
      expect(ev1.conversationId).toBeNull();

      const chunk2 = '333", "metadata": {"server_ste_metadata": {"model_';
      const ev2 = parser.feed(chunk2);
      expect(ev2.conversationId).toBeNull();

      const chunk3 = 'slug": "gpt-4o"}}}\ndata: [DONE]\n';
      const ev3 = parser.feed(chunk3);
      expect(ev3.conversationId).toBe('conv-split-333');
      expect(ev3.serverModelSlug).toBe('gpt-4o');
      expect(ev3.isStreamDone).toBe(true);
    });

    it('flushes trailing line when stream completes without newline', () => {
      const parser = new SseStreamParser();
      parser.feed('data: {"metadata": {"resolved_model_slug": "gpt-5.6-sol-preview"}}');
      const flushed = parser.flush();
      expect(flushed.resolvedModelSlug).toBe('gpt-5.6-sol-preview');
    });
  });

  describe('parseWebSocketFrame', () => {
    it('extracts model evidence from outer/inner websocket envelope', () => {
      const wsFrame = JSON.stringify([
        {
          payload: {
            payload: {
              encoded_item: 'data: {"metadata": {"resolved_model_slug": "o3-mini", "conversation_id": "conv-ws-1"}}\ndata: [DONE]\n',
            },
          },
        },
      ]);

      const evidence = parseWebSocketFrame(wsFrame);
      expect(evidence.resolvedModelSlug).toBe('o3-mini');
      expect(evidence.conversationId).toBe('conv-ws-1');
      expect(evidence.isStreamDone).toBe(true);
    });
  });
});
