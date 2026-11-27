import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import tsroute from "vite-plugin-tsroute";

// Regenerates src/api/*.ts whenever the Rust code in ../src changes.
export default defineConfig({
  plugins: [react(), tsroute({ crate: ".." })],
});
