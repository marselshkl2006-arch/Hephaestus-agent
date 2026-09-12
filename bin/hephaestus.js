#!/usr/bin/env node
// Лаунчер Гефеста: скачивает платформенный бинарник при ПЕРВОМ запуске
// (lazy-download вместо postinstall — установка без скриптов, работает
// даже с --ignore-scripts и в корпоративных политиках).
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { get as httpsGet } from "node:https";
import { spawnSync } from "node:child_process";

const pkgRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const exeSuffix = process.platform === "win32" ? ".exe" : "";
const binPath = path.join(pkgRoot, "bin", `hephaestus${exeSuffix}`);

const GITHUB_REPO = "marselshkl2006-arch/Hephaestus-agent";
const VERSION = JSON.parse(fs.readFileSync(path.join(pkgRoot, "package.json"), "utf8")).version;
const platformTargets = {
  "linux-x64": "x86_64-unknown-linux-gnu",
  "win32-x64": "x86_64-pc-windows-gnu",
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
};
const triple = platformTargets[`${process.platform}-${process.arch}`];

async function ensureBinary() {
  if (fs.existsSync(binPath)) return true;
  if (!triple) {
    console.error(`[hephaestus] платформа ${process.platform}-${process.arch} не поддерживается.`);
    return false;
  }
  const baseUrl = process.env.HEPHAESTUS_DOWNLOAD_URL
    ? `${process.env.HEPHAESTUS_DOWNLOAD_URL.replace(/\/$/, "")}/v${VERSION}`
    : `https://github.com/${GITHUB_REPO}/releases/download/v${VERSION}`;
  const url = `${baseUrl}/${triple}.tar.gz`;
  console.log(`[hephaestus] первый запуск — скачиваю ${url} …`);
  const tmpTar = path.join(pkgRoot, "bin", "hephaestus.tar.gz");
  const ok = await new Promise((resolve) => {
    const dl = (u, redirects = 0) => {
      if (redirects > 5) return resolve(false);
      const req = httpsGet(u, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) return dl(res.headers.location, redirects + 1);
        if (res.statusCode !== 200) { res.resume(); return resolve(false); }
        const f = fs.createWriteStream(tmpTar);
        res.pipe(f);
        f.on("finish", () => f.close(() => resolve(true)));
      });
      req.on("error", () => resolve(false));
    };
    dl(url);
  });
  if (!ok) {
    console.error(`[hephaestus] скачать не удалось: ${url}`);
    console.error(`[hephaestus] скачайте вручную из релиза v${VERSION} и положите в ${binPath}`);
    return false;
  }
  const tar = spawnSync("tar", ["-xzf", tmpTar, "-C", path.dirname(binPath)], { stdio: "inherit" });
  try { fs.unlinkSync(tmpTar); } catch {}
  return tar.status === 0 && fs.existsSync(binPath);
}

const ok = await ensureBinary();
if (!ok) process.exit(1);

const child = spawn(binPath, process.argv.slice(2), { stdio: "inherit", env: process.env });
child.on("error", (err) => {
  console.error(`[hephaestus] запуск не удался (${binPath}): ${err.message}`);
  process.exit(127);
});
child.on("exit", (code) => process.exit(code ?? 0));
