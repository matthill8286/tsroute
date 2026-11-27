// End-to-end: call the running Rust server through the generated client.
import { z } from "zod";
import { ApiError, createClient } from "./src/api/client.ts";
import { CreateTodoSchema, withValidation } from "./src/api/schemas.ts";

const api = createClient({ baseUrl: process.env.API ?? "http://127.0.0.1:3000" });
let failures = 0;
function check(name: string, ok: boolean, detail?: unknown) {
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${ok ? "" : `  ${JSON.stringify(detail)}`}`);
  if (!ok) failures++;
}

check("health returns text", (await api.healthCheck()) === "ok");

const a = await api.createTodo({ title: "Write tsroute", priority: "high", notes: "MVP" });
const b = await api.createTodo({ title: "Publish crate", priority: "normal" });
check("create returns typed todo", a.id === 1 && a.priority === "high" && a.done === false, a);

const all = await api.listTodos();
check("list without query", all.total === 2, all);

await api.updateTodo(a.id, { done: true });
const open = await api.listTodos({ done: false });
check("query params filter", open.total === 1 && open.items[0]?.id === b.id, open);
const high = await api.listTodos({ priority: "high" });
check("enum query param", high.total === 1 && high.items[0]?.done === true, high);

const comments = await api.addComment(a.id, "looks good");
check("json string body + path param", comments.length === 1 && comments[0] === "looks good", comments);
check("struct path params", (await api.getComment({ todo_id: a.id, index: 0 })) === "looks good");
check("Option<T> -> null", (await api.getComment({ todo_id: a.id, index: 9 })) === null);

check("delete returns void", (await api.deleteTodo(b.id)) === undefined);

try {
  await api.getTodo(b.id);
  check("404 throws", false);
} catch (e) {
  const ok = e instanceof ApiError && e.status === 404 && (e.body as { code: string }).code === "not_found";
  check("404 throws typed ApiError with JSON body", ok, e);
}

try {
  await api.createTodo({ title: " ", priority: "low" });
  check("422 throws", false);
} catch (e) {
  check("422 validation error", e instanceof ApiError && e.status === 422, e);
}

// Runtime validation with the generated Zod schemas.
const validated = withValidation(createClient({ baseUrl: process.env.API ?? "http://127.0.0.1:3000" }));
const v = await validated.getTodo(a.id);
check("withValidation passes real responses", v.id === a.id);
check("withValidation handles generics", (await validated.listTodos()).total === 1);

const lying = withValidation(
  createClient({ fetch: async () => Response.json({ id: "not-a-number", title: 1 }) }),
);
try {
  await lying.getTodo(1);
  check("withValidation rejects bad data", false);
} catch (e) {
  check("withValidation rejects bad data", e instanceof z.ZodError, e);
}
check("input schemas work for forms", !CreateTodoSchema.safeParse({ title: "x", priority: "urgent" }).success);

process.exit(failures ? 1 : 0);
