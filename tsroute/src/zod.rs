//! Turns the TypeScript that ts-rs emits into Zod schemas.
//!
//! ts-rs output is a small, regular subset of TypeScript, so a tiny parser is enough:
//! primitives, literals, unions, intersections, object literals, index signatures,
//! tuples, `Array<T>`, `Record<K, V>` and references to other declarations (with generics).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ty {
    Prim(&'static str),
    Str(String),
    Num(String),
    Bool(bool),
    Ref(String, Vec<Ty>),
    Array(Box<Ty>),
    Tuple(Vec<Ty>),
    Record(Box<Ty>, Box<Ty>),
    Object(Vec<Field>),
    Union(Vec<Ty>),
    Intersection(Vec<Ty>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Field {
    name: String,
    optional: bool,
    ty: Ty,
}

/// A parsed `type Name<A, B> = ...;` declaration.
#[derive(Debug, Clone)]
pub(crate) struct Decl {
    pub generics: Vec<String>,
    pub body: Option<Ty>,
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Num(String),
    Punct(char),
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '"' || c == '\'' {
            let mut s = String::new();
            i += 1;
            while i < chars.len() && chars[i] != c {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                }
                s.push(chars[i]);
                i += 1;
            }
            i += 1;
            out.push(Tok::Str(s));
        } else if c.is_ascii_digit()
            || (c == '-' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                i += 1;
            }
            out.push(Tok::Num(chars[start..i].iter().collect()));
        } else if c.is_alphabetic() || c == '_' || c == '$' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
            {
                i += 1;
            }
            out.push(Tok::Ident(chars[start..i].iter().collect()));
        } else if "{}[]<>(),:;?|&=".contains(c) {
            out.push(Tok::Punct(c));
            i += 1;
        } else {
            return Err(format!("unexpected character `{c}`"));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Punct(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, c: char) -> Result<(), String> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(format!("expected `{c}`, found {:?}", self.peek()))
        }
    }

    fn ty(&mut self) -> Result<Ty, String> {
        self.eat('|');
        let mut parts = vec![self.intersection()?];
        while self.eat('|') {
            parts.push(self.intersection()?);
        }
        Ok(if parts.len() == 1 {
            parts.pop().unwrap()
        } else {
            Ty::Union(parts)
        })
    }

    fn intersection(&mut self) -> Result<Ty, String> {
        let mut parts = vec![self.postfix()?];
        while self.eat('&') {
            parts.push(self.postfix()?);
        }
        Ok(if parts.len() == 1 {
            parts.pop().unwrap()
        } else {
            Ty::Intersection(parts)
        })
    }

    fn postfix(&mut self) -> Result<Ty, String> {
        let mut t = self.primary()?;
        // `T[]`
        while self.peek() == Some(&Tok::Punct('['))
            && self.toks.get(self.pos + 1) == Some(&Tok::Punct(']'))
        {
            self.pos += 2;
            t = Ty::Array(Box::new(t));
        }
        Ok(t)
    }

    fn type_args(&mut self) -> Result<Vec<Ty>, String> {
        let mut args = Vec::new();
        if self.eat('<') {
            loop {
                args.push(self.ty()?);
                if self.eat('>') {
                    break;
                }
                self.expect(',')?;
            }
        }
        Ok(args)
    }

    fn primary(&mut self) -> Result<Ty, String> {
        match self.next() {
            Some(Tok::Str(s)) => Ok(Ty::Str(s)),
            Some(Tok::Num(n)) => Ok(Ty::Num(n)),
            Some(Tok::Punct('(')) => {
                let t = self.ty()?;
                self.expect(')')?;
                Ok(t)
            }
            Some(Tok::Punct('[')) => {
                let mut items = Vec::new();
                while !self.eat(']') {
                    items.push(self.ty()?);
                    self.eat(',');
                }
                Ok(Ty::Tuple(items))
            }
            Some(Tok::Punct('{')) => self.object(),
            Some(Tok::Ident(id)) => {
                let prim = match id.as_str() {
                    "string" => Some("string"),
                    "number" => Some("number"),
                    "boolean" => Some("boolean"),
                    "null" => Some("null"),
                    "undefined" => Some("undefined"),
                    "unknown" | "any" | "object" => Some("unknown"),
                    "never" => Some("never"),
                    "bigint" => Some("bigint"),
                    "void" => Some("void"),
                    _ => None,
                };
                if let Some(p) = prim {
                    return Ok(Ty::Prim(p));
                }
                match id.as_str() {
                    "true" => return Ok(Ty::Bool(true)),
                    "false" => return Ok(Ty::Bool(false)),
                    _ => {}
                }
                let args = self.type_args()?;
                match (id.as_str(), args.as_slice()) {
                    ("Array" | "ReadonlyArray", [t]) => Ok(Ty::Array(Box::new(t.clone()))),
                    ("Record", [k, v]) => Ok(Ty::Record(Box::new(k.clone()), Box::new(v.clone()))),
                    ("Partial", [Ty::Record(k, v)]) => Ok(Ty::Record(k.clone(), v.clone())),
                    _ => Ok(Ty::Ref(id, args)),
                }
            }
            other => Err(format!("unexpected token {other:?}")),
        }
    }

    fn object(&mut self) -> Result<Ty, String> {
        // Index signature: `{ [key in K]?: V }` or `{ [key: K]: V }`
        if self.peek() == Some(&Tok::Punct('[')) {
            self.pos += 1;
            let _name = self.next();
            let key = if self.eat(':') {
                self.ty()?
            } else {
                match self.next() {
                    Some(Tok::Ident(kw)) if kw == "in" => self.ty()?,
                    other => return Err(format!("bad index signature near {other:?}")),
                }
            };
            self.expect(']')?;
            self.eat('?');
            self.expect(':')?;
            let value = self.ty()?;
            self.eat(',');
            self.eat(';');
            self.expect('}')?;
            return Ok(Ty::Record(Box::new(key), Box::new(value)));
        }
        let mut fields = Vec::new();
        while !self.eat('}') {
            let name = match self.next() {
                Some(Tok::Ident(n)) | Some(Tok::Str(n)) | Some(Tok::Num(n)) => n,
                other => return Err(format!("expected field name, found {other:?}")),
            };
            let optional = self.eat('?');
            self.expect(':')?;
            let ty = self.ty()?;
            if !self.eat(',') {
                self.eat(';');
            }
            fields.push(Field { name, optional, ty });
        }
        Ok(Ty::Object(fields))
    }
}

