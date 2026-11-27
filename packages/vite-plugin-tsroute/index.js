// vite-plugin-tsroute: keep the generated TypeScript in step with your Rust API.
import { spawn } from "node:child_process";
import path from "node:path";

const NAME = "vite-plugin-tsroute";

/** @param {import("./index.d.ts").TsrouteOptions} [options] */
export default function tsroute(options = {}) {
  const {
    crate = "..",
    watch = ["src"],
    command = "cargo test --quiet tsroute_export",
    debounce = 150,
    runOnStart = true,
    runOnBuild = true,
  } = options;

  /** @type {import("vite").ResolvedConfig} */
  let config;
  /** @type {import("vite").ViteDevServer | undefined} */
  let server;
  let crateDir = "";
  let watchDirs = [];
  let timer;
  let running = false;
  let queued = false;

  function run() {
    return new Promise((resolve) => {
      const started = Date.now();
      // Never run in check-only mode: we want the files written.
      const env = { ...process.env, CI: "", TSROUTE_CHECK: "" };
      const child = spawn(command, { cwd: crateDir, shell: true, env });
      let output = "";
      child.stdout.on("data", (d) => (output += d));
      child.stderr.on("data", (d) => (output += d));
      child.on("error", (err) => resolve({ ok: false, output: String(err), ms: Date.now() - started }));
      child.on("close", (code) => resolve({ ok: code === 0, output, ms: Date.now() - started }));
    });
  }

  function trimOutput(output) {
    // Keep the useful part: compiler errors and panics, not the progress noise.
    const lines = output.split("\n").filter((l) => !/^\s*(Compiling|Blocking|Finished|Running)\b/.test(l));
    return lines.join("\n").trim().slice(-6000);
  }

  async function regenerate(reason) {
    if (running) {
      queued = true;
      return;
    }
    running = true;
    const logger = config.logger;
    logger.info(`\x1b[36m[tsroute]\x1b[0m regenerating (${reason})…`, { timestamp: true });
    const result = await run();
    if (result.ok) {
      logger.info(`\x1b[36m[tsroute]\x1b[0m TypeScript updated in ${(result.ms / 1000).toFixed(1)}s`, {
        timestamp: true,
      });
    } else {
      const message = trimOutput(result.output) || `\`${command}\` failed`;
      logger.error(`\x1b[31m[tsroute]\x1b[0m regeneration failed:\n${message}`, { timestamp: true });
      server?.ws.send({
        type: "error",
        err: { message: `[tsroute] Rust build failed\n\n${message}`, stack: "", plugin: NAME },
      });
    }
    running = false;
    if (queued) {
      queued = false;
      regenerate("queued change");
    }
    return result;
  }

  const isWatched = (file) =>
    file.endsWith(".rs") || file.endsWith("Cargo.toml")
      ? watchDirs.some((dir) => file === dir || file.startsWith(dir + path.sep))
      : false;

  return {
    name: NAME,

    configResolved(resolved) {
      config = resolved;
      crateDir = path.resolve(config.root, crate);
      watchDirs = [...watch.map((w) => path.resolve(crateDir, w)), path.join(crateDir, "Cargo.toml")];
    },

    async buildStart() {
      if (config.command !== "build" || !runOnBuild) return;
      const result = await regenerate("build");
      if (!result.ok) this.error(`[tsroute] \`${command}\` failed:\n${trimOutput(result.output)}`);
    },

    configureServer(devServer) {
      server = devServer;
      server.watcher.add(watchDirs);
      const onFile = (file) => {
        if (!isWatched(path.resolve(file))) return;
        clearTimeout(timer);
        timer = setTimeout(() => regenerate(path.relative(crateDir, file)), debounce);
      };
      server.watcher.on("change", onFile);
      server.watcher.on("add", onFile);
      server.watcher.on("unlink", onFile);
      if (runOnStart) regenerate("startup");
    },
  };
}
