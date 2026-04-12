#!/usr/bin/env node
/**
 * Integration test: spawn mock-mcp-server in stdio mode
 * and run the full MCP handshake + tool calls, matching
 * exactly what rmcp (the Rust client) sends.
 */
import { spawn } from 'child_process';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

function send(child, obj) {
  const line = JSON.stringify(obj) + '\n';
  child.stdin.write(line);
}

function waitForResponse(child, timeoutMs = 10000) {
  return new Promise((resolve, reject) => {
    let buf = '';
    const timer = setTimeout(() => {
      child.stdout.removeListener('data', onData);
      reject(new Error(`Timed out waiting for response (${timeoutMs}ms)`));
    }, timeoutMs);

    function onData(chunk) {
      buf += chunk.toString();
      const idx = buf.indexOf('\n');
      if (idx !== -1) {
        clearTimeout(timer);
        child.stdout.removeListener('data', onData);
        const line = buf.slice(0, idx);
        resolve(JSON.parse(line));
      }
    }
    child.stdout.on('data', onData);
  });
}

async function main() {
  let failures = 0;

  // Use --dashboard-port 0 to avoid port conflicts (won't start dashboard)
  const child = spawn('node', ['dist/index.js', '--dashboard-port', '0'], {
    stdio: ['pipe', 'pipe', 'pipe'],
    cwd: __dirname,
  });

  let stderr = '';
  child.stderr.on('data', (d) => { stderr += d.toString(); });
  child.on('exit', (code, signal) => {
    if (code !== null && code !== 0) {
      console.error(`Child exited with code ${code}`);
      console.error('STDERR:', stderr);
    }
  });

  try {
    // === Test 1: Initialize handshake (rmcp sends id:0) ===
    console.log('Test 1: Initialize handshake');
    send(child, {
      jsonrpc: '2.0',
      id: 0,
      method: 'initialize',
      params: {
        protocolVersion: '2024-11-05',
        capabilities: {},
        clientInfo: { name: 'rmcp-test', version: '0.1.0' },
      },
    });

    const initResp = await waitForResponse(child);
    if (initResp.jsonrpc !== '2.0') { console.error('  FAIL: jsonrpc != 2.0'); failures++; }
    if (initResp.id !== 0) { console.error(`  FAIL: id=${initResp.id}, expected 0`); failures++; }
    if (!initResp.result) { console.error('  FAIL: no result field'); failures++; }
    else {
      if (!initResp.result.protocolVersion) { console.error('  FAIL: missing protocolVersion'); failures++; }
      if (!initResp.result.capabilities) { console.error('  FAIL: missing capabilities'); failures++; }
      if (!initResp.result.serverInfo) { console.error('  FAIL: missing serverInfo'); failures++; }
    }
    if (failures === 0) console.log('  PASS');

    // === Send initialized notification ===
    send(child, { jsonrpc: '2.0', method: 'notifications/initialized' });

    // === Test 2: tools/list ===
    console.log('Test 2: tools/list');
    send(child, { jsonrpc: '2.0', id: 1, method: 'tools/list', params: {} });
    const listResp = await waitForResponse(child);
    if (listResp.id !== 1) { console.error(`  FAIL: id=${listResp.id}`); failures++; }
    if (!Array.isArray(listResp.result?.tools)) { console.error('  FAIL: result.tools not array'); failures++; }
    else { console.log(`  PASS (${listResp.result.tools.length} tools)`); }

    // === Test 3: tools/call ===
    console.log('Test 3: tools/call get_weather');
    send(child, {
      jsonrpc: '2.0', id: 2, method: 'tools/call',
      params: { name: 'get_weather', arguments: { city: 'Seattle' } },
    });
    const callResp = await waitForResponse(child);
    if (callResp.id !== 2) { console.error(`  FAIL: id=${callResp.id}`); failures++; }
    if (!Array.isArray(callResp.result?.content)) { console.error('  FAIL: result.content not array'); failures++; }
    else {
      const text = callResp.result.content.find(c => c.type === 'text')?.text || '';
      if (!text.includes('Seattle')) { console.error('  FAIL: response does not mention Seattle'); failures++; }
      else { console.log('  PASS'); }
    }

    // === Test 4: tools/call unknown tool ===
    console.log('Test 4: tools/call unknown_tool');
    send(child, {
      jsonrpc: '2.0', id: 3, method: 'tools/call',
      params: { name: 'nonexistent_tool', arguments: {} },
    });
    const unknownResp = await waitForResponse(child);
    if (unknownResp.id !== 3) { console.error(`  FAIL: id=${unknownResp.id}`); failures++; }
    if (!unknownResp.result?.isError) { console.error('  FAIL: expected isError=true'); failures++; }
    else { console.log('  PASS'); }

  } catch (e) {
    console.error(`FATAL: ${e.message}`);
    console.error('STDERR:', stderr);
    failures++;
  } finally {
    child.kill();
  }

  if (failures === 0) {
    console.log('\n✅ ALL TESTS PASSED');
    process.exit(0);
  } else {
    console.error(`\n❌ ${failures} FAILURE(S)`);
    process.exit(1);
  }
}

main();
