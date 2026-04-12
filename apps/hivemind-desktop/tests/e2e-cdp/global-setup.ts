/**
 * Global setup for CDP-based E2E tests.
 *
 * Orchestrates:
 * 1. Platform guard (Windows only)
 * 2. Start test_daemon with scripted LLM responses
 * 3. Store test daemon auth token in OS keyring
 * 4. Start Vite dev server (debug binaries use devUrl, not embedded assets)
 * 5. Launch the real HiveMind OS Tauri binary with CDP enabled
 * 6. Wait for CDP port and write connection info for tests
 */
import { execSync, spawn, type ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as net from 'net';
import * as path from 'path';
import * as os from 'os';

const CDP_PORT = 9515;
const VITE_PORT = 3000;
const CONFIG_PATH = path.join(os.tmpdir(), 'hivemind-cdp-test-config.json');
const ORIGINAL_TOKEN_PATH = path.join(os.tmpdir(), 'hivemind-cdp-original-token.json');
const DAEMON_READY_TIMEOUT = 30_000;
const VITE_READY_TIMEOUT = 60_000;
const APP_READY_TIMEOUT = 45_000;

/** Wait for a TCP port to become available. */
function waitForPort(port: number, host = '127.0.0.1', timeoutMs = 30_000): Promise<void> {
  const start = Date.now();
  return new Promise((resolve, reject) => {
    (function tryConnect() {
      if (Date.now() - start > timeoutMs) {
        return reject(new Error(`Timeout waiting for port ${port} after ${timeoutMs}ms`));
      }
      const sock = net.createConnection({ port, host }, () => {
        sock.destroy();
        resolve();
      });
      sock.on('error', () => setTimeout(tryConnect, 300));
    })();
  });
}

/** Wait for an HTTP endpoint to respond (handles IPv4/IPv6 via hostname). */
async function waitForHttpOk(url: string, timeoutMs = 30_000): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const resp = await fetch(url, { signal: AbortSignal.timeout(2000) });
      if (resp.ok || resp.status === 404) return;
    } catch { /* retry */ }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`Timeout waiting for ${url} after ${timeoutMs}ms`);
}

/** Find the repo root (where Cargo.toml workspace is). */
function repoRoot(): string {
  return execSync('git rev-parse --show-toplevel', { encoding: 'utf8' }).trim();
}

/** Find the HiveMind OS binary path. Supports env var override. */
function hivemindBinaryPath(): string {
  if (process.env.HIVEMIND_BINARY_PATH) {
    return process.env.HIVEMIND_BINARY_PATH;
  }
  const root = repoRoot();
  // The binary name comes from the Cargo package name (hivemind-desktop)
  // With --no-bundle it's hivemind-desktop.exe; bundled builds use the productName (HiveMind OS.exe)
  const candidates = [
    path.join(root, 'target', 'debug', 'hivemind-desktop.exe'),
    path.join(root, 'target', 'debug', 'HiveMind OS.exe'),
    path.join(root, 'target', 'release', 'hivemind-desktop.exe'),
    path.join(root, 'target', 'release', 'HiveMind OS.exe'),
  ];

  for (const p of candidates) {
    if (fs.existsSync(p)) return p;
  }

  throw new Error(
    `HiveMind OS binary not found. Run 'cargo tauri build --debug --no-bundle' first, ` +
    `or set HIVEMIND_BINARY_PATH env var. Checked:\n${candidates.map(c => '  ' + c).join('\n')}`
  );
}

/**
 * Read the HiveMind OS secrets blob from the Windows Credential Manager.
 * The keyring crate stores at target="secrets.hivemind", user="secrets".
 */
