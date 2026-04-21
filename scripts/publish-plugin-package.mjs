#!/usr/bin/env node

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..");

function fail(message) {
  console.error(message);
  process.exit(1);
}

function copyEntry(source, destination) {
  fs.cpSync(source, destination, { recursive: true });
}

const args = process.argv.slice(2);
if (args.length === 0) {
  fail("Usage: node scripts/publish-plugin-package.mjs <package-dir> [--tag-prefix prefix] [--dry-run|--publish]");
}

const packageDirArg = args[0];
let tagPrefix = "";
let shouldDryRun = false;
let shouldPublish = false;

for (let i = 1; i < args.length; i += 1) {
  const arg = args[i];
  if (arg === "--tag-prefix") {
    tagPrefix = args[i + 1] ?? "";
    i += 1;
  } else if (arg === "--dry-run") {
    shouldDryRun = true;
  } else if (arg === "--publish") {
    shouldPublish = true;
  } else {
    fail(`Unknown argument: ${arg}`);
  }
}

if (shouldDryRun && shouldPublish) {
  fail("Use either --dry-run or --publish, not both.");
}

const packageDir = path.resolve(repoRoot, packageDirArg);
const packageJsonPath = path.join(packageDir, "package.json");
if (!fs.existsSync(packageJsonPath)) {
  fail(`package.json not found in ${packageDir}`);
}

const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
const sdkPackageJson = JSON.parse(
  fs.readFileSync(path.join(repoRoot, "packages", "plugin-sdk", "package.json"), "utf8"),
);

if (tagPrefix) {
  const refName = process.env.GITHUB_REF_NAME ?? "";
  const expectedTag = `${tagPrefix}-v${packageJson.version}`;
  if (refName && refName !== expectedTag) {
    fail(`Ref ${refName} does not match ${expectedTag}`);
  }
}

const schemaPath = path.join(packageDir, "dist", "config-schema.json");
if (!fs.existsSync(schemaPath)) {
  fail(`Missing dist/config-schema.json for ${packageJson.name}. Run the build first.`);
}

const filesToStage = new Set(["package.json"]);
for (const entry of packageJson.files ?? []) {
  filesToStage.add(entry);
}
for (const extra of ["README.md", "LICENSE"]) {
  if (fs.existsSync(path.join(packageDir, extra))) {
    filesToStage.add(extra);
  }
}

const tempDir = fs.mkdtempSync(
  path.join(os.tmpdir(), `${packageJson.name.replaceAll("/", "-").replaceAll("@", "")}-`),
);

for (const entry of filesToStage) {
  if (entry === "package.json") continue;
  const source = path.join(packageDir, entry);
  if (!fs.existsSync(source)) {
    fail(`Configured publish file does not exist: ${source}`);
  }
  copyEntry(source, path.join(tempDir, entry));
}

const stagedPackageJson = structuredClone(packageJson);
delete stagedPackageJson.private;
stagedPackageJson.publishConfig = {
  access: "public",
  ...(stagedPackageJson.publishConfig ?? {}),
};

if (stagedPackageJson.dependencies?.["@hivemind-os/plugin-sdk"]) {
  stagedPackageJson.dependencies["@hivemind-os/plugin-sdk"] = `^${sdkPackageJson.version}`;
}

fs.writeFileSync(
  path.join(tempDir, "package.json"),
  `${JSON.stringify(stagedPackageJson, null, 2)}\n`,
);

const npmCommand = process.platform === "win32" ? "npm.cmd" : "npm";
if (shouldDryRun || shouldPublish) {
  const args = shouldPublish
    ? ["publish", "--provenance", "--access", "public"]
    : ["pack", "--dry-run"];
  const result = spawnSync(npmCommand, args, {
    cwd: tempDir,
    stdio: "inherit",
    shell: process.platform === "win32",
  });
  if (result.error) {
    fail(result.error.message);
  }
  process.exit(result.status ?? 1);
}

console.log(tempDir);
