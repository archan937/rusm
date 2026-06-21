//! Proc-macros for `rusm-rs`. `#[rusm_rs::service]` turns a module of free
//! functions into a RUSM service: it keeps the functions and adds a `serve()`
//! dispatch loop (receive a request → call the matching function → reply) plus a
//! typed `Client` whose methods are blocking calls over the same JSON wire as
//! rusm-ts. Mirrors a TS service's `export function`s — no `impl` block, no `self`.
//!
//! A handler returning `impl Iterator<Item = T>` is a **streaming** method; a
//! parameter of type `Callback<T>` is a **callback** (the client passes a closure,
//! the service gets a handle whose invocations travel back as messages).

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    FnArg, GenericArgument, Ident, ItemFn, ItemMod, Pat, PathArguments, ReturnType, Type,
    TypeParamBound,
};

/// `#[rusm_rs::service]` on an inline `mod` of `pub fn`s.
#[proc_macro_attribute]
pub fn service(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let module = syn::parse_macro_input!(item as ItemMod);
    expand(module)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// The `rusm:runtime` WIT, embedded `inline:` into `generate!` so a component built
/// with [`main`] carries no `wit/` dir and is bindings-identical to a hand-written one.
/// This is a vendored copy of `rusm-rs/wit/world.wit` — owned here so the macro crate
/// is self-contained when published; a test (`wit_in_sync`) keeps the two byte-equal.
const RUNTIME_WIT: &str = include_str!("../wit/world.wit");

/// Parse the optional `#[handlers(bridge = "…")]` / `#[main(bridge = "…")]` attribute into
/// the **custom-bridge** interface refs the component imports (e.g.
/// `weather:bridge/forecast@0.1.0`, repeatable). Empty for the common case — a component over
/// pure `rusm:runtime`, which carries no `wit/` dir.
fn bridge_refs(attr: proc_macro2::TokenStream) -> syn::Result<Vec<String>> {
    use syn::parse::Parser;
    if attr.is_empty() {
        return Ok(Vec::new());
    }
    let metas =
        syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated.parse2(attr)?;
    metas
        .into_iter()
        .map(|meta| {
            let syn::Meta::NameValue(nv) = meta else {
                return Err(syn::Error::new_spanned(
                    meta,
                    "expected `bridge = \"ns:pkg/iface@ver\"`",
                ));
            };
            if !nv.path.is_ident("bridge") {
                return Err(syn::Error::new_spanned(
                    &nv.path,
                    "unknown key; the only key is `bridge`",
                ));
            }
            match nv.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) => Ok(s.value()),
                other => Err(syn::Error::new_spanned(
                    other,
                    "`bridge` value must be a string literal",
                )),
            }
        })
        .collect()
}

/// The `with:` mapping that reuses the `rusm-rs` SDK's bindings for the `rusm:runtime`
/// interfaces — so a component shares the SDK's `Pid`/`Request`/… types instead of
/// regenerating look-alikes.
fn runtime_mappings() -> proc_macro2::TokenStream {
    quote! {
        "rusm:runtime/actor@0.1.0": ::rusm_rs::rusm::runtime::actor,
        "rusm:runtime/kv@0.1.0": ::rusm_rs::rusm::runtime::kv,
        "rusm:runtime/log@0.1.0": ::rusm_rs::rusm::runtime::log,
        "rusm:runtime/pg@0.1.0": ::rusm_rs::rusm::runtime::pg,
        "rusm:runtime/streams@0.1.0": ::rusm_rs::rusm::runtime::streams,
        "rusm:runtime/serve@0.1.0": ::rusm_rs::rusm::runtime::serve
    }
}

