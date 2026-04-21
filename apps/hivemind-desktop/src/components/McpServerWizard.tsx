import { For, Show, createSignal, createMemo } from 'solid-js';
import { LoaderCircle, Wrench, FileText, MessageSquare, Shield } from 'lucide-solid';
import { Dialog, DialogContent } from '~/ui/dialog';
import { Button } from '~/ui/button';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import { authFetch } from '~/lib/authFetch';
import type { McpServerConfig, McpHeaderValue, TransportKind } from '../types';
import { McpRegistryBrowser } from './McpRegistryBrowser';

// ── Helpers ──────────────────────────────────────────────────────

function parseShellArgs(input: string): string[] {
  const args: string[] = [];
  let current = '';
  let inSingle = false;
  let inDouble = false;
  let wasQuoted = false;
  for (let i = 0; i < input.length; i++) {
    const ch = input[i];
    // Handle backslash escapes inside double quotes
    if (ch === '\\' && inDouble && i + 1 < input.length) {
      const next = input[i + 1];
      if (next === '"' || next === '\\') { current += next; i++; continue; }
    }
    if (ch === "'" && !inDouble) { inSingle = !inSingle; wasQuoted = true; continue; }
    if (ch === '"' && !inSingle) { inDouble = !inDouble; wasQuoted = true; continue; }
    if (ch === ' ' && !inSingle && !inDouble) {
      if (current || wasQuoted) { args.push(current); current = ''; wasQuoted = false; }
      continue;
    }
    current += ch;
  }
  if (current || wasQuoted) args.push(current);
  return args;
}

// ── Types ────────────────────────────────────────────────────────

type WizardStep = 'source' | 'transport' | 'connection' | 'options' | 'sandbox' | 'test' | 'review';

const STEP_LABELS: Record<WizardStep, string> = {
  source: 'Source',
  transport: 'Transport',
  connection: 'Connection',
  options: 'Options',
  sandbox: 'Sandbox',
  test: 'Test',
  review: 'Review',
};

const WIZARD_STEPS: WizardStep[] = ['source', 'transport', 'connection', 'options', 'sandbox', 'test', 'review'];

interface DiscoveredTool {
  name: string;
  description: string;
}
interface DiscoveredResource {
  uri: string;
  name: string;
  description?: string | null;
}
interface DiscoveredPrompt {
  name: string;
  description?: string | null;
}

interface McpServerWizardProps {
  daemon_url: string;
  existingIds: string[];
  editingConfig?: McpServerConfig;
  onFinish: (config: McpServerConfig) => void;
  onClose: () => void;
}

// ── Component ────────────────────────────────────────────────────

