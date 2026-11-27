# tsroute

**Typed TypeScript clients, TanStack Query hooks and Zod schemas, generated from the axum handlers you already have.**

No RPC framework to adopt, no OpenAPI spec to maintain, no second schema. Add one attribute to each handler and your React app gets a fully typed client: path params, query, body, response *and* error types. A test in CI makes sure the frontend can never drift from the API.

```rust
/// Get one todo by id.
#[tsroute::get("/todos/{id}", error = ErrorBody)]
async fn get_todo(State(s): State<AppState>, Path(id): Path<u64>) -> Result<Json<Todo>, ApiError> {
    // ...
}

let app = tsroute::routes![list_todos, get_todo, create_todo].with_state(state);
```

```ts
const api = createClient({ baseUrl: "https://api.example.com" });
const todo = await api.getTodo(1);            // Promise<Todo>

const { useListTodos, useCreateTodo } = createHooks(api);
const todos = useListTodos({ done: false });  // UseQueryResult<Page<Todo>, ApiError<unknown>>
const create = useCreateTodo();
create.mutate({ title: "Ship it", priority: "high" });
create.error?.body.message;                    // typed as ErrorBody
```

## Why

The Rust-to-TypeScript options today make you pick a trade-off:

| | Keeps your REST routes | No extra schema | Typed errors | React Query hooks | Runtime validation | CI drift check |
|---|---|---|---|---|---|---|
| OpenAPI (utoipa/aide) + codegen | yes | no: spec + generator step | manual | via 3rd-party generator | via 3rd-party generator | manual |
| rspc / qubit / taut-rpc | no: RPC only | yes | partly | yes | partly | no |
| ts-rs alone | yes | yes | no | no: write fetch calls by hand | no | no |
| **tsroute** | **yes** | **yes** | **yes** | **yes** | **yes (Zod)** | **built in** |

## Install

```toml
[dependencies]
tsroute = "0.1"
ts-rs = "12"   # derive TS on your request/response types
```

## Usage

**1. Derive `TS` on the types that cross the wire** (alongside serde, which ts-rs respects: `rename_all`, enums, `Option`, generics…):

```rust
#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Todo { pub id: u64, pub title: String, pub done: bool }
```

Tip: put `#[ts(optional_fields)]` on input structs so `Option` fields become `field?:` in TypeScript. If every field of a query or body type is optional, the argument itself becomes optional (`api.listTodos()`).

**2. Annotate handlers** with `get`, `post`, `put`, `patch` or `delete`:

```rust
#[tsroute::post("/todos", error = ErrorBody)]
async fn create_todo(Json(input): Json<CreateTodo>) -> Result<(StatusCode, Json<Todo>), ApiError> { ... }
```

| Option | Meaning |
|---|---|
| `error = Type` | The JSON body of error responses. Becomes `ApiError<Type>` in the client and hooks. |
| `name = "fooBar"` | Override the client function name (defaults to the handler name in camelCase). |

Handler doc comments become JSDoc on the generated functions and hooks.

**3. Build the router** with `tsroute::routes!`. It reads the path and method from each attribute, so you never write them twice. It returns a normal `axum::Router`, so state, layers and untyped routes work as usual.

**4. Generate the TypeScript from a test:**

```rust
#[test]
fn tsroute_export() {
    tsroute::Exporter::new()
        .client("web/src/api/client.ts")
        .react_query("web/src/api/hooks.ts") // optional
        .zod("web/src/api/schemas.ts")       // optional
        .sync()
        .unwrap();
}
```

`cargo test` writes the files locally. When `CI` (or `TSROUTE_CHECK`) is set, `sync()` only *checks*, and the test fails if someone changed a Rust type without regenerating:

```
generated TypeScript is out of date; re-run the export. Stale files:
  - web/src/api/client.ts
```

Use `.only_module("my_app::public")` to leave internal routes out of the client.

## Runtime validation with Zod

TypeScript types disappear at runtime. If a proxy, an old deploy or a bug sends the wrong shape, nothing stops it reaching your UI. Add `.zod("…/schemas.ts")` and tsroute also generates a [Zod](https://zod.dev) v4 schema for every type (tested with Zod 4.6):

```ts
import { createClient } from "./api/client";
import { CreateTodoSchema, routeSchemas, withValidation } from "./api/schemas";

// Every response is parsed; bad data throws a ZodError instead of rendering.
const api = withValidation(createClient({ baseUrl: "/api" }));

// Input schemas plug straight into forms, e.g. react-hook-form's zodResolver.
useForm({ resolver: zodResolver(CreateTodoSchema) });

routeSchemas.getTodo.error; // the ErrorBody schema, for parsing ApiError bodies
```

It handles everything serde and ts-rs produce: tagged, untagged and unit enums, `Option`, maps, tuples, generics (`PageSchema(TodoSchema)`) and recursive types (via `z.lazy`). The generated file ends with a compile-time check proving each schema's output type matches its TypeScript type, so a generator bug fails `tsc` rather than slipping through.

## Live regeneration with Vite

[`vite-plugin-tsroute`](packages/vite-plugin-tsroute) watches your Rust code while `vite dev` runs. Save a `.rs` file and the client, hooks and schemas regenerate. TypeScript then flags every call site that no longer matches. Rust compile errors appear in Vite's error overlay, and `vite build` regenerates first.

```ts
// vite.config.ts
import tsroute from "vite-plugin-tsroute";
export default defineConfig({ plugins: [react(), tsroute({ crate: "../api" })] });
```

## What's understood

| Rust | TypeScript |
|---|---|
| `Path<u64>` / `Path<(A, B)>` | positional args named after the path segments |
| `Path<SomeStruct>` | `params: SomeStruct` |
| `Query<T>` | `query: T` (optional if all fields are) |
| `Json<T>` argument | `body: T` |
| `Json<T>`, `(StatusCode, Json<T>)`, `Result<…, E>` | `Promise<T>` |
| `String`, `&str`, `Html<_>` | `Promise<string>` |
| `()`, `StatusCode` | `Promise<void>` |
| non-2xx response | throws `ApiError<E>` with `status`, parsed `body` and the raw `response` |

Other extractors (`State`, `Extension`, headers, …) are ignored. Compile-time errors catch mismatches between the path and the `Path` extractor, plus the old axum `:param` syntax.

## The generated client

* Zero dependencies, works in browsers, Node, Bun, Deno and React Native.
* `createClient({ baseUrl, fetch, headers })`: `headers` can be a function (e.g. to attach a fresh auth token).
* Every function accepts a trailing `RequestInit` (pass an `AbortSignal`, extra headers…).
* `Routes` interface exposes each route's response and error types for forms, mocks and tests.
* Compiles under the strictest settings (`strict`, `exactOptionalPropertyTypes`, `noUncheckedIndexedAccess`, `erasableSyntaxOnly`).

## Example

[`examples/todo-api`](examples/todo-api) is a full axum app with a React usage file and an end-to-end script:

```sh
cd examples/todo-api
cargo run &                     # start the API on :3000
cd web && npm i
npm run dev                     # React app; edit ../src/main.rs and watch the types update
npm run typecheck && npm run e2e
```

## Status

Early (0.2). Planned: multipart and form bodies, `Vec` query params via `serde_qs`, streaming/SSE responses, an `axum::Router` extension trait as an alternative to `routes!`, and MSW mock handlers.

## License

MIT
