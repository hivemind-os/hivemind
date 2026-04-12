import { EventEmitter } from 'events';

export interface ToolRequest {
  id: string;
  timestamp: number;
  method: string;
  toolName: string;
  arguments: Record<string, unknown>;
}

export interface ToolResponse {
  requestId: string;
  timestamp: number;
  duration: number;
  isError: boolean;
  content: Array<{ type: string; text?: string }>;
}

export interface MonitorEvent {
  type: 'request' | 'response' | 'connection' | 'settings_changed';
  data: ToolRequest | ToolResponse | ConnectionEvent | SettingsSnapshot;
}

export interface ConnectionEvent {
  clientId: string;
  transport: string;
  connected: boolean;
  timestamp: number;
}

export interface SettingsSnapshot {
  delay: number;
  failRate: number;
  paused: boolean;
  overrides: Record<string, string | null>;
}

export interface GlobalSettings {
  delay: number;
  failRate: number;
  paused: boolean;
}

class Monitor extends EventEmitter {
  private requestLog: Array<{ request: ToolRequest; response?: ToolResponse }> = [];
  private settings: GlobalSettings = { delay: 0, failRate: 0, paused: false };
  private toolOverrides: Map<string, string | null> = new Map();
  private connectedClients: Map<string, ConnectionEvent> = new Map();

  private static MAX_LOG_SIZE = 2000;

  getSettings(): GlobalSettings {
    return { ...this.settings };
  }

  updateSettings(partial: Partial<GlobalSettings>): void {
    Object.assign(this.settings, partial);
    this.emit('event', {
      type: 'settings_changed',
      data: this.getSnapshot(),
    } satisfies MonitorEvent);
  }

  getToolOverride(toolName: string): string | null {
    return this.toolOverrides.get(toolName) ?? null;
  }

  setToolOverride(toolName: string, responseKey: string | null): void {
    if (responseKey === null) {
      this.toolOverrides.delete(toolName);
    } else {
      this.toolOverrides.set(toolName, responseKey);
    }
    this.emit('event', {
      type: 'settings_changed',
      data: this.getSnapshot(),
    } satisfies MonitorEvent);
  }

  logRequest(request: ToolRequest): void {
    this.requestLog.push({ request });
    if (this.requestLog.length > Monitor.MAX_LOG_SIZE) {
      this.requestLog = this.requestLog.slice(-Monitor.MAX_LOG_SIZE);
    }
    this.emit('event', { type: 'request', data: request } satisfies MonitorEvent);
  }

  logResponse(response: ToolResponse): void {
    const entry = this.requestLog.find(e => e.request.id === response.requestId);
    if (entry) {
      entry.response = response;
    }
    this.emit('event', { type: 'response', data: response } satisfies MonitorEvent);
  }

  logConnection(event: ConnectionEvent): void {
    if (event.connected) {
      this.connectedClients.set(event.clientId, event);
    } else {
      this.connectedClients.delete(event.clientId);
    }
    this.emit('event', { type: 'connection', data: event } satisfies MonitorEvent);
  }

  getConnectedClients(): ConnectionEvent[] {
    return [...this.connectedClients.values()];
  }

  getRequestLog() {
    return [...this.requestLog];
  }

  getSnapshot(): SettingsSnapshot {
    const overrides: Record<string, string | null> = {};
    for (const [k, v] of this.toolOverrides) {
      overrides[k] = v;
    }
    return { ...this.settings, overrides };
  }

  clearLog(): void {
    this.requestLog = [];
  }
}

export const monitor = new Monitor();
