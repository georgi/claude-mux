// A throwaway cmuxd for one test file: a temp data dir, a git repo, and the
// fake claude as `claude` with the given scripts.

import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { request } from "node:http";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const target = fileURLToPath(new URL("../../target/debug", import.meta.url));

export interface Cmuxd {
  url: string;
  root: string;
  repo: string;
  /** A fresh pairing code from the Unix socket. */
  pairCode(): Promise<string>;
  /** The server's output so far, for failure messages. */
  log(): string;
  stop(): Promise<void>;
}

async function freePort(): Promise<number> {
  return new Promise((ok, fail) => {
    const s = createServer();
    s.once("error", fail);
    s.listen(0, "127.0.0.1", () => {
      const a = s.address();
      s.close(() => (typeof a === "object" && a ? ok(a.port) : fail(new Error("no port"))));
    });
  });
}

function unixPost(socket: string, path: string, body: unknown): Promise<{ status: number; body: string }> {
  return new Promise((ok, fail) => {
    const data = JSON.stringify(body);
    const r = request(
      { socketPath: socket, path, method: "POST", headers: { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(data) } },
      (res) => {
        let text = "";
        res.on("data", (c) => (text += c));
        res.on("end", () => ok({ status: res.statusCode ?? 0, body: text }));
      },
    );
    r.on("error", fail);
    r.end(data);
  });
}

function git(cwd: string, ...args: string[]) {
  execFileSync("git", ["-c", "init.defaultBranch=main", "-c", "user.email=e2e@example.com", "-c", "user.name=e2e", ...args], {
    cwd,
    stdio: "ignore",
  });
}

export async function startCmuxd(scripts: Record<string, unknown>): Promise<Cmuxd> {
  const cmuxd = process.env.CMUXD_BIN ?? join(target, "cmuxd");
  const fake = process.env.FAKE_CLAUDE_BIN ?? join(target, "examples/fake-claude");
  for (const bin of [cmuxd, fake]) {
    if (!existsSync(bin)) throw new Error(`${bin} is missing: run cargo build -p cmuxd --bin cmuxd --example fake-claude`);
  }
  // Short: the Unix socket path must stay under ~100 bytes.
  const root = realpathSync(mkdtempSync(join(tmpdir(), "cmux-e2e-")));
  const repo = join(root, "repo");
  mkdirSync(repo);
  git(repo, "init", "-q");
  writeFileSync(join(repo, "README.md"), "e2e\n");
  git(repo, "add", ".");
  git(repo, "commit", "-qm", "init");

  const dir = join(root, "scripts");
  mkdirSync(dir);
  for (const [name, script] of Object.entries(scripts)) {
    writeFileSync(join(dir, `${name.replace(/\//g, "_")}.json`), JSON.stringify(script));
  }
  const claude = join(root, "claude");
  writeFileSync(claude, `#!/bin/sh\nFAKE_CLAUDE_SCRIPTS='${dir}' exec '${fake}' "$@"\n`);
  chmodSync(claude, 0o755);

  const port = await freePort();
  const socket = join(root, "api.sock");
  writeFileSync(
    join(root, "config.toml"),
    [
      `data_dir = "${join(root, "data")}"`,
      `socket = "${socket}"`,
      `listen = "127.0.0.1:${port}"`,
      `name = "e2e"`,
      `[agents]`,
      `claude_bin = "${claude}"`,
      `pre_trust = false`,
      `[github]`,
      `enabled = false`,
      "",
    ].join("\n"),
  );

  let out = "";
  const child: ChildProcess = spawn(cmuxd, ["--config", join(root, "config.toml"), "serve"], {
    env: { ...process.env, CLAUDE_CONFIG_DIR: join(root, "claude-config"), RUST_LOG: process.env.RUST_LOG ?? "warn" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stdout?.on("data", (d) => (out += d));
  child.stderr?.on("data", (d) => (out += d));

  const url = `http://127.0.0.1:${port}`;
  const deadline = Date.now() + 15_000;
  for (;;) {
    if (child.exitCode !== null) throw new Error(`cmuxd exited ${child.exitCode}:\n${out}`);
    try {
      const r = await fetch(`${url}/`);
      if (r.ok) break;
    } catch {
      // Not listening yet.
    }
    if (Date.now() > deadline) throw new Error(`cmuxd did not start:\n${out}`);
    await new Promise((r) => setTimeout(r, 100));
  }

  return {
    url,
    root,
    repo,
    async pairCode() {
      const r = await unixPost(socket, "/v1/pair/start", {});
      if (r.status !== 200) throw new Error(`pair/start ${r.status}: ${r.body}`);
      return (JSON.parse(r.body) as { code: string }).code;
    },
    log: () => out,
    async stop() {
      if (child.exitCode === null) {
        child.kill("SIGTERM");
        await new Promise((r) => child.once("exit", r));
      }
      rmSync(root, { recursive: true, force: true });
    },
  };
}