export function McpServerWizard(props: McpServerWizardProps) {
  const isEditing = () => !!props.editingConfig;

  // Step state — in edit mode skip source step, start at transport
  const [step, setStep] = createSignal<WizardStep>(props.editingConfig ? 'transport' : 'source');
  const currentIdx = () => WIZARD_STEPS.indexOf(step());

  // Registry state
  const [fromRegistry, setFromRegistry] = createSignal(false);
  const [registryServerName, setRegistryServerName] = createSignal<string | null>(null);
  const [showRegistryBrowser, setShowRegistryBrowser] = createSignal(false);

  // Draft config — pre-fill from editingConfig when editing
  const [server_id, setServerId] = createSignal(props.editingConfig?.id ?? '');
  const [transport, setTransport] = createSignal<TransportKind>(props.editingConfig?.transport ?? 'stdio');
  const [command, setCommand] = createSignal(props.editingConfig?.command ?? '');
  const [args, setArgs] = createSignal(props.editingConfig?.args?.join(' ') ?? '');
  const [url, setUrl] = createSignal(props.editingConfig?.url ?? '');
  const [envPairs, setEnvPairs] = createSignal<Array<{ key: string; value: string }>>(
    props.editingConfig?.env ? Object.entries(props.editingConfig.env).map(([key, value]) => ({ key, value })) : []
  );
  const [headerPairs, setHeaderPairs] = createSignal<Array<{ key: string; value: string; secret: boolean }>>(
    props.editingConfig?.headers ? Object.entries(props.editingConfig.headers).map(([key, hv]) => ({ key, value: hv.value, secret: hv.type === 'secret-ref' })) : []
  );
  const [autoReconnect, setAutoReconnect] = createSignal(props.editingConfig?.auto_reconnect ?? true);
  const [reactive, setReactive] = createSignal(props.editingConfig?.reactive ?? false);

  // Sandbox signals — only applicable to stdio transport
  const [sandboxEnabled, setSandboxEnabled] = createSignal(props.editingConfig?.sandbox?.enabled ?? false);
  const [sandboxReadWorkspace, setSandboxReadWorkspace] = createSignal(props.editingConfig?.sandbox?.read_workspace ?? true);
  const [sandboxWriteWorkspace, setSandboxWriteWorkspace] = createSignal(props.editingConfig?.sandbox?.write_workspace ?? false);
  const [sandboxAllowNetwork, setSandboxAllowNetwork] = createSignal(props.editingConfig?.sandbox?.allow_network ?? true);
  const [sandboxExtraReadPaths, setSandboxExtraReadPaths] = createSignal<string[]>(props.editingConfig?.sandbox?.extra_read_paths ?? []);
  const [sandboxExtraWritePaths, setSandboxExtraWritePaths] = createSignal<string[]>(props.editingConfig?.sandbox?.extra_write_paths ?? []);

  const isStdio = () => transport() === 'stdio';

  // Test state
  const [testing, setTesting] = createSignal(false);
  const [testError, setTestError] = createSignal<string | null>(null);
  const [discoveredTools, setDiscoveredTools] = createSignal<DiscoveredTool[]>([]);
  const [discoveredResources, setDiscoveredResources] = createSignal<DiscoveredResource[]>([]);
  const [discoveredPrompts, setDiscoveredPrompts] = createSignal<DiscoveredPrompt[]>([]);
  const [testPassed, setTestPassed] = createSignal(false);

  // ── Build config from draft ──
  const buildConfig = (): McpServerConfig => {
    const env: Record<string, string> = {};
    for (const p of envPairs()) {
      if (p.key.trim()) env[p.key.trim()] = p.value;
    }
    const headers: Record<string, McpHeaderValue> = {};
    for (const h of headerPairs()) {
      if (h.key.trim()) {
        headers[h.key.trim()] = h.secret
          ? { type: 'secret-ref', value: h.value }
          : { type: 'plain', value: h.value };
      }
    }
    const parsedArgs = args().trim() ? parseShellArgs(args().trim()) : [];

    return {
      id: server_id().trim(),
      transport: transport(),
      command: transport() === 'stdio' ? (command().trim() || null) : null,
      args: parsedArgs,
      url: transport() !== 'stdio' ? (url().trim() || null) : null,
      env,
      headers,
      channel_class: props.editingConfig?.channel_class ?? 'internal',
      enabled: props.editingConfig?.enabled ?? true,
      auto_connect: props.editingConfig?.auto_connect ?? true,
      reactive: reactive(),
      auto_reconnect: autoReconnect(),
      sandbox: isStdio() && sandboxEnabled() ? {
        enabled: true,
        read_workspace: sandboxReadWorkspace(),
        write_workspace: sandboxWriteWorkspace(),
        allow_network: sandboxAllowNetwork(),
        extra_read_paths: sandboxExtraReadPaths().filter(p => p.trim() !== ''),
        extra_write_paths: sandboxExtraWritePaths().filter(p => p.trim() !== ''),
      } : null,
    };
  };

  // ── Validation ──
  const idError = createMemo(() => {
    const id = server_id().trim();
    if (!id) return null;
    if (props.existingIds.includes(id)) return 'ID already exists';
    if (!/^[a-zA-Z0-9_-]+$/.test(id)) return 'Only alphanumeric, dash, underscore';
    return null;
  });

  const canAdvance = createMemo(() => {
    const s = step();
    if (s === 'source') return false; // navigation via card clicks
    if (s === 'transport') return !!server_id().trim() && !idError();
    if (s === 'connection') {
      if (transport() === 'stdio') return !!command().trim();
      return !!url().trim();
    }
    if (s === 'options') return true;
    if (s === 'sandbox') return true;
    if (s === 'test') return testPassed();
    return true;
  });

  const next = () => {
    const idx = currentIdx();
    if (idx < WIZARD_STEPS.length - 1) {
      const nextStep = WIZARD_STEPS[idx + 1];
      // Skip sandbox step for non-stdio transport
      if (nextStep === 'sandbox' && !isStdio()) {
        setStep(WIZARD_STEPS[idx + 2]);
      } else {
        setStep(nextStep);
      }
    }
  };
  const back = () => {
    const idx = currentIdx();
    if (idx > 0) {
      if (step() === 'test') {
        setTestPassed(false);
        setTestError(null);
        setDiscoveredTools([]);
        setDiscoveredResources([]);
        setDiscoveredPrompts([]);
      }
      // In edit mode, transport is the first step — don't go back to source
      if (isEditing() && step() === 'transport') {
        return;
      }
      // When from registry, going back from 'options' should skip to 'source'
      if (fromRegistry() && step() === 'options') {
        setFromRegistry(false);
        setStep('source');
        return;
      }
      const prevStep = WIZARD_STEPS[idx - 1];
      // Skip sandbox step for non-stdio transport
      if (prevStep === 'sandbox' && !isStdio()) {
        setStep(WIZARD_STEPS[idx - 2]);
      } else {
        setStep(prevStep);
      }
    }
  };

  // Steps shown in the indicator (hide transport/connection when from registry, hide source when editing, hide sandbox for non-stdio)
  const displaySteps = createMemo(() => {
    let steps = WIZARD_STEPS as readonly WizardStep[];
    if (isEditing()) {
      steps = steps.filter(s => s !== 'source');
    }
    if (fromRegistry()) {
      steps = steps.filter(s => s !== 'transport' && s !== 'connection');
    }
    if (!isStdio()) {
      steps = steps.filter(s => s !== 'sandbox');
    }
    return steps;
  });

  // ── Test connection ──
  const runTest = async () => {
    setTesting(true);
    setTestError(null);
    setTestPassed(false);
    setDiscoveredTools([]);
    setDiscoveredResources([]);
    setDiscoveredPrompts([]);

    try {
      const config = buildConfig();
      const resp = await authFetch(`${props.daemon_url}/api/v1/mcp/test-connect`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ config }),
      });
      if (!resp.ok) {
        const text = await resp.text();
        setTestError(text || `HTTP ${resp.status}`);
        return;
      }
      const data = await resp.json();
      setDiscoveredTools(data.tools ?? []);
      setDiscoveredResources(data.resources ?? []);
      setDiscoveredPrompts(data.prompts ?? []);
      setTestPassed(true);
    } catch (e: any) {
      setTestError(e?.message || 'Connection failed');
    } finally {
      setTesting(false);
    }
  };

  // ── Step renderers ──

  const renderSourceStep = () => (
    <div>
      <p style="margin-bottom: 1rem; color: hsl(var(--muted-foreground)); font-size: 0.9rem;">
        Choose how to add your MCP server.
      </p>
      <div class="channel-type-grid" style="grid-template-columns: 1fr 1fr; gap: 1rem;">
        <div
          class="channel-type-card"
          onClick={() => {
            setFromRegistry(true);
            setShowRegistryBrowser(true);
          }}
          style="cursor: pointer; padding: 1.5rem; text-align: center;"
        >
          <div style="font-size: 1.5rem; margin-bottom: 0.5rem;">🔍</div>
          <div class="channel-type-card-title">Browse Registry</div>
          <div class="channel-type-card-desc">Search the official MCP server registry and auto-configure</div>
        </div>
        <div
          class="channel-type-card"
          onClick={() => {
            setFromRegistry(false);
            setStep('transport');
          }}
          style="cursor: pointer; padding: 1.5rem; text-align: center;"
        >
          <div style="font-size: 1.5rem; margin-bottom: 0.5rem;">⚙️</div>
          <div class="channel-type-card-title">Configure Manually</div>
          <div class="channel-type-card-desc">Set up transport, command, and arguments yourself</div>
        </div>
      </div>
    </div>
  );

  const handleRegistrySelect = (config: Partial<McpServerConfig>, serverInfo: { name: string; description: string }) => {
    setShowRegistryBrowser(false);
    setFromRegistry(true);
    setRegistryServerName(serverInfo.name);

    // Clear all configurable fields to prevent state pollution from previous selections
    setServerId('');
    setCommand('');
    setArgs('');
    setUrl('');
    setEnvPairs([]);
    setHeaderPairs([]);
    setTransport('stdio');

    // Prefill wizard state from the mapped config
    if (config.id) setServerId(config.id);
    if (config.transport) setTransport(config.transport);
    if (config.command) setCommand(config.command);
    if (config.args) setArgs(config.args.join(' '));
    if (config.url) setUrl(config.url);

    // Prefill env pairs
    if (config.env && Object.keys(config.env).length > 0) {
      setEnvPairs(Object.entries(config.env).map(([key, value]) => ({ key, value })));
    }

    // Prefill header pairs
    if (config.headers && Object.keys(config.headers).length > 0) {
      setHeaderPairs(Object.entries(config.headers).map(([key, hv]) => ({
        key,
        value: hv.value,
        secret: hv.type === 'secret-ref',
      })));
    }

    // Jump to options step (skip transport and connection since they're pre-filled)
    setStep('options');
  };

  const renderTransportStep = () => (
    <div >
      <div class="channel-form-group">
        <label class="channel-form-label">Server ID</label>
        <input
          type="text"
          placeholder="e.g. my-mcp-server"
          value={server_id()}
          onInput={(e) => setServerId(e.currentTarget.value)}
          autocomplete="off"
          autocorrect="off"
          autocapitalize="off"
          spellcheck={false}
        />
        <Show when={idError()}>
          <p class="channel-form-hint" style="color: var(--destructive);">{idError()}</p>
        </Show>
        <p class="channel-form-hint">A unique identifier for this MCP server.</p>
      </div>

      <div class="channel-form-group">
        <label class="channel-form-label">Transport</label>
        <div class="channel-type-grid" style="grid-template-columns: repeat(3, 1fr);">
          <div
            class="channel-type-card"
            classList={{ selected: transport() === 'stdio' }}
            onClick={() => setTransport('stdio')}
          >
            <div class="channel-type-card-title">Stdio</div>
            <div class="channel-type-card-desc">Local process via stdin/stdout</div>
          </div>
          <div
            class="channel-type-card"
            classList={{ selected: transport() === 'sse' }}
            onClick={() => setTransport('sse')}
          >
            <div class="channel-type-card-title">SSE</div>
            <div class="channel-type-card-desc">Server-Sent Events over HTTP</div>
          </div>
          <div
            class="channel-type-card"
            classList={{ selected: transport() === 'streamable-http' }}
            onClick={() => setTransport('streamable-http')}
          >
            <div class="channel-type-card-title">Streamable HTTP</div>
            <div class="channel-type-card-desc">HTTP streaming transport</div>
          </div>
        </div>
      </div>
    </div>
  );

  const renderConnectionStep = () => (
    <div >
      <Show when={transport() === 'stdio'}>
        <div class="channel-form-group">
          <label class="channel-form-label">Command</label>
          <input
            type="text"
            placeholder="e.g. npx, uvx, node"
            value={command()}
            onInput={(e) => setCommand(e.currentTarget.value)}
            autocomplete="off"
            autocorrect="off"
            autocapitalize="off"
            spellcheck={false}
          />
          <p class="channel-form-hint">The executable to launch the MCP server.</p>
        </div>
        <div class="channel-form-group">
          <label class="channel-form-label">Arguments</label>
          <input
            type="text"
            placeholder="e.g. -y @modelcontextprotocol/server-filesystem /path"
            value={args()}
            onInput={(e) => setArgs(e.currentTarget.value)}
            autocomplete="off"
            autocorrect="off"
            autocapitalize="off"
            spellcheck={false}
          />
          <p class="channel-form-hint">Space-separated arguments passed to the command.</p>
        </div>
      </Show>

      <Show when={transport() !== 'stdio'}>
        <div class="channel-form-group">
          <label class="channel-form-label">URL</label>
          <input
            type="text"
            placeholder="https://my-server.example.com/mcp"
            value={url()}
            onInput={(e) => setUrl(e.currentTarget.value)}
            autocomplete="off"
            autocorrect="off"
            autocapitalize="off"
            spellcheck={false}
          />
          <p class="channel-form-hint">The endpoint URL for the MCP server.</p>
        </div>
      </Show>

      {/* Environment variables */}
      <div class="channel-form-group">
        <div style="display: flex; align-items: center; justify-content: space-between;">
          <label class="channel-form-label" style="margin-bottom: 0;">Environment Variables</label>
          <Button
            variant="outline"
            size="sm"
            onClick={() => setEnvPairs([...envPairs(), { key: '', value: '' }])}
          >+ Add</Button>
        </div>
        <For each={envPairs()}>
          {(pair, idx) => (
            <div style="display: flex; gap: 0.5rem; align-items: center; margin-top: 0.5rem;">
              <input
                type="text"
                placeholder="KEY"
                value={pair.key}
                onInput={(e) => {
                  const updated = [...envPairs()];
                  updated[idx()] = { ...updated[idx()], key: e.currentTarget.value };
                  setEnvPairs(updated);
                }}
                autocomplete="off"
                autocorrect="off"
                autocapitalize="off"
                spellcheck={false}
                style="flex: 1;"
              />
              <input
                type="text"
                placeholder="value"
                value={pair.value}
                onInput={(e) => {
                  const updated = [...envPairs()];
                  updated[idx()] = { ...updated[idx()], value: e.currentTarget.value };
                  setEnvPairs(updated);
                }}
                autocomplete="off"
                autocorrect="off"
                autocapitalize="off"
                spellcheck={false}
                style="flex: 2;"
              />
              <Button variant="outline" size="sm" onClick={() => setEnvPairs(envPairs().filter((_, i) => i !== idx()))}>✕</Button>
            </div>
          )}
        </For>
        <Show when={envPairs().length === 0}>
          <p class="channel-form-hint">No environment variables configured.</p>
        </Show>
      </div>

      {/* HTTP Headers (SSE/StreamableHTTP only) */}
      <Show when={transport() !== 'stdio'}>
        <div class="channel-form-group">
          <div style="display: flex; align-items: center; justify-content: space-between;">
            <label class="channel-form-label" style="margin-bottom: 0;">HTTP Headers</label>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setHeaderPairs([...headerPairs(), { key: '', value: '', secret: false }])}
            >+ Add</Button>
          </div>
          <For each={headerPairs()}>
            {(pair, idx) => (
              <div style="display: flex; gap: 0.5rem; align-items: center; margin-top: 0.5rem; flex-wrap: wrap;">
                <input
                  type="text"
                  placeholder="Header-Name"
                  value={pair.key}
                  onInput={(e) => {
                    const updated = [...headerPairs()];
                    updated[idx()] = { ...updated[idx()], key: e.currentTarget.value };
                    setHeaderPairs(updated);
                  }}
                  
                  style="flex: 1; min-width: 120px;"
                />
                <input
                  type={pair.secret ? 'password' : 'text'}
                  placeholder={pair.secret ? 'Keystore reference name' : 'Header value'}
                  value={pair.value}
                  onInput={(e) => {
                    const updated = [...headerPairs()];
                    updated[idx()] = { ...updated[idx()], value: e.currentTarget.value };
                    setHeaderPairs(updated);
                  }}
                  
                  style="flex: 2; min-width: 120px;"
                />
                <label style="display: flex; align-items: center; gap: 0.25rem; font-size: 0.8rem; white-space: nowrap; cursor: pointer;">
                  <input
                    type="checkbox"
                    checked={pair.secret}
                    onChange={(e) => {
                      const updated = [...headerPairs()];
                      updated[idx()] = { ...updated[idx()], secret: e.currentTarget.checked };
                      setHeaderPairs(updated);
                    }}
                  />
                  Keystore
                </label>
                <Button variant="outline" size="sm" onClick={() => setHeaderPairs(headerPairs().filter((_, i) => i !== idx()))}>✕</Button>
              </div>
            )}
          </For>
          <Show when={headerPairs().length === 0}>
            <p class="channel-form-hint">No HTTP headers configured.</p>
          </Show>
          <p class="channel-form-hint">Enable "Keystore" to store the value securely in the OS credential store.</p>
        </div>
      </Show>
    </div>
  );

  const renderOptionsStep = () => (
    <div >
      <div class="channel-form-group">
        <label style="display: flex; align-items: center; gap: 0.5rem; cursor: pointer;">
          <input type="checkbox" checked={autoReconnect()} onChange={(e) => setAutoReconnect(e.currentTarget.checked)} />
          <span>Auto-reconnect on failure</span>
        </label>
        <p class="channel-form-hint">Automatically reconnect if the server disconnects unexpectedly.</p>
      </div>
      <div class="channel-form-group">
        <label style="display: flex; align-items: center; gap: 0.5rem; cursor: pointer;">
          <input type="checkbox" checked={reactive()} onChange={(e) => setReactive(e.currentTarget.checked)} />
          <span>Reactive mode</span>
        </label>
        <p class="channel-form-hint">Subscribe to server notifications and resource changes.</p>
      </div>
    </div>
  );

  const renderSandboxStep = () => (
    <div>
      <p style="margin-bottom: 1rem; color: hsl(var(--muted-foreground)); font-size: 0.9rem;">
        <Shield size={14} style="display: inline; vertical-align: middle; margin-right: 4px;" />
        Restrict this MCP server's file system and network access using OS-level sandboxing.
      </p>

      <div class="settings-form" style="display: flex; flex-direction: column; gap: 0.75rem;">
        <Switch checked={sandboxEnabled()} onChange={(checked) => setSandboxEnabled(checked)} class="flex items-center gap-2">
          <SwitchControl><SwitchThumb /></SwitchControl>
          <SwitchLabel>Enable sandbox for this server</SwitchLabel>
        </Switch>

        <Show when={sandboxEnabled()}>
          <div style="padding-left: 0.5rem; border-left: 2px solid hsl(var(--border)); display: flex; flex-direction: column; gap: 0.75rem;">
            <Switch checked={sandboxReadWorkspace()} onChange={(checked) => setSandboxReadWorkspace(checked)} class="flex items-center gap-2">
              <SwitchControl><SwitchThumb /></SwitchControl>
              <SwitchLabel>Allow workspace read access</SwitchLabel>
            </Switch>

            <Switch checked={sandboxWriteWorkspace()} onChange={(checked) => setSandboxWriteWorkspace(checked)} class="flex items-center gap-2">
              <SwitchControl><SwitchThumb /></SwitchControl>
              <SwitchLabel>Allow workspace write access</SwitchLabel>
            </Switch>

            <Switch checked={sandboxAllowNetwork()} onChange={(checked) => setSandboxAllowNetwork(checked)} class="flex items-center gap-2">
              <SwitchControl><SwitchThumb /></SwitchControl>
              <SwitchLabel>Allow network access</SwitchLabel>
            </Switch>

            <div style="margin-top: 4px;">
              <span class="muted" style="font-size: 12px;">Additional read-only paths</span>
              <For each={sandboxExtraReadPaths()}>{(p, i) => (
                <div style="display: flex; gap: 4px; margin-top: 4px; align-items: center;">
                  <input type="text" value={p} style="flex: 1"
                    onChange={(e) => {
                      const paths = [...sandboxExtraReadPaths()];
                      paths[i()] = e.currentTarget.value;
                      setSandboxExtraReadPaths(paths);
                    }} />
                  <button style="padding: 2px 6px; font-size: 12px;" onClick={() => {
                    setSandboxExtraReadPaths(sandboxExtraReadPaths().filter((_, idx) => idx !== i()));
                  }}>✕</button>
                </div>
              )}</For>
              <button style="margin-top: 4px; font-size: 12px;" onClick={() => {
                setSandboxExtraReadPaths([...sandboxExtraReadPaths(), '']);
              }}>+ Add path</button>
            </div>

            <div style="margin-top: 4px;">
              <span class="muted" style="font-size: 12px;">Additional read-write paths</span>
              <For each={sandboxExtraWritePaths()}>{(p, i) => (
                <div style="display: flex; gap: 4px; margin-top: 4px; align-items: center;">
                  <input type="text" value={p} style="flex: 1"
                    onChange={(e) => {
                      const paths = [...sandboxExtraWritePaths()];
                      paths[i()] = e.currentTarget.value;
                      setSandboxExtraWritePaths(paths);
                    }} />
                  <button style="padding: 2px 6px; font-size: 12px;" onClick={() => {
                    setSandboxExtraWritePaths(sandboxExtraWritePaths().filter((_, idx) => idx !== i()));
                  }}>✕</button>
                </div>
              )}</For>
              <button style="margin-top: 4px; font-size: 12px;" onClick={() => {
                setSandboxExtraWritePaths([...sandboxExtraWritePaths(), '']);
              }}>+ Add path</button>
            </div>
          </div>
        </Show>

        <Show when={!sandboxEnabled()}>
          <p class="channel-form-hint">
            When disabled, this server will fall back to the global sandbox settings (if enabled in Security settings).
          </p>
        </Show>
      </div>
    </div>
  );

  const renderTestStep = () => (
    <div >
      <Show when={!testing() && !testPassed() && !testError()}>
        <div style="text-align: center; padding: 2rem 0;">
          <p style="margin-bottom: 1rem;">Ready to test the connection to <strong>{server_id()}</strong>.</p>
          <p class="channel-form-hint" style="margin-bottom: 1.5rem;">
            This will connect to the server, discover available tools, resources, and prompts, then disconnect.
          </p>
          <Button onClick={runTest}>Connect &amp; Test</Button>
        </div>
      </Show>

      <Show when={testing()}>
        <div style="text-align: center; padding: 2rem 0;">
          <LoaderCircle class="animate-spin" size={32} style="margin: 0 auto 1rem;" />
          <p>Connecting and discovering capabilities…</p>
        </div>
      </Show>

      <Show when={testError()}>
        <div style="padding: 1rem 0;">
          <div style="background: hsl(var(--destructive)); color: hsl(var(--destructive-foreground)); padding: 0.75rem 1rem; border-radius: 6px; margin-bottom: 1rem;">
            <strong>Connection failed</strong>
            <p style="margin-top: 0.25rem; font-size: 0.85rem;">{testError()}</p>
          </div>
          <Button variant="outline" onClick={runTest}>Retry</Button>
        </div>
      </Show>

      <Show when={testPassed()}>
        <div style="padding: 0.5rem 0;">
          <div style="background: var(--success, #22c55e); color: white; padding: 0.75rem 1rem; border-radius: 6px; margin-bottom: 1rem;">
            <strong>Connected successfully!</strong>
          </div>
          <Show when={discoveredTools().length > 0}>
            <div style="margin-bottom: 1rem;">
              <h4 style="display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.5rem;">
                <Wrench size={16} /> Tools ({discoveredTools().length})
              </h4>
              <div class="mcp-discovered-list">
                <For each={discoveredTools()}>
                  {(tool) => (
                    <div class="mcp-discovered-item">
                      <strong>{tool.name}</strong>
                      <Show when={tool.description}>
                        <span style="font-size: 0.8rem; margin-left: 0.5rem; color: hsl(var(--muted-foreground));">{tool.description}</span>
                      </Show>
                    </div>
                  )}
                </For>
              </div>
            </div>
          </Show>
          <Show when={discoveredResources().length > 0}>
            <div style="margin-bottom: 1rem;">
              <h4 style="display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.5rem;">
                <FileText size={16} /> Resources ({discoveredResources().length})
              </h4>
              <div class="mcp-discovered-list">
                <For each={discoveredResources()}>
                  {(resource) => (
                    <div class="mcp-discovered-item">
                      <strong>{resource.name}</strong>
                      <span class="text-muted" style="font-size: 0.8rem; margin-left: 0.5rem;">{resource.uri}</span>
                    </div>
                  )}
                </For>
              </div>
            </div>
          </Show>
          <Show when={discoveredPrompts().length > 0}>
            <div style="margin-bottom: 1rem;">
              <h4 style="display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.5rem;">
                <MessageSquare size={16} /> Prompts ({discoveredPrompts().length})
              </h4>
              <div class="mcp-discovered-list">
                <For each={discoveredPrompts()}>
                  {(prompt) => (
                    <div class="mcp-discovered-item">
                      <strong>{prompt.name}</strong>
                      <Show when={prompt.description}>
                        <span class="text-muted" style="font-size: 0.8rem; margin-left: 0.5rem;">{prompt.description}</span>
                      </Show>
                    </div>
                  )}
                </For>
              </div>
            </div>
          </Show>
          <Show when={discoveredTools().length === 0 && discoveredResources().length === 0 && discoveredPrompts().length === 0}>
            <p class="channel-form-hint">No tools, resources, or prompts were discovered.</p>
          </Show>
        </div>
      </Show>
    </div>
  );

  const renderReviewStep = () => {
    const config = buildConfig();
    return (
      <div >
        <div class="channel-summary">
          <div class="channel-summary-row">
            <span class="channel-summary-label">Server ID</span>
            <span class="channel-summary-value">{config.id}</span>
          </div>
          <div class="channel-summary-row">
            <span class="channel-summary-label">Transport</span>
            <span class="channel-summary-value">{config.transport}</span>
          </div>
          <Show when={config.transport === 'stdio'}>
            <div class="channel-summary-row">
              <span class="channel-summary-label">Command</span>
              <span class="channel-summary-value">{config.command} {config.args.join(' ')}</span>
            </div>
          </Show>
          <Show when={config.transport !== 'stdio'}>
            <div class="channel-summary-row">
              <span class="channel-summary-label">URL</span>
              <span class="channel-summary-value">{config.url}</span>
            </div>
          </Show>
          <Show when={Object.keys(config.env).length > 0}>
            <div class="channel-summary-row">
              <span class="channel-summary-label">Env vars</span>
              <span class="channel-summary-value">{Object.keys(config.env).join(', ')}</span>
            </div>
          </Show>
          <Show when={Object.keys(config.headers).length > 0}>
            <div class="channel-summary-row">
              <span class="channel-summary-label">Headers</span>
              <span class="channel-summary-value">
                {Object.entries(config.headers).map(([k, v]) =>
                  `${k}: ${v.type === 'secret-ref' ? '(keystore)' : v.value}`
                ).join(', ')}
              </span>
            </div>
          </Show>
          <div class="channel-summary-row">
            <span class="channel-summary-label">Auto-reconnect</span>
            <span class="channel-summary-value">{config.auto_reconnect ? 'Yes' : 'No'}</span>
          </div>
          <div class="channel-summary-row">
            <span class="channel-summary-label">Reactive</span>
            <span class="channel-summary-value">{config.reactive ? 'Yes' : 'No'}</span>
          </div>
          <Show when={config.transport === 'stdio'}>
            <div class="channel-summary-row">
              <span class="channel-summary-label">Sandbox</span>
              <span class="channel-summary-value">
                {config.sandbox?.enabled
                  ? `Enabled (network: ${config.sandbox.allow_network ? 'yes' : 'no'}, read: ${config.sandbox.read_workspace ? 'yes' : 'no'}, write: ${config.sandbox.write_workspace ? 'yes' : 'no'})`
                  : 'Disabled (uses global settings)'}
              </span>
            </div>
          </Show>
          <Show when={discoveredTools().length > 0}>
            <div class="channel-summary-row">
              <span class="channel-summary-label">Discovered</span>
              <span class="channel-summary-value">
                {discoveredTools().length} tool{discoveredTools().length !== 1 ? 's' : ''}
                {discoveredResources().length > 0 ? `, ${discoveredResources().length} resource${discoveredResources().length !== 1 ? 's' : ''}` : ''}
                {discoveredPrompts().length > 0 ? `, ${discoveredPrompts().length} prompt${discoveredPrompts().length !== 1 ? 's' : ''}` : ''}
              </span>
            </div>
          </Show>
          <Show when={registryServerName()}>
            <div class="channel-summary-row">
              <span class="channel-summary-label">Source</span>
              <span class="channel-summary-value">Registry: {registryServerName()}</span>
            </div>
          </Show>
        </div>
      </div>
    );
  };

  // ── Main render ──
  return (
    <>
    <Dialog open={true} onOpenChange={(open) => { if (!open) props.onClose(); }}>
      <DialogContent class="max-w-[700px] w-[90vw] max-h-[85vh] overflow-hidden flex flex-col p-0" onInteractOutside={(e) => e.preventDefault()}>
        <div class="channel-wizard-header">
          <h2>{isEditing() ? 'Edit MCP Server' : 'Add MCP Server'}</h2>
          <div class="wizard-steps">
            <For each={displaySteps()}>
              {(s, i) => {
                const stepIdx = () => WIZARD_STEPS.indexOf(s);
                return (
                  <>
                    <Show when={i() > 0}>
                      <div class="wizard-step-line" classList={{ completed: stepIdx() <= currentIdx() }} />
                    </Show>
                    <div
                      class="wizard-step"
                      classList={{
                        active: stepIdx() === currentIdx(),
                        completed: stepIdx() < currentIdx(),
                      }}
                    >
                      <div class="wizard-step-num">
                        {stepIdx() < currentIdx() ? '✓' : i() + 1}
                      </div>
                      <span class="wizard-step-label">{STEP_LABELS[s]}</span>
                    </div>
                  </>
                );
              }}
            </For>
          </div>
        </div>

        <div class="channel-wizard-body">
          <Show when={step() === 'source'}>{renderSourceStep()}</Show>
          <Show when={step() === 'transport'}>{renderTransportStep()}</Show>
          <Show when={step() === 'connection'}>{renderConnectionStep()}</Show>
          <Show when={step() === 'options'}>{renderOptionsStep()}</Show>
          <Show when={step() === 'sandbox'}>{renderSandboxStep()}</Show>
          <Show when={step() === 'test'}>{renderTestStep()}</Show>
          <Show when={step() === 'review'}>{renderReviewStep()}</Show>
        </div>

        <div class="channel-wizard-footer">
          <div style={{ display: 'flex', gap: '0.5rem' }}>
            <Button variant="outline" onClick={props.onClose}>Cancel</Button>
            <Show when={step() !== 'source' && !(isEditing() && step() === 'transport')}>
              <Button variant="outline" onClick={back}>← Back</Button>
            </Show>
          </div>
          <Show when={step() !== 'review'} fallback={
            <Button onClick={() => props.onFinish(buildConfig())}>
              {isEditing() ? 'Save Changes' : 'Add Server'}
            </Button>
          }>
            <Show when={step() !== 'source' && (step() !== 'transport' || (server_id().trim() && !idError()))}>
              <Button onClick={next} disabled={!canAdvance()}>
                Next →
              </Button>
            </Show>
          </Show>
        </div>
      </DialogContent>
    </Dialog>
    <Show when={showRegistryBrowser()}>
      <McpRegistryBrowser
        existingIds={props.existingIds}
        onSelect={handleRegistrySelect}
        onCancel={() => {
          setFromRegistry(false);
          setShowRegistryBrowser(false);
        }}
      />
    </Show>
    </>
  );
}
