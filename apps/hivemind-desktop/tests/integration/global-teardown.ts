/**
 * Playwright global teardown: stop the test daemon.
 */
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';

const DAEMON_CONFIG_PATH = path.join(os.tmpdir(), 'hive-test-daemon.json');

export default async function globalTeardown() {
  // Read the daemon PID
  try {
    const config = JSON.parse(fs.readFileSync(DAEMON_CONFIG_PATH, 'utf8'));
    if (config.pid) {
      console.log(`[global-teardown] Stopping daemon (PID ${config.pid})…`);
      try {
        process.kill(config.pid, 'SIGTERM');
      } catch (e: unknown) {
        const err = e as NodeJS.ErrnoException;
        if (err.code !== 'ESRCH') {
          console.warn('[global-teardown] Failed to kill daemon:', err);
        }
      }
    }
    fs.unlinkSync(DAEMON_CONFIG_PATH);
  } catch {
    // Config file might not exist if setup failed
  }

  // Also try the global reference
  const proc = (globalThis as any).__HIVEMIND_DAEMON_PROCESS__;
  if (proc && !proc.killed) {
    proc.kill('SIGTERM');
  }

  console.log('[global-teardown] Done.');
}
