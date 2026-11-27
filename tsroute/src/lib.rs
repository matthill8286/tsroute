//! # tsroute
//!
//! Typed TypeScript clients and TanStack Query hooks, generated from the axum
//! handlers you already have. No RPC layer, no OpenAPI step, no second schema.
//!
//! ```ignore
//! use axum::{extract::Path, Json};
//! use serde::Serialize;
//! use ts_rs::TS;
//!
//! #[derive(Serialize, TS)]
//! struct User { id: u32, name: String }
//!
//! /// Fetch one user.
//! #[tsroute::get("/users/{id}")]
//! async fn get_user(Path(id): Path<u32>) -> Json<User> {
//!     Json(User { id, name: "Ada".into() })
//! }
//!
//! let app: axum::Router = tsroute::routes![get_user];
//!
//! // In a test or build step:
//! tsroute::Exporter::new()
//!     .client("web/src/api/client.ts")
//!     .react_query("web/src/api/hooks.ts")
//!     .sync()
//!     .unwrap();
//! ```

use std::{
    any::TypeId,
    collections::{BTreeMap, HashMap, HashSet},
    fmt::{self, Write as _},
    fs, io,
    path::{Component, Path, PathBuf},
};

pub use ts_rs;
pub use tsroute_macros::{delete, get, patch, post, put};

#[doc(hidden)]
pub mod __private {
    pub use axum;
    pub use axum::routing::MethodFilter;
    pub use inventory;
}

/// Build an [`axum::Router`] from handlers annotated with a `tsroute` attribute.
///
/// Paths and methods come from the attribute, so they are never written twice.
/// The result is a normal router: add state, layers or other routes as usual.
///
/// ```ignore
/// let app = tsroute::routes![list_todos, create_todo, todos::delete_todo]
///     .with_state(state);
/// ```
#[macro_export]
macro_rules! routes {
    ($($($seg:ident)::+),* $(,)?) => {{
        let router = $crate::__private::axum::Router::new();
        $(
            let router = router.route(
                $($seg)::+::PATH,
                $crate::__private::axum::routing::on($($seg)::+::METHOD, $($seg)::+),
            );
        )*
        router
    }};
}

/// Registers a Rust type with the [`Collector`] and returns its TypeScript form.
pub type TypeRef = fn(&mut Collector) -> TsType;

/// A resolved TypeScript type.
#[derive(Debug, Clone)]
pub struct TsType {
    /// The name to use in signatures, e.g. `User` or `Array<Todo>`.
    pub name: String,
    /// Whether the type is an object (struct), as opposed to a scalar.
    pub is_object: bool,
    /// An object whose fields are all optional (`{}` is a valid value).
    pub all_optional: bool,
}

/// How the handler extracts path parameters.
#[derive(Debug, Clone, Copy)]
pub enum PathSpec {
    None,
    /// `Path<T>`: a scalar (one segment) or a struct (fields named after segments).
    Single(TypeRef),
    /// `Path<(A, B)>`: one element per segment, in order.
    Tuple(&'static [TypeRef]),
}

/// What the handler responds with on success.
#[derive(Debug, Clone, Copy)]
pub enum ResponseSpec {
    Json(TypeRef),
    Text,
    Empty,
    Unknown,
}

/// Metadata for one route. Created by the attribute macros; you rarely need it directly.
#[derive(Debug)]
pub struct RouteInfo {
    pub method: &'static str,
    pub path: &'static str,
    pub path_params: &'static [&'static str],
    pub handler: &'static str,
    pub client_name: &'static str,
    pub module: &'static str,
    pub docs: &'static str,
    pub path_spec: PathSpec,
    pub query: Option<TypeRef>,
    pub body: Option<TypeRef>,
    pub response: ResponseSpec,
    pub error: Option<TypeRef>,
}

inventory::collect!(RouteInfo);

/// Every route registered in the binary, sorted by path then method.
pub fn registered_routes() -> Vec<&'static RouteInfo> {
    let mut routes: Vec<_> = inventory::iter::<RouteInfo>.into_iter().collect();
    routes.sort_by(|a, b| (a.path, a.method).cmp(&(b.path, b.method)));
    routes
}

