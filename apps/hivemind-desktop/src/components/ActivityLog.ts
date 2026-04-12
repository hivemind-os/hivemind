import { invoke } from '@tauri-apps/api/core';

function writeLog(level: string, source: string, message: string) {
  invoke('write_frontend_log', { level, source, message }).catch(() => {
    // Last resort — if we can't even write the log, print to console.
    console.error(`[${level}] [${source}] ${message}`);
  });
}

export function logInfo(source: string, message: string) {
  writeLog('INFO', source, message);
}

export function logWarn(source: string, message: string) {
  writeLog('WARN', source, message);
}

export function logError(source: string, message: string) {
  writeLog('ERROR', source, message);
}
