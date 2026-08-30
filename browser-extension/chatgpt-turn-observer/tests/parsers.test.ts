import { describe, it, expect } from 'vitest';
import {
  classifyUserTurnStart,
  conversationIdFromUrl,
  extractActualModel,
  findNewestUserMessage,
  formatModelDisplayName,
  isConversationEndpoint,
  parseSseChunk,
  parseWebSocketFrame,
  SseStreamParser,
} from '../src/parsers';
import { EMPTY_ROUTE_EVIDENCE } from '../src/types';

describe('parsers & turn classification', () => {
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
    it('matches standard chatgpt conversation endpoints', () => {
      expect(isConversationEndpoint('/backend-api/conversation')).toBe(true);
      expect(isConversationEndpoint('/backend-api/conversations')).toBe(true);
      expect(isConversationEndpoint('/backend-api/lat/r')).toBe(true);
      expect(isConversationEndpoint('/backend-api/f/r')).toBe(true);
      expect(isConversationEndpoint('/backend-api/f/conversation')).toBe(true);
      expect(isConversationEndpoint('/backend-anon/conversation')).toBe(true);
      expect(isConversationEndpoint('https://chatgpt.com/backend-api/conversation')).toBe(true);
    });

    it('rejects files, telemetry, and analytics endpoints', () => {
      expect(isConversationEndpoint('/backend-api/files')).toBe(false);
      expect(isConversationEndpoint('/backend-api/files/upload')).toBe(false);
      expect(isConversationEndpoint('/backend-api/telemetry')).toBe(false);
      expect(isConversationEndpoint('/ces/v1/t')).toBe(false);
      expect(isConversationEndpoint('/backend-api/synthesize')).toBe(false);
      expect(isConversationEndpoint('/backend-api/settings')).toBe(false);
    });
  });

  describe('findNewestUserMessage', () => {
    it('extracts newest message with role === user', () => {
      const messages = [
        { id: 'msg-sys', author: { role: 'system' } },
        { id: 'msg-user-1', author: { role: 'user' } },
        { id: 'msg-assistant-1', author: { role: 'assistant' } },
        { id: 'msg-user-2', role: 'user' },
      ];
      const found = findNewestUserMessage(messages);
      expect(found).not.toBeNull();
      expect(found?.id).toBe('msg-user-2');
    });

    it('returns null if no user message exists', () => {
      const messages = [
        { id: 'msg-sys', author: { role: 'system' } },
        { id: 'msg-assistant', author: { role: 'assistant' } },
      ];
      expect(findNewestUserMessage(messages)).toBeNull();
    });
  });

  describe('classifyUserTurnStart (Strict Turn Start Semantics)', () => {
    it('classifies real user message submission as NEW_USER_TURN', () => {
      const body = {
        action: 'next',
        messages: [
          {
            id: 'msg-user-100',
            author: { role: 'user' },
            content: { content_type: 'text', parts: ['Hello Coding Tools'] },
          },
        ],
        conversation_id: 'conv-real-1',
        model: 'gpt-4o',
      };

      const decision = classifyUserTurnStart(
        '/backend-api/conversation',
        'POST',
        body,
        null
      );

      expect(decision.type).toBe('NEW_USER_TURN');
      if (decision.type === 'NEW_USER_TURN') {
        expect(decision.userMessageId).toBe('msg-user-100');
        expect(decision.requestedModel).toBe('gpt-4o');
        expect(decision.conversationId).toBe('conv-real-1');
      }
    });

    it('rejects image upload requests (UPLOAD_ONLY)', () => {
      const uploadBody = {
        file_name: 'test.png',
        file_size: 1024,
        use_case: 'multimodal',
      };

      const decision = classifyUserTurnStart(
        '/backend-api/files',
        'POST',
        uploadBody,
        null
      );

      expect(decision.type).toBe('NON_TURN_REQUEST');
      if (decision.type === 'NON_TURN_REQUEST') {
        expect(decision.reason).toBe('UPLOAD_ONLY');
      }
    });

    it('rejects copy / telemetry requests (COPY_TELEMETRY)', () => {
      const telemetryBody = {
        events: [{ type: 'copy_text', message_id: 'msg-assistant-1' }],
      };

      const decision = classifyUserTurnStart(
        '/ces/v1/t',
        'POST',
        telemetryBody,
        null
      );

      expect(decision.type).toBe('NON_TURN_REQUEST');
      if (decision.type === 'NON_TURN_REQUEST') {
        expect(decision.reason).toBe('COPY_TELEMETRY');
      }
    });

    it('rejects requests without user message (NO_USER_MESSAGE)', () => {
      const noUserBody = {
        action: 'get_metadata',
        conversation_id: 'conv-123',
      };

      const decision = classifyUserTurnStart(
        '/backend-api/conversation',
        'POST',
        noUserBody,
        null
      );

      expect(decision.type).toBe('NON_TURN_REQUEST');
      if (decision.type === 'NON_TURN_REQUEST') {
        expect(decision.reason).toBe('NO_USER_MESSAGE');
      }
    });

    it('identifies SAME_TURN_CONTINUATION if userMessageId matches active turn', () => {
      const body = {
        action: 'next',
        messages: [{ id: 'msg-user-active', role: 'user' }],
        conversation_id: 'conv-123',
      };

      const decision = classifyUserTurnStart(
        '/backend-api/conversation',
        'POST',
        body,
        'msg-user-active'
      );

      expect(decision.type).toBe('SAME_TURN_CONTINUATION');
    });

    it('handles image + text combined sending: upload is non-turn, final send is exactly 1 NEW_USER_TURN', () => {
      // 1. 上传图片阶段
      const uploadDecision = classifyUserTurnStart(
        '/backend-api/files',
        'POST',
        { file_id: 'file-123' },
        null
      );
      expect(uploadDecision.type).toBe('NON_TURN_REQUEST');

      // 2. 最终点击发送阶段
      const sendDecision = classifyUserTurnStart(
        '/backend-api/conversation',
        'POST',
        {
          action: 'next',
          messages: [
            {
              id: 'msg-combined-turn',
              author: { role: 'user' },
              content: { parts: ['explain this image'] },
              metadata: { attachments: [{ id: 'file-123' }] },
            },
          ],
          model: 'o3-mini',
        },
        null
      );
      expect(sendDecision.type).toBe('NEW_USER_TURN');
      if (sendDecision.type === 'NEW_USER_TURN') {
        expect(sendDecision.userMessageId).toBe('msg-combined-turn');
        expect(sendDecision.requestedModel).toBe('o3-mini');
      }
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
