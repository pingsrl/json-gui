#!/usr/bin/env node

import fs from 'node:fs';
import os from 'node:os';
import { spawn, spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REAL_BIN = process.env.TAURI_REAL_APP_BIN
  || path.resolve(__dirname, '../src-tauri/target/debug/json-gui');
const PID_FILE = path.join(os.tmpdir(), 'jsongui-wdio-app.json');

function readTrackedApp() {
  try {
    return JSON.parse(fs.readFileSync(PID_FILE, 'utf8'));
  } catch {
    return null;
  }
}

function writeTrackedApp(pid) {
  fs.writeFileSync(PID_FILE, JSON.stringify({ pid, binary: REAL_BIN }));
}

function clearTrackedApp(expectedPid) {
  const tracked = readTrackedApp();
  if (tracked?.pid !== expectedPid) {
    return;
  }
  try {
    fs.unlinkSync(PID_FILE);
  } catch {
    // ignore missing pid file
  }
}

function isProcessAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

function processMatchesBinary(pid) {
  const result = spawnSync('ps', ['-p', String(pid), '-o', 'command='], {
    encoding: 'utf8',
  });
  return result.status === 0 && result.stdout.includes(REAL_BIN);
}

async function waitForProcessExit(pid, timeoutMs = 2000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (!isProcessAlive(pid)) {
      return true;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return !isProcessAlive(pid);
}

async function killTrackedProcess(pid) {
  if (!isProcessAlive(pid)) {
    return;
  }
  try {
    process.kill(pid, 'SIGTERM');
  } catch {
    return;
  }
  if (await waitForProcessExit(pid)) {
    return;
  }
  try {
    process.kill(pid, 'SIGKILL');
  } catch {
    // already gone
  }
  await waitForProcessExit(pid, 500);
}

async function cleanupStaleApp() {
  const tracked = readTrackedApp();
  if (!tracked?.pid) {
    return;
  }
  if (!isProcessAlive(tracked.pid)) {
    clearTrackedApp(tracked.pid);
    return;
  }
  if (!processMatchesBinary(tracked.pid)) {
    clearTrackedApp(tracked.pid);
    return;
  }
  await killTrackedProcess(tracked.pid);
  clearTrackedApp(tracked.pid);
}

await cleanupStaleApp();

const child = spawn(REAL_BIN, process.argv.slice(2), {
  stdio: ['ignore', 'pipe', 'inherit'],
  env: process.env,
});
writeTrackedApp(child.pid);

let buffered = '';
let released = false;
let shuttingDown = false;

function forward(chunk) {
  process.stdout.write(chunk);
}

async function waitForMainWindow(port) {
  const deadline = Date.now() + 15000;

  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/window/handles`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: '{}',
      });
      if (response.ok) {
        const handles = await response.json();
        if (Array.isArray(handles) && handles.length > 0) {
          return true;
        }
      }
    } catch {
      // plugin not ready yet
    }
    await new Promise((resolve) => setTimeout(resolve, 150));
  }

  return false;
}

async function cleanupCurrentChild() {
  if (shuttingDown) {
    return;
  }
  shuttingDown = true;
  if (child.exitCode !== null || child.signalCode !== null) {
    clearTrackedApp(child.pid);
    return;
  }
  await killTrackedProcess(child.pid);
  clearTrackedApp(child.pid);
}

child.stdout.on('data', async (chunk) => {
  const text = chunk.toString();

  if (released) {
    forward(text);
    return;
  }

  buffered += text;
  const match = buffered.match(/\[webdriver\] listening on port (\d+)/);
  if (!match) {
    return;
  }

  const ready = await waitForMainWindow(match[1]);
  released = true;
  forward(buffered);
  buffered = '';

  if (!ready) {
    console.error('[wdio-wrapper] main window non pronta entro il timeout');
  }
});

child.on('exit', (code, signal) => {
  clearTrackedApp(child.pid);
  if (!released && buffered) {
    forward(buffered);
  }
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 0);
});

for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
  process.on(signal, () => {
    void cleanupCurrentChild().finally(() => {
      process.exit(0);
    });
  });
}

process.on('exit', () => {
  if (child.exitCode === null && child.signalCode === null) {
    try {
      child.kill('SIGTERM');
    } catch {
      // already gone
    }
  }
});

process.on('uncaughtException', (error) => {
  console.error(error);
  void cleanupCurrentChild().finally(() => {
    process.exit(1);
  });
});

process.on('unhandledRejection', (reason) => {
  console.error(reason);
  void cleanupCurrentChild().finally(() => {
    process.exit(1);
  });
});
