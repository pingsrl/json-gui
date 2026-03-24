import { spawn } from 'node:child_process';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, '..');
const SRC_TAURI = path.resolve(ROOT, 'src-tauri');
const WDIO_DIR = path.resolve(ROOT, 'tests/e2e/wdio');
const NPM_CMD = process.platform === 'win32' ? 'npm.cmd' : 'npm';
const CARGO_CMD = process.platform === 'win32' ? 'cargo.exe' : 'cargo';
const TAURI_WD_CMD = process.platform === 'win32' ? 'tauri-wd.exe' : 'tauri-wd';
const PORT = 4444;

function run(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      stdio: 'inherit',
      ...options,
    });

    child.on('error', reject);
    child.on('exit', (code, signal) => {
      if (signal) {
        reject(new Error(`${command} terminated by signal ${signal}`));
        return;
      }
      resolve(code ?? 0);
    });
  });
}

async function waitForServer(port, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const isOpen = await new Promise((resolve) => {
      const socket = net.createConnection({ host: '127.0.0.1', port });
      socket.once('connect', () => {
        socket.end();
        resolve(true);
      });
      socket.once('error', () => resolve(false));
    });
    if (isOpen) return;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`tauri-wd not ready on port ${port} after ${timeoutMs}ms`);
}

async function stopProcess(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) {
    return;
  }
  child.kill('SIGTERM');
  await new Promise((resolve) => setTimeout(resolve, 500));
  if (child.exitCode === null && child.signalCode === null) {
    child.kill('SIGKILL');
  }
}

let tauriWd = null;

try {
  const buildExitCode = await run(CARGO_CMD, ['build'], { cwd: SRC_TAURI });
  if (buildExitCode !== 0) {
    process.exit(buildExitCode);
  }

  tauriWd = spawn(TAURI_WD_CMD, ['--port', String(PORT)], {
    cwd: ROOT,
    stdio: 'inherit',
  });
  tauriWd.on('error', (error) => {
    console.error(error);
  });

  await waitForServer(PORT);

  const extraArgs = process.argv.slice(2);
  const npmArgs = ['test'];
  if (extraArgs.length > 0) {
    npmArgs.push('--', ...extraArgs);
  }

  const testExitCode = await run(NPM_CMD, npmArgs, { cwd: WDIO_DIR });
  process.exitCode = testExitCode;
} finally {
  await stopProcess(tauriWd);
}