/// The `generate!` for a component shell. With **no** custom bridges it's the dir-free
/// `inline:` `process` world (a pure-guest component carries no `wit/`). **With** custom
/// bridges it binds the `wit/` world `rusm build` generated — `rusm:runtime` mapped to the
/// SDK, the bridges bound by `generate_all` (the world's extra imports).
fn component_bindings(has_bridges: bool) -> proc_macro2::TokenStream {
    let with = runtime_mappings();
    if has_bridges {
        quote! {
            ::wit_bindgen::generate!({
                path: "wit",
                world: "handler",
                generate_all,
                with: { #with },
            });
        }
    } else {
        quote! {
            ::wit_bindgen::generate!({
                inline: #RUNTIME_WIT,
                world: "process",
                with: { #with },
            });
        }
    }
}

/// `pub use` each custom bridge interface at the crate root, so a handler calls it as
/// `crate::<iface>::<func>(…)` rather than reaching into the generated `__rusm_component`.
fn bridge_reexports(refs: &[String]) -> syn::Result<Vec<proc_macro2::TokenStream>> {
    refs.iter()
        .map(|r| {
            let no_ver = r.split('@').next().unwrap_or(r);
            let bad = || {
                syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!("bridge `{r}`: expected `ns:pkg/iface[@ver]`"),
                )
            };
            let (pkg, iface) = no_ver.split_once('/').ok_or_else(bad)?;
            let (ns, name) = pkg.split_once(':').ok_or_else(bad)?;
            let ns = format_ident!("{}", ns.replace('-', "_"));
            let name = format_ident!("{}", name.replace('-', "_"));
            let iface = format_ident!("{}", iface.replace('-', "_"));
            Ok(quote! { pub use __rusm_component::#ns::#name::#iface; })
        })
        .collect()
}

/// `#[rusm_rs::main]` on a component's entry fn (conventionally `fn main`). Hides the
/// whole component shell — the world, the `Guest` impl, and `export!` — so the source is
/// just the developer's handler plus one call to a `serve` fn. A pure-`rusm:runtime`
/// component needs no `wit/` directory; add `bridge = "ns:pkg/iface@ver"` to import a custom
/// application bridge (then `rusm build` generates the component's `wit/`).
#[proc_macro_attribute]
pub fn main(attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = syn::parse_macro_input!(item as ItemFn);
    let refs = match bridge_refs(attr.into()) {
        Ok(refs) => refs,
        Err(e) => return e.into_compile_error().into(),
    };
    let reexports = match bridge_reexports(&refs) {
        Ok(r) => r,
        Err(e) => return e.into_compile_error().into(),
    };
    let bindings = component_bindings(!refs.is_empty());
    let entry = func.sig.ident.clone();
    quote! {
        #func

        #(#reexports)*

        #[doc(hidden)]
        mod __rusm_component {
            #bindings

            struct Component;
            impl Guest for Component {
                fn run() {
                    ::rusm_rs::logging::init();
                    super::#entry();
                }
            }
            export!(Component);
        }
    }
    .into()
}

/// `#[rusm_rs::handlers]` on an inline `mod` of `pub fn action(Request, Params) -> Response`.
/// Turns the module into a **per-request HTTP handler component**: the host resolves the
/// `[routes]` table, spawns this component per request, and dispatches the matched action
/// here. Generates the whole component shell (`process` world, `Guest`, `export!`) and the
/// action dispatch — the developer writes only handler functions: no `main`, no router, no
/// request/reply plumbing.
#[proc_macro_attribute]
pub fn handlers(attr: TokenStream, item: TokenStream) -> TokenStream {
    let module = syn::parse_macro_input!(item as ItemMod);
    let refs = match bridge_refs(attr.into()) {
        Ok(refs) => refs,
        Err(e) => return e.into_compile_error().into(),
    };
    let reexports = match bridge_reexports(&refs) {
        Ok(r) => r,
        Err(e) => return e.into_compile_error().into(),
    };
    let bindings = component_bindings(!refs.is_empty());
    let Some((_, items)) = &module.content else {
        return syn::Error::new_spanned(&module, "#[handlers] needs an inline module body")
            .into_compile_error()
            .into();
    };
    let module_ident = &module.ident;
    // Each `pub fn` in the module is an action, dispatched by its name (the `#action` half
    // of a `component#action` route): a `fn(Request, Params) -> Response`. (Server-Sent
    // Events are a per-connection `rusm_rs::sse::serve` handler now, not a 3-arg action.)
    let arms = items.iter().filter_map(|item| match item {
        syn::Item::Fn(f) if matches!(f.vis, syn::Visibility::Public(_)) => {
            let name = &f.sig.ident;
            let action = name.to_string();
            if f.sig.inputs.len() != 2 {
                return Some(
                    syn::Error::new_spanned(
                        &f.sig,
                        "a #[handlers] action takes (Request, Params) -> Response; \
                         for Server-Sent Events use a per-connection `rusm_rs::sse::serve` handler",
                    )
                    .into_compile_error(),
                );
            }
            Some(quote! {
                #action => ::core::option::Option::Some(
                    super::#module_ident::#name(__request, __params)
                ),
            })
        }
        _ => None,
    });
    quote! {
        #module

        #(#reexports)*

        #[doc(hidden)]
        mod __rusm_component {
            #bindings

            struct Component;
            impl Guest for Component {
                fn run() {
                    ::rusm_rs::logging::init();
                    ::rusm_rs::http::serve_request(|__action, __request, __params| match __action {
                        #(#arms)*
                        _ => ::core::option::Option::None,
                    });
                }
            }
            export!(Component);
        }
    }
    .into()
}

/// One parameter: a plain data value, or a callback carrying its item type.
struct Param {
    name: Ident,
    ty: Type,
    /// `Some(item)` if the parameter is `Callback<item>`.
    callback: Option<Type>,
}

/// One handler's parsed signature.
struct Handler {
    name: Ident,
    op: String,
    params: Vec<Param>,
    ret: proc_macro2::TokenStream,
    /// `Some(item)` when the handler returns `impl Iterator<Item = item>`.
    stream_item: Option<Type>,
}

impl Handler {
    fn has_callback(&self) -> bool {
        self.params.iter().any(|p| p.callback.is_some())
    }
}

fn expand(module: ItemMod) -> syn::Result<proc_macro2::TokenStream> {
    let content = module.content.as_ref().ok_or_else(|| {
        syn::Error::new_spanned(&module, "#[service] needs an inline module body")
    })?;
    let items = &content.1;

    let handlers: Vec<Handler> = items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Fn(f) => Some(parse_handler(f)),
            _ => None,
        })
        .collect::<syn::Result<_>>()?;

    let arms = handlers.iter().map(serve_arm);
    let methods = handlers.iter().map(client_method);

    let attrs = &module.attrs;
    let vis = &module.vis;
    let ident = &module.ident;
    Ok(quote! {
        #(#attrs)* #vis mod #ident {
            #(#items)*

            /// Run the request → dispatch → reply loop forever (a service's body).
            pub fn serve() -> ! {
                loop {
                    let req = rusm_rs::wire::next_request();
                    match req.op.as_str() {
                        #(#arms)*
                        other => rusm_rs::wire::reply_err(
                            &req, &format!("no such function: {}", other)),
                    }
                }
            }

            /// A typed, blocking client for this service.
            #[derive(Clone, Copy)]
            pub struct Client {
                pub pid: rusm_rs::Pid,
            }

            impl Client {
                /// Spawn a fresh instance of this service by name and connect to it.
                pub fn spawn(component: &str) -> ::core::result::Result<Self, ::std::string::String> {
                    ::core::result::Result::Ok(Self { pid: rusm_rs::spawn(component)? })
                }

                /// Connect to an already-running instance by pid.
                pub fn connect(pid: rusm_rs::Pid) -> Self {
                    Self { pid }
                }

                #(#methods)*
            }
        }
    })
}

