//! Proc macros for `tsroute`. Use them through the `tsroute` crate.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote, quote_spanned};
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input,
    spanned::Spanned,
    Expr, ExprLit, FnArg, GenericArgument, ItemFn, Lit, LitStr, Meta, PathArguments, ReturnType,
    Token, Type,
};

macro_rules! method_attr {
    ($($name:ident => $method:literal),* $(,)?) => {$(
        #[doc = concat!("Register an axum handler as a typed `", $method, "` route.")]
        ///
        /// ```ignore
        /// #[tsroute::get("/users/{id}")]
        /// async fn get_user(Path(id): Path<u32>) -> Json<User> { /* ... */ }
        /// ```
        ///
        /// Options: `error = Type` (the JSON error body, must implement `ts_rs::TS`)
        /// and `name = "customClientName"`.
        #[proc_macro_attribute]
        pub fn $name(attr: TokenStream, item: TokenStream) -> TokenStream {
            let args = parse_macro_input!(attr as RouteArgs);
            let func = parse_macro_input!(item as ItemFn);
            expand($method, args, func)
                .unwrap_or_else(syn::Error::into_compile_error)
                .into()
        }
    )*};
}

method_attr! {
    get => "GET",
    post => "POST",
    put => "PUT",
    patch => "PATCH",
    delete => "DELETE",
}

struct RouteArgs {
    path: LitStr,
    error: Option<Type>,
    name: Option<LitStr>,
}

impl Parse for RouteArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let path: LitStr = input.parse()?;
        let mut error = None;
        let mut name = None;
        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let key: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "error" => error = Some(input.parse()?),
                "name" => name = Some(input.parse()?),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown tsroute option `{other}` (expected `error` or `name`)"),
                    ))
                }
            }
        }
        Ok(Self { path, error, name })
    }
}

