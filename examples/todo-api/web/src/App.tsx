// Example React usage of the generated client + hooks. Also a type-level test:
// every `@ts-expect-error` below must be a real error or `tsc` fails.
import { QueryClient, QueryClientProvider, useQueryClient } from "@tanstack/react-query";
import { createClient, type ErrorBody, type Routes } from "./api/client";
import { createHooks, queryKeys } from "./api/hooks";

const client = createClient({
  baseUrl: import.meta.env.VITE_API_URL ?? "http://localhost:3000",
  headers: () => ({ authorization: `Bearer ${localStorage.getItem("token") ?? ""}` }),
});
const api = createHooks(client);
const queryClient = new QueryClient();

function TodoList() {
  const qc = useQueryClient();
  const todos = api.useListTodos({ done: false });
  const create = api.useCreateTodo({
    onSuccess: () => qc.invalidateQueries({ queryKey: queryKeys.listTodos() }),
  });
  const toggle = api.useUpdateTodo();

  if (todos.isPending) return <p>Loading…</p>;
  if (todos.isError) return <p>Failed: {todos.error.status}</p>;

  return (
    <div>
      <button onClick={() => create.mutate({ title: "Ship tsroute", priority: "high" })}>Add</button>
      {create.error && <p>{create.error.body.message}</p>}
      <ul>
        {todos.data.items.map((t) => (
          <li key={t.id} onClick={() => toggle.mutate({ id: t.id, body: { done: !t.done } })}>
            {t.title} ({t.priority})
          </li>
        ))}
      </ul>
    </div>
  );
}

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <TodoList />
    </QueryClientProvider>
  );
}

// ---- Type-level checks -------------------------------------------------------

async function typeChecks() {
  const todo = await client.getTodo(1);
  const title: string = todo.title;

  // @ts-expect-error: id is a number
  await client.getTodo("1");
  // @ts-expect-error: priority must be "low" | "normal" | "high"
  await client.createTodo({ title: "x", priority: "urgent" });
  // @ts-expect-error: title is required
  await client.createTodo({ priority: "low" });
  // @ts-expect-error: unknown field
  await client.updateTodo(1, { titel: "typo" });

  const err: Routes["getTodo"]["error"]["body"] = { code: "not_found", message: "" };
  const body: ErrorBody = err;
  const deleted: void = await client.deleteTodo(1);
  const comment: string | null = await client.getComment({ todo_id: 1, index: 0 });
  return [title, body, deleted, comment];
}
void typeChecks;