/// Collects TypeScript declarations for every type reachable from the routes.
pub struct Collector {
    cfg: ts_rs::Config,
    seen: HashSet<TypeId>,
    /// TS name -> (rust type id, declaration)
    decls: BTreeMap<String, (TypeId, String)>,
    conflicts: Vec<String>,
}

impl Collector {
    fn new() -> Self {
        Self {
            // JSON has no bigint; serde_json writes i64/u64 as plain numbers.
            cfg: ts_rs::Config::new().with_large_int("number"),
            seen: HashSet::new(),
            decls: BTreeMap::new(),
            conflicts: Vec::new(),
        }
    }

    /// Register `T` (and everything it depends on) and return its TS type.
    pub fn ts<T: ts_rs::TS + 'static + ?Sized>(&mut self) -> TsType {
        self.add::<T>();
        let inline = T::inline(&self.cfg);
        let is_object = inline.trim_start().starts_with('{');
        TsType {
            name: T::name(&self.cfg),
            is_object,
            all_optional: is_object && object_fields_all_optional(&inline),
        }
    }

    fn add<T: ts_rs::TS + 'static + ?Sized>(&mut self) {
        if !self.seen.insert(TypeId::of::<T>()) {
            return;
        }
        if T::output_path().is_some() {
            let id = TypeId::of::<T::WithoutGenerics>();
            let ident = T::ident(&self.cfg);
            match self.decls.get(&ident) {
                Some((existing, _)) if *existing != id => self.conflicts.push(ident),
                Some(_) => {}
                None => {
                    let mut decl = String::new();
                    if let Some(docs) = T::docs() {
                        decl.push_str(&docs);
                        if !docs.ends_with('\n') {
                            decl.push('\n');
                        }
                    }
                    decl.push_str("export ");
                    decl.push_str(&T::decl(&self.cfg));
                    self.decls.insert(ident, (id, decl));
                }
            }
        }
        T::visit_dependencies(self);
        T::visit_generics(self);
    }
}

impl ts_rs::TypeVisitor for Collector {
    fn visit<T: ts_rs::TS + 'static + ?Sized>(&mut self) {
        self.add::<T>();
    }
}

/// Errors from exporting or checking generated files.
pub enum Error {
    Io(PathBuf, io::Error),
    /// A generated file differs from what is on disk (see [`Exporter::check`]).
    OutOfDate(Vec<PathBuf>),
    /// Two routes or types would get the same TypeScript name.
    Conflict(String),
    /// Nothing to do.
    NoOutputs,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Error::OutOfDate(paths) => {
                write!(
                    f,
                    "generated TypeScript is out of date; re-run the export. Stale files:"
                )?;
                for p in paths {
                    write!(f, "\n  - {}", p.display())?;
                }
                Ok(())
            }
            Error::Conflict(msg) => f.write_str(msg),
            Error::NoOutputs => f.write_str("no outputs configured; call .client(..) first"),
        }
    }
}

// Debug shows the readable message so `.unwrap()` in a test explains itself.
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for Error {}

/// Generates TypeScript from every registered route.
#[derive(Debug, Default, Clone)]
pub struct Exporter {
    client: Option<PathBuf>,
    react_query: Option<PathBuf>,
    react_query_import: Option<String>,
    module_prefix: Option<String>,
}

