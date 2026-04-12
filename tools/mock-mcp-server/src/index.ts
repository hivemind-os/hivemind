#!/usr/bin/env node

import { parseArgs } from 'util';
import { monitor } from './monitor.js';

interface CliOptions {
  mode: 'stdio' | 'http';
  port: number;
  dashboardPort: number;
  delay: number;
  failRate: number;
}

function parseCli(): CliOptions {
  const { values } = parseArgs({
    options: {
      mode: { type: 'string', short: 'm', default: 'stdio' },
      port: { type: 'string', short: 'p', default: '6100' },
      'dashboard-port': { type: 'string', short: 'd', default: '6100' },
      delay: { type: 'string', default: '0' },
      'fail-rate': { type: 'string', default: '0' },
      help: { type: 'boolean', short: 'h', default: false },
    },
    strict: true,
  });

  if (values.help) {
    printHelp();
    process.exit(0);
  }

  const mode = values.mode as string;
  if (mode !== 'stdio' && mode !== 'http') {
    console.error(`Error: Invalid mode "${mode}". Must be "stdio" or "http".`);
    process.exit(1);
  }

  return {
    mode,
    port: parseInt(values.port as string, 10),
    dashboardPort: parseInt(values['dashboard-port'] as string, 10),
    delay: parseInt(values.delay as string, 10),
    failRate: parseFloat(values['fail-rate'] as string),
  };
}

function printHelp() {
  console.log(`
Mock MCP Server — A configurable mock MCP server for manual testing

Usage: mock-mcp-server [options]

Options:
  -m, --mode <stdio|http>       Transport mode (default: stdio)
  -p, --port <number>           HTTP port for MCP transport + dashboard (default: 6100)
  -d, --dashboard-port <number> Dashboard port in stdio mode (default: 6100)
      --delay <ms>              Default response delay in ms (default: 0)
      --fail-rate <0-1>         Random failure rate, 0.0 to 1.0 (default: 0)
  -h, --help                    Show this help message

Modes:
  stdio  Read/write MCP JSON-RPC via stdin/stdout (default).
         Dashboard served on --dashboard-port.

  http   MCP protocol over HTTP with both SSE and Streamable HTTP.
         Dashboard served on the same --port at /dashboard.
         SSE endpoint: /sse (GET) + /messages (POST)
         Streamable HTTP endpoint: /mcp (GET/POST/DELETE)

Dashboard: http://localhost:<port>/dashboard
  `.trim());
}

async function main() {
  const opts = parseCli();

  // Apply initial settings
  monitor.updateSettings({ delay: opts.delay, failRate: opts.failRate });

  if (opts.mode === 'stdio') {
    // Stdio mode: MCP on stdin/stdout, dashboard on separate HTTP port
    const { createDashboard } = await import('./dashboard/server.js');
    const { server, wss } = await createDashboard(opts.dashboardPort);

    const { startStdio } = await import('./transports/stdio.js');
    await startStdio();

    console.error(`[mock-mcp-server] Running in stdio mode`);

    process.on('SIGINT', () => {
      console.error('\n[mock-mcp-server] Shutting down...');
      wss.close();
      server.close();
      process.exit(0);
    });
  } else {
    // HTTP mode: MCP + dashboard on same port
    const express = (await import('express')).default;
    const { createHttpRouter, closeAllTransports } = await import('./transports/http.js');
    const { createDashboard } = await import('./dashboard/server.js');

    const app = express();
    app.use(express.json());

    // Mount MCP HTTP transports
    app.use(createHttpRouter());

    // Mount dashboard on the same app
    const { server, wss } = await createDashboard(opts.port, app);

    server.listen(opts.port, () => {
      console.error(`[mock-mcp-server] Running in HTTP mode on port ${opts.port}`);
      console.error(`[mock-mcp-server] Dashboard: http://localhost:${opts.port}/dashboard`);
      console.error(`[mock-mcp-server] Streamable HTTP: http://localhost:${opts.port}/mcp`);
      console.error(`[mock-mcp-server] SSE: http://localhost:${opts.port}/sse`);
    });

    process.on('SIGINT', async () => {
      console.error('\n[mock-mcp-server] Shutting down...');
      await closeAllTransports();
      wss.close();
      server.close();
      process.exit(0);
    });
  }
}

main().catch(err => {
  console.error('[mock-mcp-server] Fatal error:', err);
  process.exit(1);
});
