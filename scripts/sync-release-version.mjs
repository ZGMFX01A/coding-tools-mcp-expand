import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const VERSION_PATTERN = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

export function normalizeReleaseVersion(input) {
  const value = String(input ?? "").trim().replace(/^v/, "");
  if (!VERSION_PATTERN.test(value)) {
    throw new Error(`Invalid release version: ${input ?? ""}. Expected vX.Y.Z or X.Y.Z.`);
  }
  return value;
}

function writeJsonVersion(rootDir, relativePath, version) {
  const filePath = path.join(rootDir, relativePath);
  const document = JSON.parse(fs.readFileSync(filePath, "utf8"));
  document.version = version;

  if (relativePath.endsWith("package-lock.json") && document.packages?.[""]) {
    document.packages[""].version = version;
  }

  fs.writeFileSync(filePath, `${JSON.stringify(document, null, 2)}\n`);
}

function writeCargoVersion(rootDir, version) {
  const filePath = path.join(rootDir, "src-tauri", "Cargo.toml");
  const contents = fs.readFileSync(filePath, "utf8");
  const packageVersion = /(\[package\][\s\S]*?\r?\nversion\s*=\s*)"[^"]+"/;

  if (!packageVersion.test(contents)) {
    throw new Error(`Could not find the [package] version in ${filePath}`);
  }

  fs.writeFileSync(filePath, contents.replace(packageVersion, `$1"${version}"`));
}

export function syncReleaseVersion(rootDir, input) {
  const version = normalizeReleaseVersion(input);
  const jsonFiles = [
    "package.json",
    "package-lock.json",
    "src-tauri/tauri.conf.json",
    "browser-extension/chatgpt-turn-observer/package.json",
    "browser-extension/chatgpt-turn-observer/package-lock.json",
    "browser-extension/chatgpt-turn-observer/manifest.json",
  ];

  for (const relativePath of jsonFiles) {
    writeJsonVersion(rootDir, relativePath, version);
  }
  writeCargoVersion(rootDir, version);

  return version;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const scriptDir = path.dirname(fileURLToPath(import.meta.url));
  const rootDir = path.resolve(process.argv[3] ?? path.join(scriptDir, ".."));
  const version = syncReleaseVersion(rootDir, process.argv[2]);
  console.log(`Synchronized release version ${version}.`);
}
