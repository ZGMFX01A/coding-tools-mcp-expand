import { describe, it, expect } from 'vitest';
import {
  classifyEndpoint,
  conversationIdFromUrl,
  extractActualModel,
  formatModelDisplayName,
  parseConversationCorrelation,
  parseSseChunk,
  parseWebSocketFrame,
  SseStreamParser,
} from '../src/parsers';
import { EMPTY_ROUTE_EVIDENCE } from '../src/types';

describe('parsers following chatgpt-route-inspector architecture', () => {
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

  describe('classifyEndpoint (Strict White-listing matching Route Inspector)', () => {
    it('classifies conversation stream endpoints as conversation_stream', () => {
      expect(classifyEndpoint('/backend-api/conversation').kind).toBe('conversation_stream');
      expect(classifyEndpoint('/backend-api/conversations').kind).toBe('conversation_stream');
      expect(classifyEndpoint('/backend-api/f/conversation').kind).toBe('conversation_stream');
      expect(classifyEndpoint('/backend-api/f/conversations').kind).toBe('conversation_stream');
      expect(classifyEndpoint('/backend-anon/conversation').kind).toBe('conversation_stream');
      expect(classifyEndpoint('https://chatgpt.com/backend-api/f/conversation').kind).toBe('conversation_stream');
    });

    it('classifies conversation history reload as conversation_record', () => {
      const match = classifyEndpoint('/backend-api/conversation/67b93198-1234');
      expect(match.kind).toBe('conversation_record');
      expect(match.conversationId).toBe('67b93198-1234');
    });

    it('classifies copy telemetry, file uploads, synthesize and lat/r as other', () => {
      // 复制 Assistant 回答产生的打点接口
      expect(classifyEndpoint('/ces/v1/t').kind).toBe('other');
      expect(classifyEndpoint('/backend-api/lat/r').kind).toBe('other');
      expect(classifyEndpoint('/backend-api/telemetry').kind).toBe('other');
      // 文件与图片上传接口
      expect(classifyEndpoint('/backend-api/files').kind).toBe('other');
      expect(classifyEndpoint('/backend-api/files/upload').kind).toBe('other');
      expect(classifyEndpoint('/backend-api/attachment').kind).toBe('other');
      // 语音合成与设置
      expect(classifyEndpoint('/backend-api/synthesize').kind).toBe('other');
      expect(classifyEndpoint('/backend-api/settings').kind).toBe('other');
      expect(classifyEndpoint('/backend-api/models').kind).toBe('other');
    });
  });

  describe('parseConversationCorrelation (P0-1 Strict User Turn Semantics)', () => {
    it('extracts inputMessageId and confirms isNewUserTurn=true for genuine user send', () => {
      const payload = {
        action: 'next',
        messages: [
          {
            id: 'msg-user-submit-101',
            author: { role: 'user' },
            content: { content_type: 'text', parts: ['Hello Coding Tools'] },
          },
        ],
        conversation_id: 'conv-real-99',
        parent_message_id: 'parent-001',
        model: 'gpt-4o',
      };

      const correlation = parseConversationCorrelation(payload);
      expect(correlation).not.toBeNull();
      expect(correlation?.inputMessageId).toBe('msg-user-submit-101');
      expect(correlation?.conversationId).toBe('conv-real-99');
      expect(correlation?.parentMessageId).toBe('parent-001');
      expect(correlation?.requestedModel).toBe('gpt-4o');
      expect(correlation?.isNewUserTurn).toBe(true);
    });

    it('identifies isNewUserTurn=false when action is missing', () => {
      const payload = {
        messages: [
          {
            id: 'msg-user-1',
            author: { role: 'user' },
          },
        ],
        conversation_id: 'conv-99',
      };
      const correlation = parseConversationCorrelation(payload);
      expect(correlation).not.toBeNull();
      expect(correlation?.inputMessageId).toBeNull();
      expect(correlation?.isNewUserTurn).toBe(false);
    });

    it('identifies isNewUserTurn=false for continuation requests', () => {
      const payload = {
        action: 'continue',
        conversation_id: 'conv-real-99',
        parent_message_id: 'parent-001',
        model: 'gpt-4o',
        messages: [
          {
            id: 'msg-user-history-1',
            author: { role: 'user' },
            content: { content_type: 'text', parts: ['Previous user text'] },
          },
        ],
      };
      const correlation = parseConversationCorrelation(payload);
      expect(correlation).not.toBeNull();
      expect(correlation?.inputMessageId).toBeNull();
      expect(correlation?.isNewUserTurn).toBe(false);
    });

    it('identifies isNewUserTurn=false for retry and fork requests', () => {
      const payloadRetry = {
        action: 'retry',
        conversation_id: 'conv-real-99',
        client_message_id: 'msg-retry-1',
      };
      expect(parseConversationCorrelation(payloadRetry)?.isNewUserTurn).toBe(false);

      const payloadFork = {
        action: 'fork',
        conversation_id: 'conv-real-99',
        client_message_id: 'msg-fork-1',
      };
      expect(parseConversationCorrelation(payloadFork)?.isNewUserTurn).toBe(false);
    });

    it('identifies isNewUserTurn=false when request only contains conversation_id', () => {
      const payload = {
        action: 'next',
        conversation_id: 'conv-only-1234',
      };
      const correlation = parseConversationCorrelation(payload);
      expect(correlation).not.toBeNull();
      expect(correlation?.conversationId).toBe('conv-only-1234');
      expect(correlation?.inputMessageId).toBeNull();
      expect(correlation?.isNewUserTurn).toBe(false);
    });

    it('identifies isNewUserTurn=false when messages last item is assistant (tool response/continuation) even with client_message_id', () => {
      const payload = {
        action: 'next',
        client_message_id: 'msg-client-root-1',
        messages: [
          {
            id: 'msg-user-history-1',
            author: { role: 'user' },
          },
          {
            id: 'msg-assistant-prev',
            author: { role: 'assistant' },
          },
        ],
        conversation_id: 'conv-real-99',
      };
      const correlation = parseConversationCorrelation(payload);
      expect(correlation).not.toBeNull();
      expect(correlation?.inputMessageId).toBeNull();
      expect(correlation?.isNewUserTurn).toBe(false);
    });

    it('returns null for payload without inputMessageId and conversationId', () => {
      const emptyPayload = {
        action: 'dummy',
      };
      expect(parseConversationCorrelation(emptyPayload)).toBeNull();
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

  describe('parseWebSocketFrame with Correlation Extraction', () => {
    it('extracts model evidence and message/conversation correlation IDs and requestId', () => {
      const wsFrame = JSON.stringify([
        {
          payload: {
            payload: {
              encoded_item: 'data: {"id": "msg-user-ws", "conversation_id": "conv-ws-1", "author": {"role": "user"}}\ndata: {"metadata": {"resolved_model_slug": "o3-mini", "request_id": "req-ws-888"}}\ndata: [DONE]\n',
            },
          },
        },
      ]);

      const results = parseWebSocketFrame(wsFrame);
      expect(results.length).toBe(1);
      const first = results[0];
      expect(first.evidence.resolvedModelSlug).toBe('o3-mini');
      expect(first.evidence.requestId).toBe('req-ws-888');
      expect(first.conversationIds).toContain('conv-ws-1');
      expect(first.messageIds).toContain('msg-user-ws');
      expect(first.terminal).toBe(true);
    });
  });
});