function readSecretsBlob(): Record<string, string> | null {
  try {
    const scriptPath = path.join(os.tmpdir(), 'hivemind-cdp-cred-read.ps1');
    fs.writeFileSync(scriptPath, `
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class CredReader {
    [DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
    public static extern bool CredRead(string target, int type, int flags, out IntPtr credential);
    [DllImport("advapi32.dll")]
    public static extern void CredFree(IntPtr credential);
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
    public struct CREDENTIAL {
        public int Flags; public int Type; public string TargetName; public string Comment;
        public long LastWritten; public int CredentialBlobSize; public IntPtr CredentialBlob;
        public int Persist; public int AttributeCount; public IntPtr Attributes;
        public string TargetAlias; public string UserName;
    }
    public static string Read(string target) {
        IntPtr p;
        if (!CredRead(target, 1, 0, out p)) return null;
        var c = (CREDENTIAL)Marshal.PtrToStructure(p, typeof(CREDENTIAL));
        string s = Marshal.PtrToStringUni(c.CredentialBlob, c.CredentialBlobSize / 2);
        CredFree(p);
        return s;
    }
}
'@
$r = [CredReader]::Read('secrets.hivemind')
if ($r) { Write-Output $r } else { Write-Output '{}' }
`);
    const result = execSync(`powershell -NoProfile -ExecutionPolicy Bypass -File "${scriptPath}"`, {
      encoding: 'utf8',
      timeout: 15_000,
    }).trim();
    fs.unlinkSync(scriptPath);
    return JSON.parse(result);
  } catch {
    return null;
  }
}

/**
 * Write the HiveMind OS secrets blob to the Windows Credential Manager.
 */
function writeSecretsBlob(secrets: Record<string, string>): void {
  const jsonStr = JSON.stringify(secrets);
  const payloadPath = path.join(os.tmpdir(), 'hivemind-cdp-cred-payload.json');
  fs.writeFileSync(payloadPath, jsonStr);

  const scriptPath = path.join(os.tmpdir(), 'hivemind-cdp-cred-write.ps1');
  fs.writeFileSync(scriptPath, `
$json = Get-Content -Raw -Path '${payloadPath.replace(/\\/g, '\\\\')}'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class CredWriter {
    [DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
    public static extern bool CredWrite(ref CREDENTIAL credential, int flags);
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
    public struct CREDENTIAL {
        public int Flags; public int Type; public string TargetName; public string Comment;
        public long LastWritten; public int CredentialBlobSize; public IntPtr CredentialBlob;
        public int Persist; public int AttributeCount; public IntPtr Attributes;
        public string TargetAlias; public string UserName;
    }
    public static bool Write(string target, string user, string password) {
        var bytes = Encoding.Unicode.GetBytes(password);
        var c = new CREDENTIAL();
        c.Type = 1; c.TargetName = target; c.UserName = user; c.Persist = 2;
        c.CredentialBlobSize = bytes.Length;
        c.CredentialBlob = Marshal.AllocHGlobal(bytes.Length);
        Marshal.Copy(bytes, 0, c.CredentialBlob, bytes.Length);
        bool ok = CredWrite(ref c, 0);
        Marshal.FreeHGlobal(c.CredentialBlob);
        return ok;
    }
}
'@
[CredWriter]::Write('secrets.hivemind', 'secrets', $json)
`);
  execSync(`powershell -NoProfile -ExecutionPolicy Bypass -File "${scriptPath}"`, {
    encoding: 'utf8',
    timeout: 15_000,
  });
  fs.unlinkSync(scriptPath);
  fs.unlinkSync(payloadPath);
}

/**
 * Store the test daemon's auth token in the OS keyring so the HiveMind OS Tauri
 * app can authenticate with the test daemon. Saves the original token
 * for restoration in global-teardown.
 */
function injectDaemonToken(testToken: string): void {
  const secrets = readSecretsBlob() || {};
  // Save original for teardown restoration
  fs.writeFileSync(ORIGINAL_TOKEN_PATH, JSON.stringify({
    hadToken: 'daemon:auth-token' in secrets,
    originalToken: secrets['daemon:auth-token'] || null,
  }));
  secrets['daemon:auth-token'] = testToken;
  writeSecretsBlob(secrets);
  console.log('[cdp-setup] Injected test daemon token into OS keyring');
}

