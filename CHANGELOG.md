# Changelog

## 0.2.0

- **Zod schemas**: `Exporter::zod(path)` generates a Zod v4 schema for every type, plus `routeSchemas` (body, query, response and error per route) and `withValidation(client)` for runtime response validation. It supports tagged, untagged and unit enums, maps, tuples, generics and recursive types. A compile-time check proves every schema matches its TypeScript type.
- **@atomic-ui/vite-plugin-tsroute**: a new npm package that regenerates the TypeScript when Rust files change during `vite dev`, shows Rust compile errors in the Vite overlay, and regenerates before `vite build`.
- The example is now a runnable React + Vite app.

## 0.1.0

- First release: `#[tsroute::get/post/put/patch/delete]`, `tsroute::routes!`, a typed fetch client, TanStack Query hooks, and `Exporter::sync()` with a CI drift check.