fn parse_handler(f: &ItemFn) -> syn::Result<Handler> {
    let name = f.sig.ident.clone();
    let op = name.to_string();
    let mut params = Vec::new();
    for input in &f.sig.inputs {
        match input {
            FnArg::Receiver(r) => {
                return Err(syn::Error::new_spanned(
                    r,
                    "#[service] functions are free functions — no `self`",
                ))
            }
            FnArg::Typed(pt) => {
                let Pat::Ident(p) = &*pt.pat else {
                    return Err(syn::Error::new_spanned(
                        &pt.pat,
                        "parameters must be simple identifiers",
                    ));
                };
                let ty = (*pt.ty).clone();
                let callback = callback_item(&ty);
                params.push(Param {
                    name: p.ident.clone(),
                    ty,
                    callback,
                });
            }
        }
    }
    let (ret, stream_item) = match &f.sig.output {
        ReturnType::Default => (quote!(()), None),
        ReturnType::Type(_, ty) => (quote!(#ty), impl_iterator_item(ty)),
    };
    Ok(Handler {
        name,
        op,
        params,
        ret,
        stream_item,
    })
}

/// The `T` of `-> impl Iterator<Item = T>`, if that's the return type's shape.
fn impl_iterator_item(ty: &Type) -> Option<Type> {
    let Type::ImplTrait(it) = ty else { return None };
    it.bounds.iter().find_map(|bound| {
        let TypeParamBound::Trait(tb) = bound else {
            return None;
        };
        let seg = tb.path.segments.last()?;
        if seg.ident != "Iterator" {
            return None;
        }
        assoc_type(&seg.arguments, "Item")
    })
}

/// The `T` of a `Callback<T>` parameter type, if that's its shape.
fn callback_item(ty: &Type) -> Option<Type> {
    let Type::Path(p) = ty else { return None };
    let seg = p.path.segments.last()?;
    if seg.ident != "Callback" {
        return None;
    }
    if let PathArguments::AngleBracketed(args) = &seg.arguments {
        if let Some(GenericArgument::Type(t)) = args.args.first() {
            return Some(t.clone());
        }
    }
    None
}

fn assoc_type(args: &PathArguments, name: &str) -> Option<Type> {
    let PathArguments::AngleBracketed(args) = args else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        GenericArgument::AssocType(a) if a.ident == name => Some(a.ty.clone()),
        _ => None,
    })
}

