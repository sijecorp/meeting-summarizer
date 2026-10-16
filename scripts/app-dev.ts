// `bun run app:dev`: builds the llama-server sidecar when it is missing, build.rs's empty placeholder,
// or stamped for another llama.cpp commit or other CMake options, then runs `tauri dev` (M6.5 plan 5.1,
// DX4, ENG14). The stamp check matches src-tauri/build.rs. `--check` only reports (exit 1 when the
// sidecar needs a build). Other arguments go to `tauri dev`.
import { createHash } from "node:crypto";
import { existsSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const root = join(import.meta.dir, "..");
const win = process.platform === "win32";
const triple = win
  ? "x86_64-pc-windows-msvc"
  : process.platform === "darwin" && process.arch === "arm64"
    ? "aarch64-apple-darwin"
    : null;
if (!triple) {
  console.error(`No llama-server build for ${process.platform}-${process.arch}: macOS arm64 and Windows x64 only.`);
  process.exit(1);
}
const bin = join(root, "src-tauri", "binaries", `llama-server-${triple}${win ? ".exe" : ""}`);
const stamp = join(root, "src-tauri", "binaries", `llama-server-${triple}.stamp`);
const rev = readFileSync(join(root, "scripts", "llama.cpp.rev"), "utf8")
  .split("\n")
  .map((l) => l.trim())
  .find((l) => l && !l.startsWith("#"));
const flags = createHash("sha256")
  .update(readFileSync(join(root, "scripts", "llama-server.flags")))
  .digest("hex")
  .slice(0, 12);
const expected = `rev=${rev}\nflags=${flags}`;

function problem(): string | null {
  if (!existsSync(bin)) return "is missing";
  if (statSync(bin).size === 0) return "is an empty placeholder";
  if (!existsSync(stamp)) return "has no stamp";
  const lines = readFileSync(stamp, "utf8").split("\n").map((l) => l.trim()).filter(Boolean);
  if (lines.join("\n") !== expected) return "was built from another llama.cpp commit or other CMake options";
  return null;
}

const args = process.argv.slice(2);
const found = problem();
if (args.includes("--check")) {
  console.log(found ? `${bin} ${found}.` : `${bin} is ready (llama.cpp ${rev}, flags ${flags}).`);
  process.exit(found ? 1 : 0);
}

if (found) {
  console.log(`${bin} ${found}. Building it.`);
  const script = join(root, "scripts", win ? "build-llama-server.ps1" : "build-llama-server.sh");
  const cmd = win
    ? [Bun.which("pwsh") ? "pwsh" : "powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script]
    : ["bash", script];
  const build = Bun.spawnSync(cmd, { stdio: ["inherit", "inherit", "inherit"] });
  if (build.exitCode !== 0) process.exit(build.exitCode ?? 1);
}

// Ctrl+C reaches tauri dev too; wait for it to shut down instead of exiting first.
process.on("SIGINT", () => {});
const dev = Bun.spawn([process.execPath, "x", "tauri", "dev", ...args], {
  cwd: root,
  stdio: ["inherit", "inherit", "inherit"],
});
process.exit(await dev.exited);
