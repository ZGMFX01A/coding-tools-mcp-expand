import { describe, it, expect } from 'vitest';
import {
  conversationIdFromUrl,
  extractActualModel,
  formatModelDisplayName,
  parseConversationRequest,
  parseSseChunk,
  parseWebSocketFrame,
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

    it('handles invalid json gracefully', () => {
      const res = parseConversationRequest('invalid json');
      expect(res.turnId).toBeNull();
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

    it('NEVER uses requestedModel as actualModel when response evidence is absent', () => {
      const evidence = {
        ...EMPTY_ROUTE_EVIDENCE,
        requestedModel: 'gpt-4o',
        resolvedModelSlug: null,
        serverModelSlug: null,
        responseModelSlug: null,
      };

      const actual = extractActualModel(evidence);
      expect(actual).toBeNull();
    });
  });

  describe('parseWebSocketFrame', () => {
    it('parses sse from encoded_item inside websocket frame', () => {
      const frame = JSON.stringify([
        {
          payload: {
            payload: {
              encoded_item: 'data: {"metadata": {"resolved_model_slug": "gpt-4o"}}\ndata: [DONE]\n',
            },
          },
        },
      ]);

      const evidence = parseWebSocketFrame(frame);
      expect(evidence.resolvedModelSlug).toBe('gpt-4o');
      expect(evidence.isStreamDone).toBe(true);
      expect(extractActualModel(evidence)).toBe('gpt-4o');
    });
  });
});
