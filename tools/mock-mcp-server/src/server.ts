import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from '@modelcontextprotocol/sdk/types.js';
import { ALL_TOOLS, getToolDef, resolveResponse } from './tools/registry.js';
import { monitor } from './monitor.js';
import { randomUUID } from 'crypto';

export function createMcpServer(): Server {
  const server = new Server(
    { name: 'mock-mcp-server', version: '0.1.0' },
    { capabilities: { tools: {}, logging: {} } }
  );

  server.setRequestHandler(ListToolsRequestSchema, async () => {
    return {
      tools: ALL_TOOLS.map(t => ({
        name: t.name,
        description: t.description,
        inputSchema: t.inputSchema as any,
      })),
    };
  });

  server.setRequestHandler(CallToolRequestSchema, async (request) => {
    const { name, arguments: args } = request.params;
    const toolArgs = (args ?? {}) as Record<string, unknown>;
    const requestId = randomUUID();
    const startTime = Date.now();

    monitor.logRequest({
      id: requestId,
      timestamp: startTime,
      method: 'tools/call',
      toolName: name,
      arguments: toolArgs,
    });

    const settings = monitor.getSettings();

    if (settings.paused) {
      await waitForUnpause();
    }

    if (settings.delay > 0) {
      await sleep(settings.delay);
    }

    if (settings.failRate > 0 && Math.random() < settings.failRate) {
      const response = {
        isError: true,
        content: [{ type: 'text' as const, text: 'Error: Random failure triggered by configured fail rate.' }],
      };
      monitor.logResponse({
        requestId,
        timestamp: Date.now(),
        duration: Date.now() - startTime,
        isError: true,
        content: response.content,
      });
      return response;
    }

    const toolDef = getToolDef(name);
    if (!toolDef) {
      const errResponse = {
        isError: true,
        content: [{ type: 'text' as const, text: `Error: Unknown tool "${name}"` }],
      };
      monitor.logResponse({
        requestId,
        timestamp: Date.now(),
        duration: Date.now() - startTime,
        isError: true,
        content: errResponse.content,
      });
      return errResponse;
    }

    const override = monitor.getToolOverride(name);
    const response = resolveResponse(toolDef, override, toolArgs);

    monitor.logResponse({
      requestId,
      timestamp: Date.now(),
      duration: Date.now() - startTime,
      isError: response.isError,
      content: response.content,
    });

    return response;
  });

  return server;
}

function sleep(ms: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms));
}

function waitForUnpause(): Promise<void> {
  return new Promise(resolve => {
    const check = () => {
      if (!monitor.getSettings().paused) {
        monitor.removeListener('event', onEvent);
        resolve();
      }
    };
    const onEvent = () => check();
    monitor.on('event', onEvent);
    check();
  });
}