pub(crate) fn parse_type(src: &str) -> Result<Ty, String> {
    let mut p = Parser {
        toks: lex(src)?,
        pos: 0,
    };
    let t = p.ty()?;
    if p.pos < p.toks.len() {
        return Err(format!(
            "trailing tokens after type: {:?}",
            &p.toks[p.pos..]
        ));
    }
    Ok(t)
}

/// Parse a ts-rs declaration such as `type Page<T> = { items: Array<T>, };`.
pub(crate) fn parse_decl(src: &str) -> Decl {
    let parsed = (|| -> Result<(Vec<String>, Ty), String> {
        let mut p = Parser {
            toks: lex(src)?,
            pos: 0,
        };
        match p.next() {
            Some(Tok::Ident(kw)) if kw == "type" => {}
            Some(Tok::Ident(kw)) if kw == "enum" => return Err("TS enums are not supported".into()),
            other => return Err(format!("expected `type`, found {other:?}")),
        }
        p.next(); // name
        let mut generics = Vec::new();
        if p.eat('<') {
            loop {
                match p.next() {
                    Some(Tok::Ident(g)) => generics.push(g),
                    other => return Err(format!("bad generic parameter {other:?}")),
                }
                // Skip defaults / constraints: `T = unknown`, `T extends X`.
                while !matches!(
                    p.peek(),
                    Some(Tok::Punct(',')) | Some(Tok::Punct('>')) | None
                ) {
                    p.next();
                }
                if p.eat('>') {
                    break;
                }
                p.expect(',')?;
            }
        }
        p.expect('=')?;
        let body = p.ty()?;
        p.eat(';');
        Ok((generics, body))
    })();
    match parsed {
        Ok((generics, body)) => Decl {
            generics,
            body: Some(body),
        },
        Err(_) => Decl {
            generics: Vec::new(),
            body: None,
        },
    }
}

// ---------------------------------------------------------------------------
// Zod emitter
// ---------------------------------------------------------------------------

pub(crate) fn schema_name(type_name: &str) -> String {
    format!("{type_name}Schema")
}

fn refs(t: &Ty, out: &mut BTreeSet<String>) {
    match t {
        Ty::Ref(n, args) => {
            out.insert(n.clone());
            args.iter().for_each(|a| refs(a, out));
        }
        Ty::Array(t) => refs(t, out),
        Ty::Record(k, v) => {
            refs(k, out);
            refs(v, out);
        }
        Ty::Tuple(ts) | Ty::Union(ts) | Ty::Intersection(ts) => {
            ts.iter().for_each(|t| refs(t, out))
        }
        Ty::Object(fs) => fs.iter().for_each(|f| refs(&f.ty, out)),
        _ => {}
    }
}

