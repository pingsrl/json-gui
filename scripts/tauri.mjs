import { spawnSync } from "node:child_process";

const args = process.argv.slice(2);
const shouldSkipUpdaterArtifacts =
  args[0] === "build" && !process.env.TAURI_SIGNING_PRIVATE_KEY;

const tauriArgs = ["tauri", ...args];

if (shouldSkipUpdaterArtifacts) {
  tauriArgs.push(
    "--config",
    JSON.stringify({
      bundle: {
        createUpdaterArtifacts: false
      }
    })
  );
  console.warn(
    "TAURI_SIGNING_PRIVATE_KEY non impostata: updater artifacts disattivati per questo build locale."
  );
}

const result = spawnSync("npx", tauriArgs, {
  stdio: "inherit",
  env: process.env
});

if (result.error) {
  throw result.error;
}

process.exit(result.status ?? 1);
