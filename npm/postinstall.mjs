// postinstall: скачать платформенный бинарник Гефеста.
//
// КАК У OPCODECODE: основной пакет npm — тонкая обёртка; бинарники живут
// в платформенных пакетах (hephaestus-linux-x64 и т.п.) или скачиваются
// с release-сервера (Gitea/GitHub) напрямую.
//
// Порядок:
//  1) если установился платформенный optionalDependency (содержит bin/) —
//     использовать его;
//  2) иначе скачать с HEPHAESTUS_DOWNLOAD_URL/<version>/<triple>.tar.gz.
//
// Скачанный бинарник кладётся в <pkg>/bin/hephaestus(.exe) — его вызывает
// bin/hephaestus.js. Никаких ключей/токенов тут нет и не должно быть:
// пользователь настраивает ~/.hephaestus после установки.

import { createWriteStream, existsSync, mkdirSync, chmodSync } from "node:fs";
import { get } from "node:https";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";

const pkgRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)));
const binDir = path.join(pkgRoot, "bin");
const require = createRequire(import.meta.url);
const { version, name } = JSON.parse(
  (await import("node:fs")).readFileSync(path.join(pkgRoot, "package.json"), "utf8")
);

const platformTargets = {
  "linux-x64": "x86_64-unknown-linux-gnu",
  "win32-x64": "x86_64-pc-windows-gnu",
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
};

const key = `${process.platform}-${process.arch}`;
const triple = platformTargets[key];
if (!triple) {
  console.error(`[hephaestus] платформа ${key} не поддерживается этим пакетом.`);
  process.exit(1);
}

const exeSuffix = process.platform === "win32" ? ".exe" : "";
const binPath = path.join(binDir, `hephaestus${exeSuffix}`);

// 1. Платформенный npm-пакет?
const pkgName = `hephaestus-${key}`;
try {
  const req = createRequire(path.join(pkgRoot, "package.json"));
  const platformPkgPath = req.resolve(`${pkgName}/package.json`);
  const platformPkgDir = path.dirname(platformPkgPath);
  const src = path.join(platformPkgDir, `hephaestus${exeSuffix}`);
  if (existsSync(src)) {
    mkdirSync(binDir, { recursive: true });
    await import("node:fs").then((fs) => fs.copyFileSync(src, binPath));
    if (process.platform !== "win32") chmodSync(binPath, 0o755);
    console.log(`[hephaestus] бинарник из пакета ${pkgName} → ${binPath}`);
    process.exit(0);
  }
} catch {
  // пакет не установлен — качаем с release-сервера.
}

// 2. Скачивание с release-сервера.
const baseUrl = process.env.HEPHAESTUS_DOWNLOAD_URL; // например https://git.example.com/hephaestus/releases
if (!baseUrl) {
  console.error(
    `[hephaestus] не задан HEPHAESTUS_DOWNLOAD_URL и не установлен ${pkgName}.\n` +
      `Скачайте бинарник вручную и положите в ${binPath},\n` +
      `или соберите из исходников: cargo build --release`
  );
  process.exit(1);
}

const url = `${baseUrl.replace(/\/$/, "")}/v${version}/${triple}.tar.gz`;
console.log(`[hephaestus] скачиваю ${url} …`);

mkdirSync(binDir, { recursive: true });

function download(u, redirects = 0) {
  return new Promise((resolve, reject) => {
    if (redirects > 5) return reject(new Error("слишком много редиректов"));
    get(u, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        return resolve(download(new URL(res.headers.location, u).href, redirects + 1));
      }
      if (res.statusCode !== 200) {
        return reject(new Error(`HTTP ${res.statusCode} для ${u}`));
      }
      res.pipe(resolve);
      resolve(tarExtractTarget());
      function tarExtractTarget() {}
    }).on("error", reject);
  });
}

// Проще и надёжнее: качаем в tmp и распаковываем tar (есть во всех системах,
// на Windows — bsdtar, встроен в Win10+).
const tmpTar = path.join(binDir, "hephaestus.tar.gz");
await new Promise((resolve, reject) => {
  const file = createWriteStream(tmpTar);
  const request = (u, redirects) => {
    get(u, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        return request(new URL(res.headers.location, u).href, redirects + 1);
      }
      if (res.statusCode !== 200) {
        return reject(new Error(`HTTP ${res.statusCode} для ${u}`));
      }
      res.pipe(file);
      file.on("finish", () => file.close(resolve));
    }).on("error", reject);
  };
  request(url, 0);
});

const tar = spawnSync("tar", ["-xzf", tmpTar, "-C", binDir], { stdio: "inherit" });
if (tar.status !== 0) {
  console.error("[hephaestus] распаковка не удалась — установлен tar? (на Windows bsdtar встроен в Win10+)");
  process.exit(1);
}
import("node:fs").then((fs) => fs.unlinkSync(tmpTar));
if (process.platform !== "win32") chmodSync(binPath, 0o755);

console.log(`[hephaestus] готово: ${binPath}`);
console.log("[hephaestus] первый запуск создаст ~/.hephaestus с примерами конфигов.");
console.log("[hephaestus] Telegram-токен (опционально): /telegram save <токен> после запуска.");
