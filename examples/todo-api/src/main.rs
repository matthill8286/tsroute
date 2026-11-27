//! A small todo API showing tsroute. Run `cargo test` to (re)generate
//! `web/src/api/client.ts` and `web/src/api/hooks.ts`.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ---- Types (shared with the frontend via ts-rs) ----------------------------

/// A single todo item.
#[derive(Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Todo {
    pub id: u64,
    pub title: String,
    pub done: bool,
    pub priority: Priority,
    /// Optional free-text notes.
    pub notes: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    Normal,
    High,
}

#[derive(Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct CreateTodo {
    pub title: String,
    pub priority: Priority,
    pub notes: Option<String>,
}

#[derive(Deserialize, TS)]
#[ts(optional_fields)]
pub struct UpdateTodo {
    pub title: Option<String>,
    pub done: Option<bool>,
}

#[derive(Deserialize, TS)]
#[ts(optional_fields)]
pub struct ListFilter {
    pub done: Option<bool>,
    pub priority: Option<Priority>,
}

#[derive(Serialize, TS)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: usize,
}

#[derive(Deserialize, TS)]
pub struct CommentPath {
    pub todo_id: u64,
    pub index: usize,
}

/// JSON body of every error response.
#[derive(Serialize, TS)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

pub struct ApiError(StatusCode, &'static str, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            code: self.1.into(),
            message: self.2,
        };
        (self.0, Json(body)).into_response()
    }
}

fn not_found(id: u64) -> ApiError {
    ApiError(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("todo {id} does not exist"),
    )
}

// ---- State -----------------------------------------------------------------

#[derive(Clone, Default)]
pub struct AppState {
    todos: Arc<Mutex<BTreeMap<u64, Todo>>>,
    comments: Arc<Mutex<BTreeMap<u64, Vec<String>>>>,
}

// ---- Handlers --------------------------------------------------------------

/// List todos, optionally filtered.
#[tsroute::get("/todos")]
async fn list_todos(State(s): State<AppState>, Query(f): Query<ListFilter>) -> Json<Page<Todo>> {
    let items: Vec<Todo> = s
        .todos
        .lock()
        .unwrap()
        .values()
        .filter(|t| f.done.is_none_or(|d| t.done == d))
        .filter(|t| f.priority.is_none_or(|p| t.priority == p))
        .cloned()
        .collect();
    Json(Page {
        total: items.len(),
        items,
    })
}

/// Get one todo by id.
#[tsroute::get("/todos/{id}", error = ErrorBody)]
async fn get_todo(State(s): State<AppState>, Path(id): Path<u64>) -> Result<Json<Todo>, ApiError> {
    s.todos
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(not_found(id))
}

/// Create a todo.
#[tsroute::post("/todos", error = ErrorBody)]
async fn create_todo(
    State(s): State<AppState>,
    Json(input): Json<CreateTodo>,
) -> Result<(StatusCode, Json<Todo>), ApiError> {
    if input.title.trim().is_empty() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            "title is required".into(),
        ));
    }
    let mut todos = s.todos.lock().unwrap();
    let id = todos.keys().last().map_or(1, |k| k + 1);
    let todo = Todo {
        id,
        title: input.title,
        done: false,
        priority: input.priority,
        notes: input.notes,
    };
    todos.insert(id, todo.clone());
    Ok((StatusCode::CREATED, Json(todo)))
}

/// Update a todo's title or done flag.
#[tsroute::patch("/todos/{id}", error = ErrorBody)]
async fn update_todo(
    State(s): State<AppState>,
    Path(id): Path<u64>,
    Json(patch): Json<UpdateTodo>,
) -> Result<Json<Todo>, ApiError> {
    let mut todos = s.todos.lock().unwrap();
    let todo = todos.get_mut(&id).ok_or(not_found(id))?;
    if let Some(t) = patch.title {
        todo.title = t;
    }
    if let Some(d) = patch.done {
        todo.done = d;
    }
    Ok(Json(todo.clone()))
}

#[tsroute::delete("/todos/{id}", error = ErrorBody)]
async fn delete_todo(
    State(s): State<AppState>,
    Path(id): Path<u64>,
) -> Result<StatusCode, ApiError> {
    s.todos
        .lock()
        .unwrap()
        .remove(&id)
        .map(|_| StatusCode::NO_CONTENT)
        .ok_or(not_found(id))
}

/// Add a comment to a todo.
#[tsroute::post("/todos/{todo_id}/comments")]
async fn add_comment(
    State(s): State<AppState>,
    Path(todo_id): Path<u64>,
    Json(text): Json<String>,
) -> Json<Vec<String>> {
    let mut all = s.comments.lock().unwrap();
    let list = all.entry(todo_id).or_default();
    list.push(text);
    Json(list.clone())
}

/// Read one comment (struct path params).
#[tsroute::get("/todos/{todo_id}/comments/{index}")]
async fn get_comment(
    State(s): State<AppState>,
    Path(p): Path<CommentPath>,
) -> Json<Option<String>> {
    Json(
        s.comments
            .lock()
            .unwrap()
            .get(&p.todo_id)
            .and_then(|c| c.get(p.index).cloned()),
    )
}

/// Liveness probe.
#[tsroute::get("/health", name = "healthCheck")]
async fn health() -> &'static str {
    "ok"
}

pub fn app() -> Router {
    tsroute::routes![
        list_todos,
        get_todo,
        create_todo,
        update_todo,
        delete_todo,
        add_comment,
        get_comment,
        health,
    ]
    .with_state(AppState::default())
}

#[tokio::main]
async fn main() {
    let addr = std::env::var("ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    println!("listening on http://{addr}");
    axum::serve(listener, app()).await.unwrap();
}

#[cfg(test)]
mod tests {
    /// Regenerates the TypeScript client locally; fails in CI if it's stale.
    #[test]
    fn typescript_client_is_up_to_date() {
        tsroute::Exporter::new()
            .client("web/src/api/client.ts")
            .react_query("web/src/api/hooks.ts")
            .sync()
            .unwrap();
    }
}
