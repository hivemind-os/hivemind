import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { createMcpServer } from '../server.js';
import { monitor } from '../monitor.js';

export async function startStdio(): Promise<void> {
  const server = createMcpServer();
  const transport = new StdioServerTransport();

  monitor.logConnection({
    clientId: 'stdio',
    transport: 'stdio',
    connected: true,
    timestamp: Date.now(),
  });

  transport.onclose = () => {
    monitor.logConnection({
      clientId: 'stdio',
      transport: 'stdio',
      connected: false,
      timestamp: Date.now(),
    });
  };

  await server.connect(transport);
  console.error('[mock-mcp-server] Stdio transport connected');
}
