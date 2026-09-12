// postinstall: скачать платформенный бинарник Гефеста с GitHub Releases.
//
// Схема как у opencode: npm-пакет — тонкая обёртка; бинарники лежат в
// Release v<version> репозитория. Windows-тар распаковывается встроенным
// bsdtar (Win10+), Linux/macOS — системным tar.
//
// Приоритет:
//  1) платформенный optionalDependency (hephaestus-windows-x64 и т.п.) —
//     если кто-то раздаёт их через npm;
//  2) GitHub Releases: https://github.com/<repo>/releases/download/v<X>/<triple>.tar.gz
//  3) HEPHAESTUS_DOWNLOAD_URL — свой mirror (env).

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { get as httpsGet } from "node:https";

const pkgRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)));
const binDir = path.join(pkgRoot, "bin");
const rawPkg = fs.readFileSync(path.join(pkgRoot, "package.json"), "utf8");
const { version } = JSON.parse(rawPkg);

const GITHUB_REPO = "marselshkl2006-arch/Hephaestus-agent";
const platformTargets = {
  "linux-x64": "x86_64-unknown-linux-gnu",
  "win32-x64": "x86_64-pc-windows-gnu",
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
};

const key = `${process.platform}-${process.arch}`;
const triple = platformTargets[key];
if (!triple) {
  console.error(`[hephaestus] платформа ${key} не поддерживается.`);
  process.exit(1);
}
const exeSuffix = process.platform === "win32" ? ".exe" : "";
const binPath = path.join(binDir, `hephaestus${exeSuffix}`);

// Уже скачан (переустановка пакета) — не качаем повторно.
if (fs.existsSync(binPath)) {
  console.log(`[hephaestus] бинарник уже на месте: ${binPath}`);
  process.exit(0);
}

// 1. Платформенный npm-пакет?
try {
  const req = createRequire(path.join(pkgRoot, "package.json"));
  const platformPkgDir = path.dirname(req.resolve(`hephaestus-${key}/package.json`));
  const src = path.join(platformPkgDir, `hephaestus${exeSuffix}`);
  if (fs.existsSync(src)) {
    fs.mkdirSync(binDir, { recursive: true });
    fs.copyFileSync(src, binPath);
    if (process.platform !== "win32") fs.chmodSync(binPath, 0o755);
    console.log(`[hephaestus] из пакета hephaestus-${key} → ${binPath}`);
    process.exit(0);
  }
} catch {
  // не установлен — идём в Releases
}

// 2/3. Откуда качать: env-mirror или GitHub Releases.
const baseUrl =
  process.env.HEPHAESTUS_DOWNLOAD_URL
    ? `${process.env.HEPHAESTUS_DOWNLOAD_URL.replace(/\/$/, "")}/v${version}`
    : `https://github.com/${GITHUB_REPO}/releases/download/v${version}`;
const url = `${baseUrl}/${triple}.tar.gz`;
console.log(`[hephaestus] скачиваю ${url} …`);

fs.mkdirSync(binDir, { recursive: true });
const tmpTar = path.join(binDir, "hephaestus.tar.gz");

function download(u, redirects = 0) {
  return new Promise((resolve, reject) => {
    if (redirects > 5) return reject(new Error("слишком много редиректов"));
    const req = httpsGet(u, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        return resolve(download(res.headers.location, redirects + 1));
      }
      if (res.statusCode !== 200) {
        res.resume();
        return reject(new Error(`HTTP ${res.statusCode} для ${u}`));
      }
      const file = fs.createWriteStream(tmpTar);
      res.pipe(file);
      file.on("finish", () => file.close(resolve));
    });
    req.on("error", reject);
  });
}

try {
  await download(url);
} catch (err) {
  console.error(`[hephaestus] скачать не удалось: ${err.message}`);
  console.error(`[hephaestus] положите бинарник вручную в ${binPath}`);
  console.error(`[hephaestus] или соберите из исходников: cargo build --release`);
  process.exit(1);
}

const tar = spawnSync("tar", ["-xzf", tmpTar, "-C", binDir], { stdio: "inherit" });
fs.unlinkSync(tmpTar);
if (tar.status !== 0) {
  console.error("[hephaestus] распаковка не удалась (нужен tar; на Windows встроен в Win10+)");
  process.exit(1);
}
if (process.platform !== "win32") fs.chmodSync(binPath, 0o755);

console.log(`[hephaestus] установлен: ${binPath}`);
console.log("[hephaestus] запуск: hephaestus — первый старт создаст ~/.hephaestus с примерами конфигов.");
