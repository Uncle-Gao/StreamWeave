import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const version = process.argv[2];

if (!version) {
  console.error("Usage: node scripts/sync-release-version.mjs <version>");
  process.exit(2);
}

if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`Invalid release version: ${version}`);
  process.exit(2);
}

function readJson(path) {
  return JSON.parse(readFileSync(join(rootDir, path), "utf8"));
}

function writeJson(path, value) {
  writeFileSync(join(rootDir, path), `${JSON.stringify(value, null, 2)}\n`);
}

const rootDir = process.env.STREAMWEAVE_VERSION_ROOT || ".";

const packageJson = readJson("package.json");
packageJson.version = version;
writeJson("package.json", packageJson);

const packageLock = readJson("package-lock.json");
packageLock.version = version;
if (packageLock.packages?.[""]) {
  packageLock.packages[""].version = version;
}
writeJson("package-lock.json", packageLock);

const tauriConfig = readJson("src-tauri/tauri.conf.json");
tauriConfig.version = version;
writeJson("src-tauri/tauri.conf.json", tauriConfig);

console.log(`Synced release version: ${version}`);
