/**
 * Playwright global setup: start the test daemon and write connection info.
 */
import { execSync, spawn, type ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';

const DAEMON_CONFIG_PATH = path.join(os.tmpdir(), 'hive-test-daemon.json');
const DAEMON_READY_TIMEOUT = 30_000;

let daemonProcess: ChildProcess | null = null;

export default async function globalSetup() {
  // Find the workspace root (where Cargo.toml is)
  const repoRoot = execSync('git rev-parse --show-toplevel', { encoding: 'utf8' }).trim();

  // Build the test daemon binary (skip if already built)
  console.log('[global-setup] Building test daemon…');
  execSync('cargo build --bin test_daemon -p hive-test-utils', {
    cwd: repoRoot,
    stdio: 'inherit',
    timeout: 300_000,
  });

  // Prepare the scripted provider config for our test scenarios
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
      {
        needle: 'Research the topic',
        responses: [
          {
            content: '',
            tool_calls: [
              {
                id: 'tc-ask-topic',
                name: 'core.ask_user',
                arguments: {
                  question: 'Which topic interests you?',
                  choices: ['Quantum computing', 'AI safety', 'Climate tech'],
                  allow_freeform: true,
                },
              },
            ],
          },
          { content: 'Research findings: user is interested in quantum computing.', tool_calls: [] },
        ],
      },
      {
        needle: 'Execute based on research',
        responses: [
          { content: 'Execution complete: generated report on quantum computing.', tool_calls: [] },
        ],
      },
      {
        needle: 'pricing document',
        responses: [
          { content: 'Based on our pricing document, our standard plan is $99/month.', tool_calls: [] },
        ],
      },
    ],
    default_responses: [{ content: 'I am the default test agent.', tool_calls: [] }],
  };

  // Write config to a temp file
  const config_path = path.join(os.tmpdir(), 'hive-daemon-config.json');
  fs.writeFileSync(config_path, JSON.stringify(scenarioConfig));

  // Start the daemon
  const daemonBin = path.join(repoRoot, 'target', 'debug', 'test_daemon');
  console.log('[global-setup] Starting test daemon…');
  daemonProcess = spawn(daemonBin, [config_path], {
    stdio: ['pipe', 'pipe', 'inherit'],
  });

  // Read the first line of stdout for connection info
  const daemonInfo = await new Promise<{ base_url: string; auth_token: string }>((resolve, reject) => {
    const timeout = setTimeout(() => {
      reject(new Error('Daemon did not produce connection info within timeout'));
    }, DAEMON_READY_TIMEOUT);

    let buffer = '';
    daemonProcess!.stdout!.on('data', (chunk: Buffer) => {
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

    daemonProcess!.on('error', (err) => {
      clearTimeout(timeout);
      reject(err);
    });

    daemonProcess!.on('exit', (code) => {
      clearTimeout(timeout);
      reject(new Error(`Daemon exited with code ${code} before producing output`));
    });
  });

  // Wait for healthcheck (heartbeat is POST, status is GET)
  const healthUrl = `${daemonInfo.base_url}/api/v1/status`;
  let healthy = false;
  for (let i = 0; i < 30; i++) {
    try {
      const resp = await fetch(healthUrl, {
        headers: { Authorization: `Bearer ${daemonInfo.auth_token}` },
      });
      if (resp.ok) {
        healthy = true;
        break;
      }
    } catch { /* retry */ }
    await new Promise((r) => setTimeout(r, 500));
  }
  if (!healthy) throw new Error('Daemon healthcheck failed');

  // Write connection info for tests to read
  fs.writeFileSync(
    DAEMON_CONFIG_PATH,
    JSON.stringify({
      baseUrl: daemonInfo.base_url,
      authToken: daemonInfo.auth_token,
      pid: daemonProcess.pid,
    }),
  );

  console.log(`[global-setup] Daemon ready at ${daemonInfo.base_url} (PID ${daemonProcess.pid})`);

  // Store process reference for teardown
  (globalThis as any).__HIVEMIND_DAEMON_PROCESS__ = daemonProcess;
}