/// The last path segment of a type, e.g. `Json` for `axum::Json<T>`, with its generic args.
fn last_segment(ty: &Type) -> Option<(String, Vec<&Type>)> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    let args = match &seg.arguments {
        PathArguments::AngleBracketed(a) => a
            .args
            .iter()
            .filter_map(|a| match a {
                GenericArgument::Type(t) => Some(t),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    Some((seg.ident.to_string(), args))
}

/// A `fn(&mut Collector) -> String` that registers a type and returns its TS name.
fn type_ref(ty: &Type) -> TokenStream2 {
    quote_spanned! {ty.span()=>
        {
            fn __ts(c: &mut ::tsroute::Collector) -> ::tsroute::TsType {
                c.ts::<#ty>()
            }
            __ts as ::tsroute::TypeRef
        }
    }
}

fn parse_path_params(path: &LitStr) -> syn::Result<Vec<String>> {
    let value = path.value();
    if !value.starts_with('/') {
        return Err(syn::Error::new(
            path.span(),
            "route path must start with `/`",
        ));
    }
    let mut params = Vec::new();
    for seg in value.split('/') {
        if seg.starts_with(':') {
            return Err(syn::Error::new(
                path.span(),
                "axum 0.8 uses `{param}` syntax, not `:param`",
            ));
        }
        if let Some(inner) = seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            params.push(inner.trim_start_matches('*').to_string());
        }
    }
    Ok(params)
}

enum Response {
    Json(Box<Type>),
    Text,
    Empty,
    Unknown,
}

fn classify_response(ty: &Type) -> Response {
    match ty {
        Type::Tuple(t) if t.elems.is_empty() => return Response::Empty,
        // `(StatusCode, Json<T>)`, `(StatusCode, HeaderMap, Json<T>)`: the body is the last element.
        Type::Tuple(t) => {
            return t
                .elems
                .last()
                .map(classify_response)
                .unwrap_or(Response::Unknown)
        }
        Type::Reference(r) => return classify_response(&r.elem),
        _ => {}
    }
    match last_segment(ty) {
        Some((name, args)) => match (name.as_str(), args.as_slice()) {
            ("Json", [inner]) => Response::Json(Box::new((*inner).clone())),
            ("String" | "str" | "Html", _) => Response::Text,
            ("StatusCode" | "NoContent", _) => Response::Empty,
            _ => Response::Unknown,
        },
        None => Response::Unknown,
    }
}

fn expand(method: &str, args: RouteArgs, func: ItemFn) -> syn::Result<TokenStream2> {
    let fn_name = &func.sig.ident;
    let vis = &func.vis;
    let path_lit = &args.path;
    let segments = parse_path_params(path_lit)?;

    if func.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            func.sig.fn_token.span(),
            "tsroute handlers must be `async fn`",
        ));
    }
    if !func.sig.generics.params.is_empty() {
        return Err(syn::Error::new(
            func.sig.generics.span(),
            "tsroute handlers cannot be generic (the client needs concrete types)",
        ));
    }

    // Doc comments become JSDoc on the generated client function.
    let docs: Vec<String> = func
        .attrs
        .iter()
        .filter_map(|a| match &a.meta {
            Meta::NameValue(nv) if nv.path.is_ident("doc") => match &nv.value {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(s), ..
                }) => Some(s.value().trim().to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let docs = docs.join("\n");

    // Extractors.
    let mut path_spec = quote!(::tsroute::PathSpec::None);
    let mut query = quote!(::core::option::Option::None);
    let mut body = quote!(::core::option::Option::None);
    let mut seen_path = false;
    for arg in &func.sig.inputs {
        let FnArg::Typed(pat_ty) = arg else {
            return Err(syn::Error::new(
                arg.span(),
                "tsroute handlers cannot take `self`",
            ));
        };
        let Some((name, generics)) = last_segment(&pat_ty.ty) else {
            continue;
        };
        let [inner] = generics.as_slice() else {
            continue;
        };
        match name.as_str() {
            "Path" => {
                seen_path = true;
                path_spec = match inner {
                    Type::Tuple(t) => {
                        if t.elems.len() != segments.len() {
                            return Err(syn::Error::new(
                                t.span(),
                                format!(
                                    "path has {} parameter(s) ({}) but the extractor has {} element(s)",
                                    segments.len(),
                                    segments.join(", "),
                                    t.elems.len()
                                ),
                            ));
                        }
                        let refs = t.elems.iter().map(type_ref);
                        quote!(::tsroute::PathSpec::Tuple(&[#(#refs),*]))
                    }
                    other => {
                        let r = type_ref(other);
                        quote!(::tsroute::PathSpec::Single(#r))
                    }
                };
            }
            "Query" => {
                let r = type_ref(inner);
                query = quote!(::core::option::Option::Some(#r));
            }
            "Json" => {
                let r = type_ref(inner);
                body = quote!(::core::option::Option::Some(#r));
            }
            _ => {}
        }
    }
    if !segments.is_empty() && !seen_path {
        return Err(syn::Error::new(
            path_lit.span(),
            format!(
                "path declares parameter(s) {} but the handler has no `Path<..>` extractor",
                segments.join(", ")
            ),
        ));
    }

    // Return type: unwrap `Result<T, E>`.
    let ret_ty: Type = match &func.sig.output {
        ReturnType::Default => syn::parse_quote!(()),
        ReturnType::Type(_, t) => (**t).clone(),
    };
    let ok_ty = match last_segment(&ret_ty) {
        Some((n, g)) if n == "Result" && !g.is_empty() => g[0].clone(),
        _ => ret_ty.clone(),
    };
    let response = match classify_response(&ok_ty) {
        Response::Json(t) => {
            let r = type_ref(&t);
            quote!(::tsroute::ResponseSpec::Json(#r))
        }
        Response::Text => quote!(::tsroute::ResponseSpec::Text),
        Response::Empty => quote!(::tsroute::ResponseSpec::Empty),
        Response::Unknown => quote!(::tsroute::ResponseSpec::Unknown),
    };
    let error = match &args.error {
        Some(t) => {
            let r = type_ref(t);
            quote!(::core::option::Option::Some(#r))
        }
        None => quote!(::core::option::Option::None),
    };

    let client_name = match &args.name {
        Some(n) => n.value(),
        None => fn_name.to_string(),
    };
    let fn_name_str = fn_name.to_string();
    let method_ident = format_ident!("{}", method);
    let segment_lits = segments.iter().map(|s| LitStr::new(s, Span::call_site()));

    Ok(quote! {
        #func

        /// Route metadata generated by `tsroute`. Used by `tsroute::routes!`.
        #[doc(hidden)]
        #[allow(non_snake_case, dead_code)]
        #vis mod #fn_name {
            pub const PATH: &str = #path_lit;
            pub const METHOD: ::tsroute::__private::MethodFilter =
                ::tsroute::__private::MethodFilter::#method_ident;
        }

        ::tsroute::__private::inventory::submit! {
            ::tsroute::RouteInfo {
                method: #method,
                path: #path_lit,
                path_params: &[#(#segment_lits),*],
                handler: #fn_name_str,
                client_name: #client_name,
                module: ::core::module_path!(),
                docs: #docs,
                path_spec: #path_spec,
                query: #query,
                body: #body,
                response: #response,
                error: #error,
            }
        }
    })
}
