/**
 * Global teardown for CDP-based E2E tests.
 * Kills the HiveMind OS app and test daemon processes, restores the OS keyring.
 */
import { execSync } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';

const CONFIG_PATH = path.join(os.tmpdir(), 'hivemind-cdp-test-config.json');
const ORIGINAL_TOKEN_PATH = path.join(os.tmpdir(), 'hivemind-cdp-original-token.json');

/**
 * Read the HiveMind OS secrets blob from the Windows Credential Manager.
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

/** Restore the original daemon auth token in the OS keyring. */
function restoreDaemonToken(): void {
  try {
    const raw = fs.readFileSync(ORIGINAL_TOKEN_PATH, 'utf8');
    const { hadToken, originalToken } = JSON.parse(raw);
    const secrets = readSecretsBlob() || {};
    if (hadToken && originalToken) {
      secrets['daemon:auth-token'] = originalToken;
    } else {
      delete secrets['daemon:auth-token'];
    }
    writeSecretsBlob(secrets);
    console.log('[cdp-teardown] Restored original daemon token in OS keyring');
    fs.unlinkSync(ORIGINAL_TOKEN_PATH);
  } catch {
    // Original token file may not exist if setup failed before injection
  }
}

/** Gracefully stop a process: try SIGTERM, wait, then force kill the process tree. */
async function gracefulKill(pid: number, label: string, gracePeriodMs = 3000): Promise<void> {
  try {
    process.kill(pid, 0); // check if alive
  } catch {
    return; // already dead
  }

  console.log(`[cdp-teardown] Stopping ${label} (PID ${pid})…`);
  try {
    // On Windows, SIGTERM doesn't work for GUI apps. Use taskkill without /F first
    // for a graceful WM_CLOSE, then force if still alive.
    if (process.platform === 'win32') {
      try {
        execSync(`taskkill /PID ${pid}`, { timeout: 5000, stdio: 'ignore' });
      } catch { /* may fail if already closing */ }
    } else {
      process.kill(pid, 'SIGTERM');
    }

    // Wait for graceful exit
    const deadline = Date.now() + gracePeriodMs;
    while (Date.now() < deadline) {
      try { process.kill(pid, 0); } catch { return; } // exited
      await new Promise(r => setTimeout(r, 200));
    }

    // Force kill the entire process tree (/T) to ensure WebView2 children are cleaned up
    try {
      process.kill(pid, 0); // still alive?
      if (process.platform === 'win32') {
        execSync(`taskkill /F /T /PID ${pid}`, { timeout: 5000, stdio: 'ignore' });
      } else {
        process.kill(pid, 'SIGKILL');
      }
      console.log(`[cdp-teardown] Force-killed ${label} (tree)`);
    } catch { /* already dead */ }
  } catch (e: unknown) {
    const err = e as NodeJS.ErrnoException;
    if (err.code !== 'ESRCH') {
      console.warn(`[cdp-teardown] Error killing ${label}:`, err.message);
    }
  }
}

export default async function globalTeardown() {
  try {
    const raw = fs.readFileSync(CONFIG_PATH, 'utf8');
    const config = JSON.parse(raw);

    if (config.skip) {
      console.log('[cdp-teardown] Tests were skipped (non-Windows platform).');
      fs.unlinkSync(CONFIG_PATH);
      return;
    }

    // Kill the HiveMind OS app (graceful shutdown to let Vite clean up)
    if (config.appPid) {
      await gracefulKill(config.appPid, 'HiveMind OS app');
    }

    // Kill the Vite dev server (if we started it).
    // The shell PID may have already exited while the actual node process orphaned,
    // so also find and kill whatever is listening on port 3000.
    if (config.vitePid) {
      await gracefulKill(config.vitePid, 'Vite dev server (shell)', 1000);
      // The shell wrapper often dies while the node/vite child survives.
      // Find and kill the actual process on port 3000.
      try {
        const portOutput = execSync(
          'netstat -ano | findstr ":3000.*LISTENING"',
          { encoding: 'utf8', timeout: 5000 },
        ).trim();
        const lines = portOutput.split('\n').filter(l => l.includes('LISTENING'));
        for (const line of lines) {
          const pid = parseInt(line.trim().split(/\s+/).pop() || '', 10);
          if (pid > 0) {
            try {
              execSync(`taskkill /F /T /PID ${pid}`, { timeout: 5000, stdio: 'ignore' });
              console.log(`[cdp-teardown] Killed Vite process on port 3000 (PID ${pid})`);
            } catch { /* already dead */ }
          }
        }
      } catch { /* no process on port 3000 — already cleaned up */ }
    }

    // Kill the test daemon
    if (config.daemonPid) {
      await gracefulKill(config.daemonPid, 'test daemon', 2000);
    }

    // Restore the original daemon token in the OS keyring
    restoreDaemonToken();

    // Wait a beat for file handles to be fully released after process tree kill
    await new Promise(r => setTimeout(r, 1000));

    // Clean up entire .vite cache (deps + stale deps_temp_* dirs) to prevent EPERM on next run
    try {
      const root = execSync('git rev-parse --show-toplevel', { encoding: 'utf8' }).trim();
      const viteDir = path.join(root, 'apps', 'hivemind-desktop', 'node_modules', '.vite');
      if (fs.existsSync(viteDir)) {
        fs.rmSync(viteDir, { recursive: true, force: true });
        console.log('[cdp-teardown] Cleared .vite cache');
      }
    } catch { /* best effort */ }

    fs.unlinkSync(CONFIG_PATH);
  } catch {
    // Config file might not exist if setup failed
  }

  console.log('[cdp-teardown] Done.');
}
