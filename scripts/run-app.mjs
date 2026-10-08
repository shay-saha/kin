import { spawn } from "node:child_process";
import nextEnv from "@next/env";

nextEnv.loadEnvConfig(process.cwd());

const production = process.argv.includes("--production");
const children = [];
let stopping = false;
function stop(code = 0) {
  if (stopping) return;
  stopping = true;
  process.exitCode = code;
  for (const child of children) child.kill("SIGTERM");
}
function start(command, arguments_) {
  const child = spawn(command, arguments_, { stdio: "inherit", env: process.env });
  children.push(child);
  child.on("error", error => { console.error(error.message); stop(1); });
  child.on("exit", code => stop(code ?? 1));
}
process.on("SIGINT", () => stop(130));
process.on("SIGTERM", () => stop(143));
if (!process.env.KIN_BACKEND_URL) {
  if (production) start("brain/target/release/kin-brain", []);
  else start("cargo", ["run", "--locked", "--manifest-path", "brain/Cargo.toml"]);
}
start(process.execPath, ["node_modules/next/dist/bin/next", production ? "start" : "dev", ...process.argv.slice(2).filter(argument => argument !== "--production")]);