export default async function globalSetup() {
  // ── Platform guard ────────────────────────────────────────────────
  if (process.platform !== 'win32') {
    console.log('[cdp-setup] Skipping: CDP tests only run on Windows (WebView2).');
    // Write a config that tells tests to skip
    fs.writeFileSync(CONFIG_PATH, JSON.stringify({ skip: true }));
    return;
  }

  const root = repoRoot();

  // ── 1. Start test_daemon ──────────────────────────────────────────
  console.log('[cdp-setup] Building test daemon…');
  execSync('cargo build --bin test_daemon -p hive-test-utils', {
    cwd: root,
    stdio: 'inherit',
    timeout: 300_000,
  });

  const scenarioConfig = {
    rules: [
      {
        needle: 'Ask the user what color they prefer',
        responses: [
          {
            content: '',
            tool_calls: [
              {
                id: 'tc-ask',
                name: 'core.ask_user',
                arguments: {
                  question: 'What is your favorite color?',
                  choices: ['Red', 'Blue', 'Green'],
                  allow_freeform: false,
                },
              },
            ],
          },
          { content: "The user's favorite color is Blue.", tool_calls: [] },
        ],
      },
    ],
    default_responses: [
      { content: 'Hello from the test daemon! This is a CDP E2E test.', tool_calls: [] },
    ],
  };

  const scenarioConfigPath = path.join(os.tmpdir(), 'hivemind-cdp-daemon-config.json');
  fs.writeFileSync(scenarioConfigPath, JSON.stringify(scenarioConfig));

  const daemonBin = path.join(root, 'target', 'debug', 'test_daemon');
  console.log('[cdp-setup] Starting test daemon…');
  const daemonProcess = spawn(daemonBin, [scenarioConfigPath], {
    stdio: ['pipe', 'pipe', 'inherit'],
  });

  // Read daemon connection info from stdout
  const daemonInfo = await new Promise<{ base_url: string; auth_token: string }>((resolve, reject) => {
    const timeout = setTimeout(() => {
      reject(new Error('Test daemon did not produce connection info within timeout'));
    }, DAEMON_READY_TIMEOUT);

    let buffer = '';
    daemonProcess.stdout!.on('data', (chunk: Buffer) => {
      buffer += chunk.toString();
      const newline = buffer.indexOf('\n');
      if (newline >= 0) {
        clearTimeout(timeout);
        try {
          resolve(JSON.parse(buffer.slice(0, newline)));
        } catch (e) {
          reject(new Error(`Failed to parse daemon output: ${buffer.slice(0, newline)}`));
        }
      }
    });

    daemonProcess.on('error', (err) => { clearTimeout(timeout); reject(err); });
    daemonProcess.on('exit', (code) => {
      clearTimeout(timeout);
      reject(new Error(`Daemon exited with code ${code} before producing output`));
    });
  });

  // Wait for daemon healthcheck
  const healthUrl = `${daemonInfo.base_url}/api/v1/status`;
  let healthy = false;
  for (let i = 0; i < 30; i++) {
    try {
      const resp = await fetch(healthUrl, {
        headers: { Authorization: `Bearer ${daemonInfo.auth_token}` },
      });
      if (resp.ok) { healthy = true; break; }
    } catch { /* retry */ }
    await new Promise((r) => setTimeout(r, 500));
  }
  if (!healthy) throw new Error('Test daemon healthcheck failed');

  // Mark setup as completed so the app doesn't show the setup wizard
  const configResp = await fetch(`${daemonInfo.base_url}/api/v1/config/get`, {
    headers: { Authorization: `Bearer ${daemonInfo.auth_token}` },
  });
  if (configResp.ok) {
    const config = await configResp.json();
    config.setup_completed = true;
    await fetch(`${daemonInfo.base_url}/api/v1/config`, {
      method: 'PUT',
      headers: {
        Authorization: `Bearer ${daemonInfo.auth_token}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(config),
    });
    console.log('[cdp-setup] Set setup_completed=true in daemon config');
  }

  console.log(`[cdp-setup] Test daemon ready at ${daemonInfo.base_url} (PID ${daemonProcess.pid})`);

  // ── 2. Inject test daemon auth token into OS keyring ───────────────
  // The HiveMind OS Tauri app reads the daemon auth token from the OS keyring.
  // We must inject the test daemon's token so the app can authenticate.
  injectDaemonToken(daemonInfo.auth_token);

  // ── 3. Start Vite dev server ────────────────────────────────────
  // Debug-profile Tauri binaries use devUrl (http://localhost:3000), not embedded assets.
  // We must start the Vite dev server before launching the app.
  const viteDir = path.join(root, 'apps', 'hivemind-desktop', 'node_modules', '.vite');
  if (fs.existsSync(viteDir)) {
    try {
      fs.rmSync(viteDir, { recursive: true, force: true });
      console.log('[cdp-setup] Cleared stale .vite cache');
    } catch {
      console.warn('[cdp-setup] Could not clear .vite cache (may be locked)');
    }
  }

  // Check if Vite is already running on port 3000 (may bind to IPv6 ::1 or IPv4)
  let viteProcess: ChildProcess | null = null;
  let viteAlreadyRunning = false;
  try {
    const resp = await fetch(`http://localhost:${VITE_PORT}`, { signal: AbortSignal.timeout(2000) });
    if (resp.ok || resp.status === 404) {
      viteAlreadyRunning = true;
      console.log(`[cdp-setup] Vite dev server already running on port ${VITE_PORT}`);
    }
  } catch { /* not running */ }

  if (!viteAlreadyRunning) {
    console.log('[cdp-setup] Starting Vite dev server…');
    const appDir = path.join(root, 'apps', 'hivemind-desktop');
    viteProcess = spawn('npx', ['vite', '--port', String(VITE_PORT)], {
      cwd: appDir,
      stdio: 'pipe',
      shell: true,
      env: { ...process.env },
    });

    viteProcess.on('error', (err) => {
      console.error('[cdp-setup] Vite dev server error:', err);
    });

    // Wait for Vite to be ready (use localhost which resolves to IPv4 or IPv6)
    console.log(`[cdp-setup] Waiting for Vite on port ${VITE_PORT}…`);
    await waitForHttpOk(`http://localhost:${VITE_PORT}`, VITE_READY_TIMEOUT);
    console.log(`[cdp-setup] Vite dev server ready on port ${VITE_PORT} (PID ${viteProcess.pid})`);
  }

  // ── 4. Launch the real HiveMind OS binary with CDP ──────────────────────
  const binaryPath = hivemindBinaryPath();
  console.log(`[cdp-setup] Launching HiveMind OS binary: ${binaryPath}`);

  const appEnv = {
    ...process.env,
    // Point the HiveMind OS app at our test daemon
    HIVEMIND_DAEMON_URL: daemonInfo.base_url,
    // Enable CDP on WebView2
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}`,
  };

  const appProcess = spawn(binaryPath, [], {
    env: appEnv,
    stdio: 'pipe',
  });

  appProcess.on('error', (err) => {
    console.error('[cdp-setup] HiveMind OS binary error:', err);
  });

  // ── 5. Wait for CDP port ──────────────────────────────────────────
  console.log(`[cdp-setup] Waiting for CDP on port ${CDP_PORT}…`);
  await waitForPort(CDP_PORT, '127.0.0.1', APP_READY_TIMEOUT);
  console.log(`[cdp-setup] CDP port ${CDP_PORT} is ready.`);

  // ── 6. Write config for tests ─────────────────────────────────────
  fs.writeFileSync(
    CONFIG_PATH,
    JSON.stringify({
      skip: false,
      cdpUrl: `http://127.0.0.1:${CDP_PORT}`,
      daemonBaseUrl: daemonInfo.base_url,
      daemonAuthToken: daemonInfo.auth_token,
      appPid: appProcess.pid,
      daemonPid: daemonProcess.pid,
      vitePid: viteProcess?.pid ?? null,
    }),
  );

  console.log(`[cdp-setup] HiveMind OS app ready (PID ${appProcess.pid}), CDP at http://127.0.0.1:${CDP_PORT}`);
}