/// `&(a, b)` (args as a JSON array). `&[(); 0]` for none, so it serializes to `[]`.
fn args_tuple(idents: &[&Ident]) -> proc_macro2::TokenStream {
    match idents.len() {
        0 => quote!(&[(); 0]),
        1 => {
            let a = idents[0];
            quote!(&(#a,))
        }
        _ => quote!(&(#(#idents),*)),
    }
}

fn serve_arm(h: &Handler) -> proc_macro2::TokenStream {
    let op = &h.op;
    let name = &h.name;
    let idents: Vec<&Ident> = h.params.iter().map(|p| &p.name).collect();
    let reply = if h.stream_item.is_some() {
        quote!(rusm_rs::wire::reply_stream(&req, #name(#(#idents),*)))
    } else {
        quote!(rusm_rs::wire::reply_ok(&req, &#name(#(#idents),*)))
    };

    // Callback handlers need per-position arg extraction (a callback isn't a
    // deserializable value); the common case deserializes the whole args tuple.
    if h.has_callback() {
        let bindings = h.params.iter().enumerate().map(|(i, p)| {
            let pname = &p.name;
            if let Some(item) = &p.callback {
                quote! { let #pname = rusm_rs::wire::callback::<#item>(&req, #i); }
            } else {
                let ty = &p.ty;
                quote! {
                    let #pname: #ty = match rusm_rs::wire::arg(&req, #i) {
                        ::core::result::Result::Ok(v) => v,
                        ::core::result::Result::Err(e) => {
                            rusm_rs::wire::reply_err(&req, &e);
                            continue;
                        }
                    };
                }
            }
        });
        return quote! { #op => { #(#bindings)* #reply; } };
    }

    if h.params.is_empty() {
        return quote! { #op => #reply, };
    }
    let types = h.params.iter().map(|p| &p.ty);
    let (tuple_ty, binding) = if h.params.len() == 1 {
        let t = &h.params[0].ty;
        let a = idents[0];
        (quote!((#t,)), quote!((#a,)))
    } else {
        (quote!((#(#types),*)), quote!((#(#idents),*)))
    };
    quote! {
        #op => match req.args::<#tuple_ty>() {
            ::core::result::Result::Ok(#binding) => #reply,
            ::core::result::Result::Err(e) => rusm_rs::wire::reply_err(&req, &e),
        },
    }
}

fn client_method(h: &Handler) -> proc_macro2::TokenStream {
    let name = &h.name;
    let op = &h.op;

    // Callback methods take closures and build args per position (markers for
    // callbacks), routing the service's invocations back to the closures.
    if h.has_callback() {
        let params = h.params.iter().map(|p| {
            let pname = &p.name;
            match &p.callback {
                Some(item) => quote!(mut #pname: impl FnMut(#item) + 'static),
                None => {
                    let ty = &p.ty;
                    quote!(#pname: #ty)
                }
            }
        });
        let registrations = h.params.iter().filter(|p| p.callback.is_some()).map(|p| {
            let pname = &p.name;
            let cbvar = format_ident!("__cb_{}", pname);
            quote! {
                let #cbvar = rusm_rs::wire::register_callback(move |__v| {
                    #pname(rusm_rs::serde_json::from_value(__v).expect("callback arg"));
                });
            }
        });
        let arg_exprs = h.params.iter().map(|p| {
            let pname = &p.name;
            if p.callback.is_some() {
                let cbvar = format_ident!("__cb_{}", pname);
                quote!(rusm_rs::serde_json::json!({ "__cb": #cbvar }))
            } else {
                quote!(rusm_rs::serde_json::to_value(&#pname).expect("arg serializes"))
            }
        });
        let unregs = h.params.iter().filter(|p| p.callback.is_some()).map(|p| {
            let cbvar = format_ident!("__cb_{}", &p.name);
            quote!(rusm_rs::wire::unregister_callback(#cbvar);)
        });
        let ret = &h.ret;
        return quote! {
            pub fn #name(&self, #(#params),*)
                -> ::core::result::Result<#ret, ::std::string::String> {
                #(#registrations)*
                let __args = rusm_rs::serde_json::json!([ #(#arg_exprs),* ]);
                let __r = rusm_rs::wire::call_json(self.pid, #op, __args);
                #(#unregs)*
                __r
            }
        };
    }

    let idents: Vec<&Ident> = h.params.iter().map(|p| &p.name).collect();
    let types = h.params.iter().map(|p| &p.ty);
    let args = args_tuple(&idents);
    if let Some(item) = &h.stream_item {
        quote! {
            pub fn #name(&self #(, #idents: #types)*)
                -> impl ::core::iter::Iterator<Item = #item> {
                rusm_rs::wire::call_stream(self.pid, #op, #args)
            }
        }
    } else {
        let ret = &h.ret;
        quote! {
            pub fn #name(&self #(, #idents: #types)*)
                -> ::core::result::Result<#ret, ::std::string::String> {
                rusm_rs::wire::call(self.pid, #op, #args)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn bridge_refs_parses_validates_and_defaults_empty() {
        assert!(
            bridge_refs(quote!()).unwrap().is_empty(),
            "no attr → no bridges"
        );
        assert_eq!(
            bridge_refs(quote!(bridge = "weather:bridge/forecast@0.1.0")).unwrap(),
            ["weather:bridge/forecast@0.1.0"],
        );
        assert_eq!(
            bridge_refs(quote!(bridge = "a:b/c", bridge = "d:e/f")).unwrap(),
            ["a:b/c", "d:e/f"],
        );
        assert!(
            bridge_refs(quote!(foo = "x")).is_err(),
            "unknown key rejected"
        );
        assert!(
            bridge_refs(quote!(bridge = 3)).is_err(),
            "non-string rejected"
        );
    }

    #[test]
    fn bindings_switch_between_inline_and_the_generated_wit_world() {
        // No bridges → the dir-free inline `process` world.
        let inline = component_bindings(false).to_string();
        assert!(inline.contains("inline") && inline.contains("\"process\""));
        assert!(!inline.contains("generate_all"));
        // With bridges → the `rusm build`-generated `wit/` `handler` world, generate_all.
        let path = component_bindings(true).to_string();
        assert!(
            path.contains("\"wit\"")
                && path.contains("\"handler\"")
                && path.contains("generate_all")
        );
        // Either way the rusm:runtime interfaces are mapped to the SDK.
        assert!(inline.contains("rusm_rs :: rusm :: runtime :: actor"));
        assert!(path.contains("rusm_rs :: rusm :: runtime :: serve"));
    }

    #[test]
    fn reexports_map_a_versioned_kebab_ref_to_a_crate_path() {
        let re = bridge_reexports(&["acme:json-codec/encode@0.1.0".into()]).unwrap();
        assert_eq!(re.len(), 1);
        assert_eq!(
            re[0].to_string(),
            // version stripped; kebab → snake in the module path.
            "pub use __rusm_component :: acme :: json_codec :: encode ;",
        );
        assert!(bridge_reexports(&["malformed".into()]).is_err());
    }

    /// The vendored WIT must stay byte-identical to the canonical copy in `rusm-rs`,
    /// so a `#[rusm_rs::main]` component generates exactly the bindings `rusm-rs` does.
    /// If this fails, re-copy `crates/rusm-rs/wit/world.wit` over the vendored one.
    #[test]
    fn wit_in_sync() {
        assert_eq!(
            include_str!("../wit/world.wit"),
            include_str!("../../rusm-rs/wit/world.wit"),
            "vendored wit drifted from rusm-rs/wit/world.wit",
        );
    }
}