impl Exporter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Where to write the typed fetch client (types + `createClient`).
    pub fn client(mut self, path: impl Into<PathBuf>) -> Self {
        self.client = Some(path.into());
        self
    }

    /// Also write TanStack Query hooks (`createHooks(client)`) to this path.
    pub fn react_query(mut self, path: impl Into<PathBuf>) -> Self {
        self.react_query = Some(path.into());
        self
    }

    /// Override the import specifier the hooks file uses for the client
    /// (defaults to a relative path, e.g. `./client`).
    pub fn react_query_import(mut self, specifier: impl Into<String>) -> Self {
        self.react_query_import = Some(specifier.into());
        self
    }

    /// Only include routes whose handler lives under this module path,
    /// e.g. `"my_app::public"`. Useful for keeping internal routes out.
    pub fn only_module(mut self, prefix: impl Into<String>) -> Self {
        self.module_prefix = Some(prefix.into());
        self
    }

    fn routes(&self) -> Vec<&'static RouteInfo> {
        registered_routes()
            .into_iter()
            .filter(|r| match &self.module_prefix {
                Some(p) => r.module == p || r.module.starts_with(&format!("{p}::")),
                None => true,
            })
            .collect()
    }

    /// Render all configured outputs as `(path, contents)`.
    pub fn render(&self) -> Result<Vec<(PathBuf, String)>, Error> {
        let client_path = self.client.clone().ok_or(Error::NoOutputs)?;
        let model = build_model(&self.routes())?;
        let mut out = vec![(client_path.clone(), render_client(&model))];
        if let Some(hooks_path) = &self.react_query {
            let import = match &self.react_query_import {
                Some(s) => s.clone(),
                None => relative_import(hooks_path, &client_path),
            };
            out.push((hooks_path.clone(), render_hooks(&model, &import)));
        }
        Ok(out)
    }

    /// Write the generated files (only touching files whose contents changed).
    pub fn export(&self) -> Result<(), Error> {
        for (path, contents) in self.render()? {
            if fs::read_to_string(&path).ok().as_deref() == Some(contents.as_str()) {
                continue;
            }
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                fs::create_dir_all(dir).map_err(|e| Error::Io(dir.to_path_buf(), e))?;
            }
            fs::write(&path, contents).map_err(|e| Error::Io(path.clone(), e))?;
        }
        Ok(())
    }

    /// Fail if the files on disk don't match what would be generated.
    /// Run this in CI so the frontend can never drift from the API.
    pub fn check(&self) -> Result<(), Error> {
        let stale: Vec<PathBuf> = self
            .render()?
            .into_iter()
            .filter(|(p, c)| fs::read_to_string(p).ok().as_deref() != Some(c.as_str()))
            .map(|(p, _)| p)
            .collect();
        if stale.is_empty() {
            Ok(())
        } else {
            Err(Error::OutOfDate(stale))
        }
    }

    /// [`check`](Self::check) when the `CI` or `TSROUTE_CHECK` env var is set,
    /// otherwise [`export`](Self::export). Ideal inside a `#[test]`.
    pub fn sync(&self) -> Result<(), Error> {
        let check = ["CI", "TSROUTE_CHECK"]
            .iter()
            .any(|v| std::env::var(v).is_ok_and(|v| !v.is_empty() && v != "0" && v != "false"));
        if check {
            self.check()
        } else {
            self.export()
        }
    }
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

struct Param {
    name: String,
    ty: String,
    optional: bool,
}

impl Param {
    fn new(name: impl Into<String>, ty: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ty: ty.into(),
            optional: false,
        }
    }
    fn sig(&self) -> String {
        format!(
            "{}{}: {}",
            self.name,
            if self.optional { "?" } else { "" },
            self.ty
        )
    }
}

enum UrlPart {
    Lit(String),
    /// A JS expression producing the segment value.
    Expr(String),
}

struct RouteModel {
    info: &'static RouteInfo,
    name: String,
    params: Vec<Param>,
    url: Vec<UrlPart>,
    query: Option<String>,
    body: Option<String>,
    response: String,
    kind: &'static str,
    error: String,
}

struct Model {
    decls: Vec<String>,
    type_names: Vec<String>,
    routes: Vec<RouteModel>,
}

