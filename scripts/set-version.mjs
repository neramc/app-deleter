#!/usr/bin/env node
// 빌드 시 결정된 버전을 저장소 내 모든 매니페스트에 반영한다.
// 대상: package.json(.version), src-tauri/tauri.conf.json(.version),
//       Cargo.toml([package]의 version).
// 사용법: node scripts/set-version.mjs 1.2.3   (앞의 'v'는 자동 제거)

import { promises as fs } from "node:fs";
import path from "node:path";
import process from "node:process";

const raw = process.argv[2];
if (!raw) {
  console.error("사용법: node scripts/set-version.mjs <version>");
  process.exit(1);
}
const version = raw.replace(/^v/, "").trim();
if (!/^\d+\.\d+\.\d+$/.test(version)) {
  console.error(`잘못된 버전 형식: ${version} (x.y.z 형태여야 함)`);
  process.exit(1);
}

const IGNORE = new Set(["node_modules", "target", "gen", "dist", "dist-ssr", ".git"]);
const root = process.cwd();
const changed = [];

async function walk(dir) {
  const entries = await fs.readdir(dir, { withFileTypes: true });
  for (const e of entries) {
    if (e.isDirectory()) {
      if (IGNORE.has(e.name)) continue;
      await walk(path.join(dir, e.name));
    } else {
      await handleFile(path.join(dir, e.name), e.name);
    }
  }
}

async function handleFile(file, name) {
  if (name === "package.json" || name === "tauri.conf.json") {
    const text = await fs.readFile(file, "utf8");
    const json = JSON.parse(text);
    if (json.version === undefined) return;
    if (json.version === version) return;
    json.version = version;
    await fs.writeFile(file, JSON.stringify(json, null, 2) + "\n");
    changed.push(path.relative(root, file));
  } else if (name === "Cargo.toml") {
    const text = await fs.readFile(file, "utf8");
    // [package] 섹션의 첫 version = "..." 한 줄만 교체한다.
    const updated = text.replace(
      /(\[package\][\s\S]*?\n\s*version\s*=\s*")([^"]*)(")/,
      `$1${version}$3`,
    );
    if (updated !== text) {
      await fs.writeFile(file, updated);
      changed.push(path.relative(root, file));
    }
  }
}

await walk(root);
console.log(`버전 ${version} 적용 완료. 변경된 파일:`);
for (const f of changed) console.log(`  - ${f}`);
