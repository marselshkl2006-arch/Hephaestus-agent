#!/usr/bin/env node
// Тонкий лаунчер: передаёт все аргументы нативному бинарнику Гефеста.
import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const pkgRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const exe = process.platform === "win32" ? "hephaestus.exe" : "hephaestus";
const binPath = path.join(pkgRoot, "bin", exe);

const child = spawn(binPath, process.argv.slice(2), {
  stdio: "inherit",
  env: process.env,
});
child.on("error", (err) => {
  console.error(`[hephaestus] бинарник не найден (${binPath}): ${err.message}`);
  console.error("[hephaestus] переустановите пакет или положите бинарник вручную.");
  process.exit(127);
});
child.on("exit", (code) => process.exit(code ?? 0));