fn build_model(routes: &[&'static RouteInfo]) -> Result<Model, Error> {
    let mut c = Collector::new();
    let mut models = Vec::new();
    let mut names: HashMap<String, &'static RouteInfo> = HashMap::new();

    for &info in routes {
        let name = camel_case(info.client_name);
        if let Some(prev) = names.insert(name.clone(), info) {
            return Err(Error::Conflict(format!(
                "two routes map to the client function `{name}`: {}::{} and {}::{}. \
                 Rename one or pass `name = \"..\"` to the attribute.",
                prev.module, prev.handler, info.module, info.handler
            )));
        }

        let mut params = Vec::new();
        let mut seg_exprs: HashMap<&str, String> = HashMap::new();
        match info.path_spec {
            PathSpec::None => {}
            PathSpec::Tuple(refs) => {
                for (seg, r) in info.path_params.iter().zip(refs) {
                    let ty = r(&mut c);
                    let pname = camel_case(seg);
                    seg_exprs.insert(seg, pname.clone());
                    params.push(Param::new(pname, ty.name));
                }
            }
            PathSpec::Single(r) => {
                let ty = r(&mut c);
                if ty.is_object {
                    for seg in info.path_params {
                        seg_exprs.insert(seg, format!("params[{}]", js_string(seg)));
                    }
                    params.push(Param::new("params", ty.name));
                } else {
                    for seg in info.path_params {
                        let pname = camel_case(seg);
                        seg_exprs.insert(seg, pname.clone());
                        params.push(Param::new(pname, ty.name.clone()));
                    }
                }
            }
        }

        let body = info.body.map(|r| r(&mut c));
        let query = info.query.map(|r| r(&mut c));
        if let Some(b) = &body {
            params.push(Param::new("body", b.name.clone()));
        }
        if let Some(q) = &query {
            params.push(Param::new("query", q.name.clone()));
        }
        // Trailing all-optional objects can be omitted by the caller.
        for (p, ty) in params
            .iter_mut()
            .rev()
            .zip([query.as_ref(), body.as_ref()].into_iter().flatten())
        {
            if !ty.all_optional {
                break;
            }
            p.optional = true;
        }
        let body = body.map(|t| t.name);
        let query = query.map(|t| t.name);

        let mut url = Vec::new();
        for seg in info.path.split('/').skip(1) {
            url.push(UrlPart::Lit("/".into()));
            match seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                Some(p) => {
                    let wildcard = p.starts_with('*');
                    let key = p.trim_start_matches('*');
                    let expr = seg_exprs
                        .get(key)
                        .cloned()
                        .unwrap_or_else(|| "undefined".into());
                    url.push(UrlPart::Expr(if wildcard {
                        // Wildcards may contain slashes; encode each piece.
                        format!("String({expr}).split(\"/\").map(encodeURIComponent).join(\"/\")")
                    } else {
                        format!("encodeURIComponent(String({expr}))")
                    }));
                }
                None => url.push(UrlPart::Lit(seg.to_string())),
            }
        }

        let (response, kind) = match info.response {
            ResponseSpec::Json(r) => (r(&mut c).name, "json"),
            ResponseSpec::Text => ("string".into(), "text"),
            ResponseSpec::Empty => ("void".into(), "empty"),
            ResponseSpec::Unknown => ("unknown".into(), "auto"),
        };
        let error = info
            .error
            .map(|r| r(&mut c).name)
            .unwrap_or_else(|| "unknown".into());

        models.push(RouteModel {
            info,
            name,
            params,
            url,
            query,
            body,
            response,
            kind,
            error,
        });
    }

    if let Some(name) = c.conflicts.first() {
        return Err(Error::Conflict(format!(
            "two different Rust types both export as TypeScript `{name}`; \
             rename one with #[ts(rename = \"..\")]"
        )));
    }

    let type_names = c.decls.keys().cloned().collect();
    let decls = c.decls.into_values().map(|(_, d)| d).collect();
    Ok(Model {
        decls,
        type_names,
        routes: models,
    })
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

const HEADER: &str = "// Generated by tsroute from your axum routes. Do not edit by hand.\n\
                      /* eslint-disable */\n";

const RUNTIME: &str = r#"
/** Thrown for any non-2xx response. `body` is the parsed JSON (or text) error body. */
export class ApiError<E = unknown> extends Error {
  readonly status: number;
  readonly body: E;
  readonly response: Response;
  constructor(status: number, body: E, response: Response) {
    super(`Request failed with status ${status}`);
    this.name = "ApiError";
    this.status = status;
    this.body = body;
    this.response = response;
  }
}

export interface ClientOptions {
  /** Prefix for every request, e.g. `https://api.example.com`. Defaults to same origin. */
  baseUrl?: string;
  /** Custom fetch (for SSR, tests or auth wrappers). Defaults to `globalThis.fetch`. */
  fetch?: typeof fetch;
  /** Headers sent with every request. A function is called per request (handy for tokens). */
  headers?: HeadersInit | (() => HeadersInit | Promise<HeadersInit>);
}

type ResponseKind = "json" | "text" | "empty" | "auto";

function toSearch(query: unknown): string {
  if (query == null || typeof query !== "object") return "";
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query as Record<string, unknown>)) {
    if (value == null) continue;
    for (const v of Array.isArray(value) ? value : [value]) params.append(key, String(v));
  }
  const s = params.toString();
  return s ? `?${s}` : "";
}