pub(crate) struct Emitter<'a> {
    known: &'a HashMap<String, Decl>,
    /// Declarations not yet defined at this point in the file (forward / cyclic refs).
    lazy: &'a HashSet<String>,
}

fn js_key(name: &str) -> String {
    let ident = name
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$');
    if ident {
        name.to_string()
    } else {
        format!("{name:?}")
    }
}

impl Emitter<'_> {
    pub(crate) fn emit(&self, t: &Ty, generics: &[String]) -> String {
        match t {
            Ty::Prim(p) => match *p {
                "string" => "z.string()".into(),
                "number" => "z.number()".into(),
                "boolean" => "z.boolean()".into(),
                "null" => "z.null()".into(),
                "undefined" | "void" => "z.undefined()".into(),
                "never" => "z.never()".into(),
                "bigint" => "z.bigint()".into(),
                _ => "z.unknown()".into(),
            },
            Ty::Str(s) => format!("z.literal({s:?})"),
            Ty::Num(n) => format!("z.literal({n})"),
            Ty::Bool(b) => format!("z.literal({b})"),
            Ty::Array(inner) => format!("z.array({})", self.emit(inner, generics)),
            Ty::Tuple(items) => {
                let items: Vec<_> = items.iter().map(|i| self.emit(i, generics)).collect();
                format!("z.tuple([{}])", items.join(", "))
            }
            Ty::Record(k, v) => {
                let key = match &**k {
                    Ty::Prim("number") => "z.coerce.number()".to_string(),
                    other => self.emit(other, generics),
                };
                format!("z.record({key}, {})", self.emit(v, generics))
            }
            Ty::Object(fields) => {
                if fields.is_empty() {
                    return "z.object({})".into();
                }
                let fs: Vec<String> = fields
                    .iter()
                    .map(|f| {
                        let mut s = self.emit(&f.ty, generics);
                        if f.optional {
                            s.push_str(".exactOptional()");
                        }
                        format!("{}: {s}", js_key(&f.name))
                    })
                    .collect();
                format!("z.object({{ {} }})", fs.join(", "))
            }
            Ty::Union(parts) => {
                let nullable = parts.contains(&Ty::Prim("null"));
                let rest: Vec<&Ty> = parts.iter().filter(|p| **p != Ty::Prim("null")).collect();
                let base = if !rest.is_empty() && rest.iter().all(|p| matches!(p, Ty::Str(_))) {
                    let vals: Vec<String> = rest
                        .iter()
                        .map(|p| match p {
                            Ty::Str(s) => format!("{s:?}"),
                            _ => unreachable!(),
                        })
                        .collect();
                    format!("z.enum([{}])", vals.join(", "))
                } else if rest.len() == 1 {
                    self.emit(rest[0], generics)
                } else if rest.is_empty() {
                    return "z.null()".into();
                } else {
                    let vs: Vec<_> = rest.iter().map(|p| self.emit(p, generics)).collect();
                    format!("z.union([{}])", vs.join(", "))
                };
                if nullable {
                    format!("{base}.nullable()")
                } else {
                    base
                }
            }
            Ty::Intersection(parts) => {
                let mut it = parts.iter().map(|p| self.emit(p, generics));
                let first = it.next().unwrap_or_else(|| "z.unknown()".into());
                it.fold(first, |acc, p| format!("z.intersection({acc}, {p})"))
            }
            Ty::Ref(name, args) => {
                if args.is_empty() && generics.contains(name) {
                    return name.clone();
                }
                if !self.known.get(name).is_some_and(|d| d.body.is_some()) {
                    return "z.unknown()".into();
                }
                let args: Vec<_> = args.iter().map(|a| self.emit(a, generics)).collect();
                let call = if args.is_empty() {
                    schema_name(name)
                } else {
                    format!("{}({})", schema_name(name), args.join(", "))
                };
                if self.lazy.contains(name) {
                    format!("z.lazy(() => {call})")
                } else {
                    call
                }
            }
        }
    }
}

