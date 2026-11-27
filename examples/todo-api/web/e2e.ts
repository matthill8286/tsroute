// End-to-end: call the running Rust server through the generated client.
import { ApiError, createClient } from "./src/api/client.ts";

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

process.exit(failures ? 1 : 0);
