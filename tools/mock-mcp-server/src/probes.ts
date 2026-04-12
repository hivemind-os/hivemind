import { readFile, writeFile, readdir, stat } from 'fs/promises';
import path from 'path';

export interface ProbeResult {
  success: boolean;
  duration: number;
  data?: unknown;
  error?: string;
}

const MAX_FILE_READ_BYTES = 64 * 1024; // 64KB
const NETWORK_TIMEOUT_MS = 10_000; // 10s

export async function probeFileRead(filePath: string): Promise<ProbeResult> {
  const start = performance.now();
  try {
    const buf = await readFile(filePath);
    const truncated = buf.length > MAX_FILE_READ_BYTES;
    const content = buf.subarray(0, MAX_FILE_READ_BYTES).toString('utf-8');
    return {
      success: true,
      duration: Math.round(performance.now() - start),
      data: {
        path: filePath,
        size: buf.length,
        truncated,
        content,
      },
    };
  } catch (err: any) {
    return {
      success: false,
      duration: Math.round(performance.now() - start),
      error: `${err.code ?? 'UNKNOWN'}: ${err.message}`,
    };
  }
}

export async function probeFileWrite(
  filePath: string,
  content: string,
): Promise<ProbeResult> {
  const start = performance.now();
  try {
    await writeFile(filePath, content, 'utf-8');
    return {
      success: true,
      duration: Math.round(performance.now() - start),
      data: {
        path: filePath,
        bytesWritten: Buffer.byteLength(content, 'utf-8'),
      },
    };
  } catch (err: any) {
    return {
      success: false,
      duration: Math.round(performance.now() - start),
      error: `${err.code ?? 'UNKNOWN'}: ${err.message}`,
    };
  }
}

export async function probeDirList(dirPath: string): Promise<ProbeResult> {
  const start = performance.now();
  try {
    const entries = await readdir(dirPath, { withFileTypes: true });
    const items = await Promise.all(
      entries.slice(0, 200).map(async (e) => {
        const fullPath = path.join(dirPath, e.name);
        let size: number | undefined;
        if (e.isFile()) {
          try {
            const s = await stat(fullPath);
            size = s.size;
          } catch {
            // ignore stat errors
          }
        }
        return {
          name: e.name,
          type: e.isDirectory() ? 'directory' : 'file',
          ...(size !== undefined && { size }),
        };
      }),
    );
    return {
      success: true,
      duration: Math.round(performance.now() - start),
      data: {
        path: dirPath,
        count: entries.length,
        truncated: entries.length > 200,
        entries: items,
      },
    };
  } catch (err: any) {
    return {
      success: false,
      duration: Math.round(performance.now() - start),
      error: `${err.code ?? 'UNKNOWN'}: ${err.message}`,
    };
  }
}

export async function probeNetwork(
  url: string,
  method: string = 'GET',
  body?: string,
  headers?: Record<string, string>,
): Promise<ProbeResult> {
  const start = performance.now();
  try {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), NETWORK_TIMEOUT_MS);

    const init: RequestInit = {
      method,
      signal: controller.signal,
      headers: {
        ...headers,
      },
    };

    if (body && method !== 'GET' && method !== 'HEAD') {
      init.body = body;
    }

    const res = await fetch(url, init);
    clearTimeout(timer);

    const responseBody = await res.text();
    const truncated = responseBody.length > MAX_FILE_READ_BYTES;

    return {
      success: true,
      duration: Math.round(performance.now() - start),
      data: {
        url,
        status: res.status,
        statusText: res.statusText,
        headers: Object.fromEntries(res.headers.entries()),
        bodyLength: responseBody.length,
        body: responseBody.slice(0, MAX_FILE_READ_BYTES),
        truncated,
      },
    };
  } catch (err: any) {
    const message = err.name === 'AbortError'
      ? `Request timed out after ${NETWORK_TIMEOUT_MS}ms`
      : err.message;
    return {
      success: false,
      duration: Math.round(performance.now() - start),
      error: message,
    };
  }
}
