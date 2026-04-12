import { randomUUID } from 'crypto';
import { Router, Request, Response } from 'express';
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js';
import { SSEServerTransport } from '@modelcontextprotocol/sdk/server/sse.js';
import { isInitializeRequest } from '@modelcontextprotocol/sdk/types.js';
import { Transport } from '@modelcontextprotocol/sdk/shared/transport.js';
import { createMcpServer } from '../server.js';
import { monitor } from '../monitor.js';

const transports: Record<string, Transport> = {};

export function createHttpRouter(): Router {
  const router = Router();

  // --- Streamable HTTP transport: /mcp ---
  router.all('/mcp', async (req: Request, res: Response) => {
    try {
      const sessionId = req.headers['mcp-session-id'] as string | undefined;
      let transport: StreamableHTTPServerTransport;

      if (sessionId && transports[sessionId]) {
        const existing = transports[sessionId];
        if (existing instanceof StreamableHTTPServerTransport) {
          transport = existing;
        } else {
          res.status(400).json({
            jsonrpc: '2.0',
            error: { code: -32000, message: 'Session uses a different transport protocol' },
            id: null,
          });
          return;
        }
      } else if (!sessionId && req.method === 'POST' && isInitializeRequest(req.body)) {
        transport = new StreamableHTTPServerTransport({
          sessionIdGenerator: () => randomUUID(),
          onsessioninitialized: (sid: string) => {
            transports[sid] = transport;
            monitor.logConnection({
              clientId: sid,
              transport: 'streamable-http',
              connected: true,
              timestamp: Date.now(),
            });
          },
        });

        transport.onclose = () => {
          const sid = (transport as any).sessionId;
          if (sid && transports[sid]) {
            delete transports[sid];
            monitor.logConnection({
              clientId: sid,
              transport: 'streamable-http',
              connected: false,
              timestamp: Date.now(),
            });
          }
        };

        const server = createMcpServer();
        await server.connect(transport);
      } else {
        res.status(400).json({
          jsonrpc: '2.0',
          error: { code: -32000, message: 'Bad Request: No valid session ID provided' },
          id: null,
        });
        return;
      }

      await transport.handleRequest(req, res, req.body);
    } catch (error) {
      console.error('[mock-mcp-server] Error handling /mcp:', error);
      if (!res.headersSent) {
        res.status(500).json({
          jsonrpc: '2.0',
          error: { code: -32603, message: 'Internal server error' },
          id: null,
        });
      }
    }
  });

  // --- SSE transport: /sse + /messages ---
  router.get('/sse', async (req: Request, res: Response) => {
    try {
      const transport = new SSEServerTransport('/messages', res);
      transports[transport.sessionId] = transport;

      monitor.logConnection({
        clientId: transport.sessionId,
        transport: 'sse',
        connected: true,
        timestamp: Date.now(),
      });

      res.on('close', () => {
        delete transports[transport.sessionId];
        transport.close().catch(() => {});
        monitor.logConnection({
          clientId: transport.sessionId,
          transport: 'sse',
          connected: false,
          timestamp: Date.now(),
        });
      });

      const server = createMcpServer();
      await server.connect(transport);
    } catch (error) {
      console.error('[mock-mcp-server] Error handling /sse:', error);
      if (!res.headersSent) {
        res.status(500).send('Internal server error');
      }
    }
  });

  router.post('/messages', async (req: Request, res: Response) => {
    const sessionId = req.query.sessionId as string;
    const existing = transports[sessionId];

    if (existing instanceof SSEServerTransport) {
      await existing.handlePostMessage(req, res, req.body);
    } else {
      res.status(400).send('No SSE transport found for sessionId');
    }
  });

  return router;
}

export async function closeAllTransports(): Promise<void> {
  for (const sid of Object.keys(transports)) {
    try {
      await transports[sid].close();
      delete transports[sid];
    } catch {
      // ignore cleanup errors
    }
  }
}
