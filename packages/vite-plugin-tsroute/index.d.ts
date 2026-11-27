import type { Plugin } from "vite";

export interface TsrouteOptions {
  /** Directory of the Rust crate (where `cargo` runs). Default: `".."`, relative to the Vite root. */
  crate?: string;
  /** Paths to watch, relative to `crate`. Default: `["src"]`. */
  watch?: string[];
  /** Command that regenerates the TypeScript. Default: `"cargo test --quiet tsroute_export"`. */
  command?: string;
  /** Milliseconds to wait after the last change before regenerating. Default: `150`. */
  debounce?: number;
  /** Also regenerate once when the dev server starts. Default: `true`. */
  runOnStart?: boolean;
  /** During `vite build`, regenerate first and fail the build on error. Default: `true`. */
  runOnBuild?: boolean;
}

export default function tsroute(options?: TsrouteOptions): Plugin;