async function readBody(res: Response, kind: ResponseKind): Promise<unknown> {
  if (kind === "empty" || res.status === 204) return undefined;
  const text = await res.text();
  if (kind === "text") return text;
  if (!text) return undefined;
  const isJson = (res.headers.get("content-type") ?? "").includes("json");
  if (kind === "json" || isJson) {
    try {
      return JSON.parse(text);
    } catch {
      return text;
    }
  }
  return text;
}

function makeRequest(options: ClientOptions) {
  return async function request<T>(
    method: string,
    path: string,
    kind: ResponseKind,
    args: { query?: unknown; body?: unknown; init?: RequestInit | undefined },
  ): Promise<T> {
    const doFetch = options.fetch ?? globalThis.fetch;
    const base = typeof options.headers === "function" ? await options.headers() : options.headers;
    const headers = new Headers(base);
    new Headers(args.init?.headers).forEach((v, k) => headers.set(k, v));
    if (args.body !== undefined && !headers.has("content-type")) {
      headers.set("content-type", "application/json");
    }
    if (!headers.has("accept")) headers.set("accept", "application/json");
    const res = await doFetch(`${options.baseUrl ?? ""}${path}${toSearch(args.query)}`, {
      ...args.init,
      method,
      headers,
      body: args.body === undefined ? null : JSON.stringify(args.body),
    });
    if (!res.ok) {
      throw new ApiError(res.status, await readBody(res, "auto"), res);
    }
    return (await readBody(res, kind)) as T;
  };
}
"#;

fn render_client(m: &Model) -> String {
    let mut s = String::from(HEADER);

    if !m.decls.is_empty() {
        s.push_str(
            "\n// ---- Types --------------------------------------------------------------\n\n",
        );
        for d in &m.decls {
            s.push_str(d);
            s.push_str("\n\n");
        }
    }

    s.push_str("// ---- Runtime ------------------------------------------------------------\n");
    s.push_str(RUNTIME);

    s.push_str(
        "\n// ---- Routes -------------------------------------------------------------\n\n",
    );
    s.push_str("/** Create a typed client for the API. */\n");
    s.push_str("export function createClient(options: ClientOptions = {}) {\n");
    s.push_str("  const request = makeRequest(options);\n");
    s.push_str("  return {\n");
    for r in &m.routes {
        write_jsdoc(&mut s, "    ", r);
        let mut sig: Vec<String> = r.params.iter().map(Param::sig).collect();
        sig.push("init?: RequestInit".into());
        let url: String = r
            .url
            .iter()
            .map(|p| match p {
                UrlPart::Lit(l) => l.replace('`', "\\`").replace("${", "\\${"),
                UrlPart::Expr(e) => format!("${{{e}}}"),
            })
            .collect();
        let mut args = Vec::new();
        if r.query.is_some() {
            args.push("query");
        }
        if r.body.is_some() {
            args.push("body");
        }
        args.push("init");
        let _ = writeln!(
            s,
            "    {}: (\n      {},\n    ): Promise<{}> =>\n      request<{}>({}, `{}`, {}, {{ {} }}),\n",
            r.name,
            sig.join(",\n      "),
            r.response,
            r.response,
            js_string(r.info.method),
            url,
            js_string(r.kind),
            args.join(", "),
        );
    }
    trim_blank(&mut s);
    s.push_str("  };\n}\n\n");
    s.push_str("export type ApiClient = ReturnType<typeof createClient>;\n\n");

    // Per-route helper types: handy for forms, mocks and error handling.
    s.push_str("/** Request, response and error types for each route. */\n");
    s.push_str("export interface Routes {\n");
    for r in &m.routes {
        let _ = writeln!(
            s,
            "  {}: {{ method: {}; path: {}; response: {}; error: ApiError<{}>; }};",
            r.name,
            js_string(r.info.method),
            js_string(r.info.path),
            r.response,
            r.error
        );
    }
    s.push_str("}\n");
    s
}

fn write_jsdoc(s: &mut String, indent: &str, r: &RouteModel) {
    let _ = writeln!(s, "{indent}/**");
    for line in r.info.docs.lines() {
        let _ = writeln!(s, "{indent} * {}", line.replace("*/", "*\\/").trim_end());
    }
    if !r.info.docs.is_empty() {
        let _ = writeln!(s, "{indent} *");
    }
    let _ = writeln!(s, "{indent} * `{} {}`", r.info.method, r.info.path);
    let _ = writeln!(s, "{indent} * @throws {{ApiError<{}>}}", r.error);
    let _ = writeln!(s, "{indent} */");
}