/// Order declarations so dependencies come first. Returns the order and the set of
/// declarations that take part in a cycle (they need `z.lazy` and a type annotation).
pub(crate) fn order(decls: &BTreeMap<String, Decl>) -> (Vec<String>, HashSet<String>) {
    let deps: HashMap<&str, BTreeSet<String>> = decls
        .iter()
        .map(|(n, d)| {
            let mut s = BTreeSet::new();
            if let Some(b) = &d.body {
                refs(b, &mut s);
            }
            s.retain(|r| decls.contains_key(r) && !d.generics.contains(r));
            (n.as_str(), s)
        })
        .collect();

    #[derive(PartialEq, Clone, Copy)]
    enum Mark {
        Visiting,
        Done,
    }
    fn visit<'a>(
        n: &'a str,
        deps: &'a HashMap<&'a str, BTreeSet<String>>,
        marks: &mut HashMap<&'a str, Mark>,
        stack: &mut Vec<&'a str>,
        out: &mut Vec<String>,
        cyclic: &mut HashSet<String>,
    ) {
        match marks.get(n) {
            Some(Mark::Done) => return,
            Some(Mark::Visiting) => {
                // Everything on the stack from `n` upwards is part of a cycle.
                if let Some(i) = stack.iter().position(|s| *s == n) {
                    cyclic.extend(stack[i..].iter().map(|s| s.to_string()));
                }
                return;
            }
            None => {}
        }
        marks.insert(n, Mark::Visiting);
        stack.push(n);
        for d in &deps[n] {
            let (key, _) = deps.get_key_value(d.as_str()).unwrap();
            visit(key, deps, marks, stack, out, cyclic);
        }
        stack.pop();
        marks.insert(n, Mark::Done);
        out.push(n.to_string());
    }

    let mut marks = HashMap::new();
    let mut out = Vec::new();
    let mut cyclic = HashSet::new();
    for n in decls.keys() {
        visit(
            n.as_str(),
            &deps,
            &mut marks,
            &mut Vec::new(),
            &mut out,
            &mut cyclic,
        );
    }
    (out, cyclic)
}

// ---------------------------------------------------------------------------
// File rendering
// ---------------------------------------------------------------------------

