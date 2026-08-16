#!/usr/bin/env node
/**
 * Copy the napi cdylib into this package as aidb.node / aidb.<os>-<arch>.node.
 * Used by npm pack, prepare, and CI. Users run `npm i aidb` — they do not copy a dylib.
 */
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const pkg = path.resolve(here, "..");
const repo = path.resolve(pkg, "../..");

function platformTag() {
  const os =
    process.platform === "darwin"
      ? "darwin"
      : process.platform === "linux"
        ? "linux"
        : process.platform === "win32"
          ? "win32"
          : null;
  const arch =
    process.arch === "arm64" ? "arm64" : process.arch === "x64" ? "x64" : null;
  if (!os || !arch) {
    throw new Error(
      `aidb has no prebuilt napi addon for ${process.platform}-${process.arch}. Install from the repo with a Rust toolchain.`
    );
  }
  return `${os}-${arch}`;
}

function cargoNames() {
  if (process.platform === "darwin") {
    return ["libaidb_node.dylib", "aidb.node"];
  }
  if (process.platform === "win32") {
    return ["aidb_node.dll", "aidb.node"];
  }
  return ["libaidb_node.so", "aidb.node"];
}

function targetRoots() {
  const roots = [];
  if (process.env.CARGO_TARGET_DIR) {
    roots.push(process.env.CARGO_TARGET_DIR);
  }
  roots.push(path.join(repo, "target"));
  return roots;
}

function findArtifact(profile) {
  const names = cargoNames();
  for (const root of targetRoots()) {
    for (const name of names) {
      const candidate = path.join(root, profile, name);
      if (fs.existsSync(candidate)) {
        return candidate;
      }
    }
  }
  return null;
}

function resolveProfile(wantRelease) {
  if (wantRelease) {
    return "release";
  }
  const env = process.env.AIDB_NATIVE_PROFILE;
  if (env === "debug" || env === "release") {
    return env;
  }
  if (findArtifact("release")) {
    return "release";
  }
  if (findArtifact("debug")) {
    return "debug";
  }
  return "release";
}

function cargoBuild(profile) {
  const args = ["build", "-p", "aidb-node"];
  if (profile === "release") {
    args.push("--release");
  }
  const result = spawnSync("cargo", args, {
    cwd: repo,
    stdio: "inherit",
  });
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

const wantRelease = process.argv.includes("--release");
const profile = resolveProfile(wantRelease);
let source = findArtifact(profile);
if (!source) {
  cargoBuild(profile);
  source = findArtifact(profile);
}
if (!source) {
  throw new Error("aidb-node build did not produce a cdylib");
}

const tag = platformTag();
const tagged = path.join(pkg, `aidb.${tag}.node`);
fs.copyFileSync(source, tagged);
console.log(`staged ${path.relative(repo, source)} -> ${path.relative(repo, tagged)}`);