fn render_hooks(m: &Model, import: &str) -> String {
    let mut s = String::from(HEADER);
    s.push_str(
        "import {\n  useMutation,\n  useQuery,\n  type UseMutationOptions,\n  type UseQueryOptions,\n} from \"@tanstack/react-query\";\n",
    );
    let mut imports = vec!["ApiClient".to_string(), "ApiError".to_string()];
    let used: Vec<&String> = m
        .type_names
        .iter()
        .filter(|t| {
            m.routes.iter().any(|r| {
                std::iter::once(&r.response)
                    .chain(std::iter::once(&r.error))
                    .chain(r.params.iter().map(|p| &p.ty))
                    .any(|ty| mentions(ty, t))
            })
        })
        .collect();
    imports.extend(used.into_iter().cloned());
    let _ = writeln!(
        s,
        "import type {{ {} }} from {};\n",
        imports.join(", "),
        js_string(import)
    );

    // Query keys: stable, typed, and shareable for invalidation.
    s.push_str("/** Query keys used by the generated hooks. Use them with `queryClient.invalidateQueries`. */\n");
    s.push_str("export const queryKeys = {\n");
    for r in m.routes.iter().filter(|r| r.info.method == "GET") {
        let sig: Vec<String> = r.params.iter().map(Param::sig).collect();
        let mut key = vec![js_string(&r.name)];
        key.extend(r.params.iter().map(|p| p.name.clone()));
        let _ = writeln!(
            s,
            "  {}: ({}) => [{}] as const,",
            r.name,
            sig.join(", "),
            key.join(", ")
        );
    }
    s.push_str("};\n\n");

    s.push_str("/** Create TanStack Query hooks bound to a client from `createClient`. */\n");
    s.push_str("export function createHooks(client: ApiClient) {\n  return {\n");
    for r in &m.routes {
        let hook = format!("use{}", pascal_first(&r.name));
        let err = format!("ApiError<{}>", r.error);
        write_jsdoc(&mut s, "    ", r);
        if r.info.method == "GET" {
            let mut sig: Vec<String> = r.params.iter().map(Param::sig).collect();
            sig.push(format!(
                "options?: Omit<UseQueryOptions<{}, {err}>, \"queryKey\" | \"queryFn\">",
                r.response
            ));
            let names: Vec<&str> = r.params.iter().map(|p| p.name.as_str()).collect();
            let call_args = names.join(", ");
            let client_args = if names.is_empty() {
                "{ signal }".to_string()
            } else {
                format!("{call_args}, {{ signal }}")
            };
            let _ = writeln!(
                s,
                "    {hook}: (\n      {},\n    ) =>\n      useQuery({{\n        queryKey: queryKeys.{}({call_args}),\n        queryFn: ({{ signal }}) => client.{}({client_args}),\n        ...options,\n      }}),\n",
                sig.join(",\n      "),
                r.name,
                r.name,
            );
        } else {
            // Mutation variables: nothing -> void, only a body -> the body, else an object.
            let (vars_ty, call) = match (r.params.as_slice(), &r.body) {
                ([], _) => ("void".to_string(), format!("() => client.{}()", r.name)),
                ([only], Some(_)) if only.name == "body" => (
                    only.ty.clone(),
                    format!("(body) => client.{}(body)", r.name),
                ),
                (ps, _) => {
                    let fields: Vec<String> = ps.iter().map(Param::sig).collect();
                    let args: Vec<String> = ps.iter().map(|p| format!("vars.{}", p.name)).collect();
                    (
                        format!("{{ {} }}", fields.join("; ")),
                        format!("(vars) => client.{}({})", r.name, args.join(", ")),
                    )
                }
            };
            let _ = writeln!(
                s,
                "    {hook}: (\n      options?: Omit<UseMutationOptions<{}, {err}, {vars_ty}>, \"mutationFn\">,\n    ) =>\n      useMutation({{\n        mutationFn: {call},\n        ...options,\n      }}),\n",
                r.response,
            );
        }
    }
    trim_blank(&mut s);
    s.push_str("  };\n}\n");
    s
}

