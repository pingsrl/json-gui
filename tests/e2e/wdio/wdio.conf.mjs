import path from 'path';
import { fileURLToPath } from 'url';
import { spawn } from 'child_process';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, '../../..');

const APP_BIN = process.env.TAURI_APP_PATH
  || path.resolve(ROOT, 'src-tauri/target/debug/json-gui');

const FIXTURE_PATH = path.resolve(__dirname, '../fixtures/test.json');
const VITE_PORT = 1420;

let viteProcess = null;

/** Aspetta che Vite sia pronto su localhost:PORT. */
async function waitForVite(port, timeout = 30000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`http://localhost:${port}/`);
      if (res.ok || res.status === 304) return;
    } catch { /* non ancora pronto */ }
    await new Promise(r => setTimeout(r, 300));
  }
  throw new Error(`Vite non disponibile su :${port} dopo ${timeout}ms`);
}

/** Controlla se qualcosa è già in ascolto su una porta. */
async function isPortOpen(port) {
  try {
    await fetch(`http://localhost:${port}/`);
    return true;
  } catch { return false; }
}

export const config = {
  runner: 'local',
  port: 4444,
  specs: ['./specs/**/*.spec.mjs'],
  maxInstances: 1,
  capabilities: [{
    'tauri:options': {
      application: APP_BIN,
    },
  }],
  logLevel: 'warn',
  waitforTimeout: 10000,
  connectionRetryTimeout: 60000,
  connectionRetryCount: 1,
  framework: 'mocha',
  reporters: ['spec'],
  mochaOpts: {
    ui: 'bdd',
    timeout: 30000,
  },

  async onPrepare() {
    global.FIXTURE_PATH = FIXTURE_PATH;

    // Avvia Vite solo se non è già in ascolto (es. npm run dev separato)
    if (await isPortOpen(VITE_PORT)) {
      console.log(`[e2e] Vite già in ascolto su :${VITE_PORT}`);
      return;
    }
    console.log(`[e2e] Avvio Vite su :${VITE_PORT}...`);
    viteProcess = spawn('npm', ['run', 'dev'], {
      cwd: ROOT,
      stdio: ['ignore', 'pipe', 'pipe'],
      shell: true,
    });
    viteProcess.stderr.on('data', d => {
      const msg = d.toString();
      if (!msg.includes('VITE') && !msg.includes('vite')) process.stderr.write(msg);
    });
    await waitForVite(VITE_PORT);
    console.log(`[e2e] Vite pronto.`);
  },

  onComplete() {
    if (viteProcess) {
      console.log('[e2e] Chiudo Vite...');
      viteProcess.kill('SIGTERM');
      viteProcess = null;
    }
  },

  before() {
    global.FIXTURE_PATH = FIXTURE_PATH;
  },
};