/// Render the `schemas.ts` file.
pub(crate) fn render(m: &crate::Model, import: &str) -> String {
    use std::fmt::Write as _;

    let decls: BTreeMap<String, Decl> = m
        .raw_decls
        .iter()
        .map(|(n, raw)| (n.clone(), parse_decl(raw)))
        .collect();
    let (order, cyclic) = order(&decls);
    let known: HashMap<String, Decl> = decls.clone().into_iter().collect();

    let mut s = String::from(crate::HEADER);
    s.push_str("import { z } from \"zod\";\n");
    let mut imports = vec!["ApiClient".to_string()];
    imports.extend(m.type_names.iter().cloned());
    let _ = writeln!(
        s,
        "import type {{ {} }} from {};\n",
        imports.join(", "),
        crate::js_string(import)
    );

    s.push_str("// ---- Schemas ------------------------------------------------------------\n\n");
    let mut defined: HashSet<String> = HashSet::new();
    let mut checks = Vec::new();
    for name in &order {
        let d = &decls[name];
        let schema = schema_name(name);
        let Some(body) = &d.body else {
            let _ = writeln!(
                s,
                "/** `{name}` could not be converted; validated as unknown. */"
            );
            let _ = writeln!(s, "export const {schema} = z.unknown();\n");
            continue;
        };
        // Anything not defined yet (a forward or cyclic reference) must be lazy.
        let lazy: HashSet<String> = decls
            .keys()
            .filter(|k| !defined.contains(*k))
            .cloned()
            .collect();
        let expr = Emitter {
            known: &known,
            lazy: &lazy,
        }
        .emit(body, &d.generics);
        if d.generics.is_empty() {
            let annotation = if cyclic.contains(name) {
                format!(": z.ZodType<{name}>")
            } else {
                String::new()
            };
            let _ = writeln!(s, "export const {schema}{annotation} = {expr};\n");
            checks.push(format!(
                "  void (null! as z.output<typeof {schema}> satisfies {name});"
            ));
        } else {
            let params: Vec<String> = d
                .generics
                .iter()
                .map(|g| format!("{g} extends z.ZodType"))
                .collect();
            let args: Vec<String> = d.generics.iter().map(|g| format!("{g}: {g}")).collect();
            let _ = writeln!(
                s,
                "export const {schema} = <{}>({}) =>\n  {expr};\n",
                params.join(", "),
                args.join(", ")
            );
        }
        defined.insert(name.clone());
    }

    let none = HashSet::new();
    let e = Emitter {
        known: &known,
        lazy: &none,
    };
    let to_zod = |ty: &str| match parse_type(ty) {
        Ok(t) => e.emit(&t, &[]),
        Err(_) => "z.unknown()".to_string(),
    };
    s.push_str("// ---- Routes -------------------------------------------------------------\n\n");
    s.push_str("/** Zod schemas for each route's inputs, JSON response and error body. */\n");
    s.push_str("export const routeSchemas = {\n");
    for r in &m.routes {
        let mut parts = Vec::new();
        if let Some(b) = &r.body {
            parts.push(format!("body: {}", to_zod(b)));
        }
        if let Some(q) = &r.query {
            parts.push(format!("query: {}", to_zod(q)));
        }
        match r.kind {
            "json" => parts.push(format!("response: {}", to_zod(&r.response))),
            "text" => parts.push("response: z.string()".into()),
            _ => {}
        }
        if r.error != "unknown" {
            parts.push(format!("error: {}", to_zod(&r.error)));
        }
        let _ = writeln!(s, "  {}: {{ {} }},", r.name, parts.join(", "));
    }
    s.push_str("} as const;\n\n");

    s.push_str(
        r#"/**
 * Wrap a client so every response is checked against its schema at runtime.
 * A mismatch throws a `z.ZodError` instead of letting bad data into your UI.
 */
export function withValidation<C extends ApiClient>(client: C): C {
  const wrapped: Record<string, unknown> = { ...client };
  for (const [name, schemas] of Object.entries(routeSchemas)) {
    const response = (schemas as { response?: z.ZodType }).response;
    const call = (client as Record<string, unknown>)[name];
    if (!response || typeof call !== "function") continue;
    wrapped[name] = async (...args: unknown[]) =>
      response.parse(await (call as (...a: unknown[]) => Promise<unknown>)(...args));
  }
  return wrapped as C;
}
"#,
    );

    if !checks.is_empty() {
        s.push_str("\n// Compile-time proof that every schema produces its TypeScript type.\n");
        s.push_str("function _schemasMatchTypes() {\n");
        for c in &checks {
            s.push_str(c);
            s.push('\n');
        }
        s.push_str("}\nvoid _schemasMatchTypes;\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zod(src: &str) -> String {
        let known = HashMap::new();
        let lazy = HashSet::new();
        Emitter {
            known: &known,
            lazy: &lazy,
        }
        .emit(&parse_type(src).unwrap(), &[])
    }

    #[test]
    fn primitives_and_unions() {
        assert_eq!(zod("string | null"), "z.string().nullable()");
        assert_eq!(
            zod(r#""low" | "normal" | "high""#),
            r#"z.enum(["low", "normal", "high"])"#
        );
        assert_eq!(zod("Array<number>"), "z.array(z.number())");
        assert_eq!(zod("[string, number]"), "z.tuple([z.string(), z.number()])");
        assert_eq!(zod("number | string"), "z.union([z.number(), z.string()])");
    }

    #[test]
    fn objects() {
        assert_eq!(
            zod("{ id: number, /** docs */ notes?: string, \"type\": \"a\", }"),
            r#"z.object({ id: z.number(), notes: z.string().exactOptional(), type: z.literal("a") })"#
        );
        assert_eq!(
            zod("{ [key in string]?: number }"),
            "z.record(z.string(), z.number())"
        );
        assert_eq!(zod("{ }"), "z.object({})");
    }

    #[test]
    fn tagged_enum() {
        let src = r#"{ "type": "Circle", radius: number, } | { "type": "Square", side: number, }"#;
        assert_eq!(
            zod(src),
            r#"z.union([z.object({ type: z.literal("Circle"), radius: z.number() }), z.object({ type: z.literal("Square"), side: z.number() })])"#
        );
    }

    #[test]
    fn decl_with_generics() {
        let d = parse_decl("type Page<T> = { items: Array<T>, total: number, };");
        assert_eq!(d.generics, vec!["T".to_string()]);
        assert!(d.body.is_some());
    }

    #[test]
    fn ordering_and_cycles() {
        let mut decls = BTreeMap::new();
        decls.insert("A".to_string(), parse_decl("type A = { b: B, };"));
        decls.insert("B".to_string(), parse_decl("type B = { n: number, };"));
        decls.insert(
            "Tree".to_string(),
            parse_decl("type Tree = { kids: Array<Tree>, };"),
        );
        let (order, cyclic) = order(&decls);
        assert!(order.iter().position(|x| x == "B") < order.iter().position(|x| x == "A"));
        assert!(cyclic.contains("Tree") && !cyclic.contains("A"));
    }
}