fn trim_blank(s: &mut String) {
    while s.ends_with("\n\n") {
        s.pop();
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn camel_case(s: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for (i, ch) in s.chars().enumerate() {
        if ch == '_' || ch == '-' {
            upper = i > 0 && !out.is_empty();
        } else if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn pascal_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn js_string(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Whether every field of an inline TS object type (`{ a?: T, b?: U, }`) is optional.
fn object_fields_all_optional(inline: &str) -> bool {
    let body = inline.trim();
    let Some(body) = body.strip_prefix('{').and_then(|b| b.strip_suffix('}')) else {
        return false;
    };
    let mut fields = Vec::new();
    let (mut depth, mut start, mut quote, mut comment) = (0i32, 0usize, None::<char>, false);
    let bytes: Vec<(usize, char)> = body.char_indices().collect();
    let mut i = 0;
    while i < bytes.len() {
        let (pos, ch) = bytes[i];
        let next = bytes.get(i + 1).map(|b| b.1);
        if comment {
            if ch == '*' && next == Some('/') {
                comment = false;
                i += 1;
            }
        } else if let Some(q) = quote {
            if ch == '\\' {
                i += 1;
            } else if ch == q {
                quote = None;
            }
        } else {
            match ch {
                '/' if next == Some('*') => comment = true,
                '"' | '\'' | '`' => quote = Some(ch),
                '{' | '[' | '(' | '<' => depth += 1,
                '}' | ']' | ')' | '>' => depth -= 1,
                ',' if depth == 0 => {
                    fields.push(&body[start..pos]);
                    start = pos + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    fields.push(&body[start..]);
    fields
        .into_iter()
        .map(strip_comments)
        .filter(|f| !f.trim().is_empty())
        .all(|f| {
            f.split(':')
                .next()
                .is_some_and(|k| k.trim_end().ends_with('?'))
        })
}

fn strip_comments(field: &str) -> String {
    let mut out = String::new();
    let mut rest = field;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start..]
            .find("*/")
            .map_or("", |end| &rest[start + end + 2..]);
    }
    out.push_str(rest);
    out
}

/// Whether the TS type expression `ty` mentions the identifier `name`.
fn mentions(ty: &str, name: &str) -> bool {
    ty.match_indices(name).any(|(i, _)| {
        let before = ty[..i].chars().next_back();
        let after = ty[i + name.len()..].chars().next();
        let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Import specifier from `from` (a file) to `to` (a .ts file), e.g. `./client`.
fn relative_import(from: &Path, to: &Path) -> String {
    let from_dir: Vec<Component> = from
        .parent()
        .map(|p| p.components().collect())
        .unwrap_or_default();
    let to_stem = to.with_extension("");
    let to_parts: Vec<Component> = to_stem.components().collect();
    let common = from_dir
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from_dir.len() {
        parts.push("..".into());
    }
    for c in &to_parts[common..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    let joined = parts.join("/");
    if joined.starts_with("..") {
        joined
    } else {
        format!("./{joined}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel() {
        assert_eq!(camel_case("get_user_by_id"), "getUserById");
        assert_eq!(camel_case("todo_id"), "todoId");
        assert_eq!(camel_case("_private"), "private");
    }

    #[test]
    fn relative() {
        assert_eq!(
            relative_import(
                Path::new("web/api/hooks.ts"),
                Path::new("web/api/client.ts")
            ),
            "./client"
        );
        assert_eq!(
            relative_import(Path::new("web/hooks/q.ts"), Path::new("web/api/client.ts")),
            "../api/client"
        );
        assert_eq!(
            relative_import(Path::new("hooks.ts"), Path::new("client.ts")),
            "./client"
        );
    }

    #[test]
    fn optional_fields() {
        assert!(object_fields_all_optional(
            "{ done?: boolean, priority?: Priority, }"
        ));
        assert!(object_fields_all_optional("{ }"));
        assert!(!object_fields_all_optional(
            "{ done?: boolean, title: string, }"
        ));
        assert!(object_fields_all_optional(
            "{ /** a: b */ a?: { x: number, y: string }, }"
        ));
        assert!(!object_fields_all_optional(
            "{ a?: { x?: number }, b: string }"
        ));
    }

    #[test]
    fn mentions_whole_identifiers() {
        assert!(mentions("Array<Todo>", "Todo"));
        assert!(!mentions("Array<TodoList>", "Todo"));
        assert!(mentions("Todo | null", "Todo"));
    }
}
