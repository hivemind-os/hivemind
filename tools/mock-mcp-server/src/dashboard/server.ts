import express from 'express';
import { createServer, Server } from 'http';
import { WebSocketServer, WebSocket } from 'ws';
import path from 'path';
import { fileURLToPath } from 'url';
import { monitor, MonitorEvent } from '../monitor.js';
import { ALL_TOOLS } from '../tools/registry.js';
import { probeFileRead, probeFileWrite, probeDirList, probeNetwork } from '../probes.js';
import { AddressInfo } from 'net';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

export interface DashboardHandle {
  app: express.Express;
  server: Server;
  wss: WebSocketServer;
}

/**
 * Start the dashboard and wait for it to be ready.
 * Returns only after the HTTP server is actually listening.
 * If the requested port is in use, automatically retries on the next port.
 */
export async function createDashboard(
  port: number,
  existingApp?: express.Express,
): Promise<DashboardHandle> {
  const app = existingApp ?? express();
  const server = createServer(app);

  // Serve static dashboard files
  const publicDir = path.join(__dirname, 'public');
  app.use('/dashboard', express.static(publicDir, { index: 'index.html' }));

  // REST API for initial state
  app.get('/api/tools', (_req, res) => {
    res.json(ALL_TOOLS);
  });

  app.get('/api/log', (_req, res) => {
    res.json(monitor.getRequestLog());
  });

  app.get('/api/settings', (_req, res) => {
    res.json(monitor.getSnapshot());
  });

  // Probe endpoints — real I/O for sandbox testing
  app.use(express.json());

  app.post('/api/probe/file-read', async (req, res) => {
    const { path: filePath } = req.body ?? {};
    if (!filePath || typeof filePath !== 'string') {
      return res.status(400).json({ error: 'Missing "path" string in body' });
    }
    res.json(await probeFileRead(filePath));
  });

  app.post('/api/probe/file-write', async (req, res) => {
    const { path: filePath, content } = req.body ?? {};
    if (!filePath || typeof filePath !== 'string') {
      return res.status(400).json({ error: 'Missing "path" string in body' });
    }
    res.json(await probeFileWrite(filePath, content ?? ''));
  });

  app.post('/api/probe/dir-list', async (req, res) => {
    const { path: dirPath } = req.body ?? {};
    if (!dirPath || typeof dirPath !== 'string') {
      return res.status(400).json({ error: 'Missing "path" string in body' });
    }
    res.json(await probeDirList(dirPath));
  });

  app.post('/api/probe/network', async (req, res) => {
    const { url, method, body, headers } = req.body ?? {};
    if (!url || typeof url !== 'string') {
      return res.status(400).json({ error: 'Missing "url" string in body' });
    }
    res.json(await probeNetwork(url, method, body, headers));
  });

  // In stdio mode, we manage listen ourselves (with port retry).
  // In HTTP mode (existingApp provided), the caller manages listen.
  if (!existingApp) {
    await listenWithRetry(server, port);
    const addr = server.address() as AddressInfo;
    console.error(`[mock-mcp-server] Dashboard: http://localhost:${addr.port}/dashboard`);
  }

  // Attach WebSocket AFTER the server is listening (or after setup for HTTP mode)
  // so that EADDRINUSE errors during listen don't crash through WSS.
  const wss = new WebSocketServer({ server, path: '/ws' });

  wss.on('connection', (ws: WebSocket) => {
    ws.send(JSON.stringify({
      type: 'init',
      data: {
        tools: ALL_TOOLS,
        log: monitor.getRequestLog(),
        settings: monitor.getSnapshot(),
        clients: monitor.getConnectedClients(),
      },
    }));

    const onEvent = (event: MonitorEvent) => {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify(event));
      }
    };

    monitor.on('event', onEvent);

    ws.on('message', (raw: Buffer) => {
      try {
        const msg = JSON.parse(raw.toString());
        handleDashboardMessage(msg);
      } catch {
        // ignore malformed messages
      }
    });

    ws.on('close', () => {
      monitor.removeListener('event', onEvent);
    });
  });

  return { app, server, wss };
}

/**
 * Try to listen on `port`. If EADDRINUSE, retry on port+1, port+2, etc.
 * up to 10 attempts. Resolves when listening; rejects on persistent failure.
 */
function listenWithRetry(server: Server, port: number, maxRetries = 10): Promise<void> {
  return new Promise((resolve, reject) => {
    let attempt = 0;

    function tryListen(p: number) {
      server.once('error', (err: NodeJS.ErrnoException) => {
        if (err.code === 'EADDRINUSE' && attempt < maxRetries) {
          attempt++;
          const nextPort = p + 1;
          console.error(`[mock-mcp-server] Port ${p} in use, trying ${nextPort}...`);
          tryListen(nextPort);
        } else {
          reject(err);
        }
      });
      server.listen(p, () => resolve());
    }

    tryListen(port);
  });
}

function handleDashboardMessage(msg: any) {
  switch (msg.type) {
    case 'set_settings':
      monitor.updateSettings(msg.data);
      break;
    case 'set_override':
      monitor.setToolOverride(msg.toolName, msg.responseKey);
      break;
    case 'clear_log':
      monitor.clearLog();
      break;
  }
}
