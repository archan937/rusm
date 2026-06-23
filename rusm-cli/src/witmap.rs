//! Mapping a custom bridge's WIT **value types** to Rust (for the generated js-runner glue)
//! and TypeScript (for the generated `.d.ts`), so a TS guest calls a bridge with arbitrary
//! value types — records, variants, enums, lists, options, results, tuples, primitives —
//! marshaled over `serde_json`, exactly as a Rust/Go guest does (closing the string-only gap).
//!
//! The glue deserializes each JS argument into the **owned** Rust type, then passes it to the
//! typed wit-bindgen import binding with the borrow that binding expects: wit-bindgen borrows
//! a param **iff its type contains heap data** (a `string`/`list`, transitively) — `&str`,
//! `&[T]`, `&Record`, `Option<&str>` — and takes plain types (enums, primitive records,
//! `option<u32>`) by value (verified against wit-bindgen 0.46). Results are always owned.
//!
//! Resources/handles/streams/futures aren't values and fail loudly; a few exotic *param*
//! shapes whose borrow form isn't simple (`option<list>`, `option<record>`, `tuple`/`result`
//! params) also fail loudly with the Rust/Go-guest workaround — never a silent miscompile.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use wit_parser::{Resolve, Type, TypeDefKind, TypeId, TypeOwner};

/// How the generated glue passes an owned local to the wit-bindgen binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Borrow {
    /// By value (`name`) — primitives, enums, and heap-free records/variants/options.
    Value,
    /// By shared reference (`&name`) — `string`, `list<_>`, and heap-bearing records/variants.
    Ref,
    /// `name.as_deref()` — `option<string>` (`Option<String>` → `Option<&str>`).
    AsDeref,
}

/// One bridge-function parameter, lowered for codegen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    /// The owned Rust type the glue deserializes into (e.g. `String`, `Vec<u8>`, the record path).
    pub owned_rust: String,
    /// How to pass it to the binding.
    pub borrow: Borrow,
    /// The TypeScript type for the `.d.ts`.
    pub ts: String,
    /// The Go type for this parameter — used in the generated `_runner.go` dispatch switch.
    /// Primitives map to their Go equivalents; complex types (records, variants, enums) map to
    /// `interface{}` (JSON round-trip; the user's host.go function accepts the matching type).
    pub go: String,
}

/// One custom bridge function the TS guest can call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Func {
    /// The wit-bindgen binding path **relative to the bindings module root**
    /// (`ns::pkg::iface::func`); the caller prepends the module the bindings live in.
    pub call_path: String,
    /// The bridge-relative function name (the JS method + the `__<bridge>__<func>` primitive).
    pub name: String,
    pub params: Vec<Param>,
    /// `None` for a `-> ()` function (the JS wrapper returns `undefined`).
    pub result_ts: Option<String>,
    /// The owned Rust type for the result (e.g. `String`, `Vec<u8>`). `None` for `-> ()`.
    /// Used by generated TS/Go delegation shims to `serde_json::from_slice` the reply.
    pub result_rust: Option<String>,
    /// The Go return type for the `_runner.go` dispatch. `None` for `-> ()`.
    pub result_go: Option<String>,
}

/// A bridge's full TS-callable surface: its functions + the TS type declarations (interfaces /
/// unions) the named types need, emitted once into the `.d.ts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Api {
    pub functions: Vec<Func>,
    /// type name → TS declaration (e.g. `Point` → `interface Point { x: number }`). `BTreeMap`
    /// for a stable, deduplicated emission order.
    pub ts_decls: BTreeMap<String, String>,
}

/// Lower a bridge's `bridge.wit` into its TS-callable [`Api`] — the typed functions + the TS
/// type declarations the named types need.
pub fn bridge_api(wit: &Path) -> Result<Api> {
    let mut resolve = Resolve::new();
    let (pkg_id, _) = resolve
        .push_path(wit)
        .with_context(|| format!("parsing bridge WIT {}", wit.display()))?;
    let pkg = &resolve.packages[pkg_id];
    let ns = ident(&pkg.name.namespace);
    let pkg_name = ident(&pkg.name.name);
    let mut ts_decls = BTreeMap::new();
    let mut functions = Vec::new();
    for (iface_name, &iface_id) in &pkg.interfaces {
        for (fn_name, function) in &resolve.interfaces[iface_id].functions {
            let mut params = Vec::new();
            for p in &function.params {
                let ty = lower(&resolve, &p.ty, &ns, &pkg_name, &mut ts_decls)?;
                let borrow = param_borrow(&resolve, &p.ty)?;
                let go = go_type(&resolve, &p.ty);
                params.push(Param {
                    name: ident(&p.name),
                    owned_rust: ty.rust,
                    borrow,
                    ts: ty.ts,
                    go,
                });
            }
            let (result_ts, result_rust, result_go) = match &function.result {
                None => (None, None, None),
                Some(t) => {
                    let lowered = lower(&resolve, t, &ns, &pkg_name, &mut ts_decls)?;
                    (Some(lowered.ts), Some(lowered.rust), Some(go_type(&resolve, t)))
                }
            };
            functions.push(Func {
                call_path: format!(
                    "{ns}::{pkg_name}::{}::{}",
                    ident(iface_name),
                    ident(fn_name)
                ),
                name: fn_name.clone(),
                params,
                result_ts,
                result_rust,
                result_go,
            });
        }
    }
    Ok(Api {
        functions,
        ts_decls,
    })
}

/// A WIT name as a Rust/JS identifier (kebab → snake), matching wit-bindgen's module/field
/// naming.
fn ident(name: &str) -> String {
    name.replace('-', "_")
}

/// A WIT name as a Rust type name (kebab → PascalCase), matching wit-bindgen.
fn pascal(name: &str) -> String {
    name.split('-')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect()
}

/// The owned-Rust + TypeScript forms of a type.
struct Lowered {
    rust: String,
    ts: String,
}

/// How wit-bindgen borrows a **parameter** of `ty`: by reference iff it holds heap data, with
/// `option<string>` as the one structural special case. The exotic borrow shapes
/// (`option<list>`, `option<record>`, `tuple`/`result` params) fail loudly.
fn param_borrow(resolve: &Resolve, ty: &Type) -> Result<Borrow> {
    Ok(match ty {
        Type::Id(id) => match &resolve.types[*id].kind {
            TypeDefKind::Option(inner) => match inner {
                Type::String => Borrow::AsDeref,
                _ if !contains_heap(resolve, inner) => Borrow::Value,
                _ => bail!(
                    "TS bridge: `option<…>` of a heap type (string excepted) isn't supported as a \
                     parameter yet — pass it from a Rust or Go guest, or flatten the option"
                ),
            },
            TypeDefKind::Type(t) => return param_borrow(resolve, t),
            _ if contains_heap(resolve, ty) => Borrow::Ref,
            _ => Borrow::Value,
        },
        Type::String => Borrow::Ref,
        _ => Borrow::Value, // primitives, char, bool
    })
}

/// Whether `ty` carries heap data (a `string` or `list`, transitively) — the condition under
/// which wit-bindgen borrows it.
fn contains_heap(resolve: &Resolve, ty: &Type) -> bool {
    match ty {
        Type::String => true,
        Type::Id(id) => match &resolve.types[*id].kind {
            TypeDefKind::List(_) => true,
            TypeDefKind::Option(t) | TypeDefKind::Type(t) => contains_heap(resolve, t),
            TypeDefKind::Result(r) => {
                r.ok.map(|t| contains_heap(resolve, &t)).unwrap_or(false)
                    || r.err.map(|t| contains_heap(resolve, &t)).unwrap_or(false)
            }
            TypeDefKind::Tuple(t) => t.types.iter().any(|t| contains_heap(resolve, t)),
            TypeDefKind::Record(r) => r.fields.iter().any(|f| contains_heap(resolve, &f.ty)),
            TypeDefKind::Variant(v) => v
                .cases
                .iter()
                .any(|c| c.ty.map(|t| contains_heap(resolve, &t)).unwrap_or(false)),
            // enum / flags carry no payload.
            _ => false,
        },
        _ => false,
    }
}

/// Lower a value type to its owned-Rust + TS forms, registering any named type's TS
/// declaration in `decls`. Errors loudly on non-value types (resources, streams, …).
fn lower(
    resolve: &Resolve,
    ty: &Type,
    ns: &str,
    pkg: &str,
    decls: &mut BTreeMap<String, String>,
) -> Result<Lowered> {
    let simple = |rust: &str, ts: &str| Lowered {
        rust: rust.into(),
        ts: ts.into(),
    };
    Ok(match ty {
        Type::Bool => simple("bool", "boolean"),
        Type::U8 => simple("u8", "number"),
        Type::U16 => simple("u16", "number"),
        Type::U32 => simple("u32", "number"),
        Type::U64 => simple("u64", "number"),
        Type::S8 => simple("i8", "number"),
        Type::S16 => simple("i16", "number"),
        Type::S32 => simple("i32", "number"),
        Type::S64 => simple("i64", "number"),
        Type::F32 => simple("f32", "number"),
        Type::F64 => simple("f64", "number"),
        Type::Char => simple("char", "string"),
        Type::String => simple("String", "string"),
        Type::Id(id) => lower_def(resolve, *id, ns, pkg, decls)?,
        other => bail!("TS bridge: unsupported WIT type {other:?}"),
    })
}

/// Lower a named/compound type (the `Type::Id` arm of [`lower`]).
fn lower_def(
    resolve: &Resolve,
    id: TypeId,
    ns: &str,
    pkg: &str,
    decls: &mut BTreeMap<String, String>,
) -> Result<Lowered> {
    let def = &resolve.types[id];
    match &def.kind {
        TypeDefKind::Type(t) => lower(resolve, t, ns, pkg, decls),
        TypeDefKind::List(t) => {
            let inner = lower(resolve, t, ns, pkg, decls)?;
            Ok(Lowered {
                rust: format!("Vec<{}>", inner.rust),
                ts: format!("{}[]", ts_paren(&inner.ts)),
            })
        }
        TypeDefKind::Option(t) => {
            let inner = lower(resolve, t, ns, pkg, decls)?;
            Ok(Lowered {
                rust: format!("Option<{}>", inner.rust),
                ts: format!("{} | null", ts_paren(&inner.ts)),
            })
        }
        TypeDefKind::Result(r) => {
            let ok = opt_lower(resolve, r.ok, ns, pkg, decls)?;
            let err = opt_lower(resolve, r.err, ns, pkg, decls)?;
            // serde's externally-tagged `Result`: `{"Ok": …}` / `{"Err": …}`.
            Ok(Lowered {
                rust: format!("Result<{}, {}>", ok.rust, err.rust),
                ts: format!("{{ Ok: {} }} | {{ Err: {} }}", ok.ts, err.ts),
            })
        }
        TypeDefKind::Tuple(t) => {
            let parts: Vec<Lowered> = t
                .types
                .iter()
                .map(|t| lower(resolve, t, ns, pkg, decls))
                .collect::<Result<_>>()?;
            Ok(Lowered {
                rust: format!(
                    "({})",
                    parts
                        .iter()
                        .map(|p| p.rust.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                ts: format!(
                    "[{}]",
                    parts
                        .iter()
                        .map(|p| p.ts.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            })
        }
        TypeDefKind::Record(rec) => {
            let name = type_name(def)?;
            let mut fields = String::new();
            for f in &rec.fields {
                let t = lower(resolve, &f.ty, ns, pkg, decls)?;
                fields.push_str(&format!("  {}: {};\n", ident(&f.name), t.ts));
            }
            decls
                .entry(name.clone())
                .or_insert_with(|| format!("interface {name} {{\n{fields}}}"));
            Ok(named(resolve, id, ns, pkg, &name))
        }
        TypeDefKind::Enum(en) => {
            let name = type_name(def)?;
            let cases = en
                .cases
                .iter()
                .map(|c| format!("\"{}\"", pascal(&c.name)))
                .collect::<Vec<_>>()
                .join(" | ");
            decls
                .entry(name.clone())
                .or_insert_with(|| format!("type {name} = {cases};"));
            Ok(named(resolve, id, ns, pkg, &name))
        }
        TypeDefKind::Variant(var) => {
            let name = type_name(def)?;
            let mut cases = Vec::new();
            for c in &var.cases {
                let tag = pascal(&c.name);
                cases.push(match c.ty {
                    Some(t) => format!("{{ {tag}: {} }}", lower(resolve, &t, ns, pkg, decls)?.ts),
                    None => format!("\"{tag}\""),
                });
            }
            decls
                .entry(name.clone())
                .or_insert_with(|| format!("type {name} = {};", cases.join(" | ")));
            Ok(named(resolve, id, ns, pkg, &name))
        }
        other => bail!(
            "TS bridge: WIT type kind {other:?} isn't a value type — call this bridge from a \
             Rust or Go guest"
        ),
    }
}

/// The Rust path (relative to the bindings module root) + TS reference for a named type
/// defined in the bridge's package.
fn named(resolve: &Resolve, id: TypeId, ns: &str, pkg: &str, ts_name: &str) -> Lowered {
    let iface = match resolve.types[id].owner {
        TypeOwner::Interface(i) => resolve.interfaces[i].name.clone().unwrap_or_default(),
        _ => String::new(),
    };
    Lowered {
        rust: format!("{ns}::{pkg}::{}::{ts_name}", ident(&iface)),
        ts: ts_name.to_string(),
    }
}

/// Lower an optional payload type (a `result`'s ok/err), defaulting `None` to unit.
fn opt_lower(
    resolve: &Resolve,
    ty: Option<Type>,
    ns: &str,
    pkg: &str,
    decls: &mut BTreeMap<String, String>,
) -> Result<Lowered> {
    match ty {
        Some(t) => lower(resolve, &t, ns, pkg, decls),
        None => Ok(Lowered {
            rust: "()".into(),
            ts: "null".into(),
        }),
    }
}

/// A named type's PascalCase Rust/TS name.
fn type_name(def: &wit_parser::TypeDef) -> Result<String> {
    def.name
        .as_deref()
        .map(pascal)
        .ok_or_else(|| anyhow::anyhow!("TS bridge: anonymous named type isn't supported"))
}

/// Parenthesize a TS type if needed before `[]`/`|` (a union element).
fn ts_paren(ts: &str) -> String {
    if ts.contains(' ') {
        format!("({ts})")
    } else {
        ts.to_string()
    }
}

/// Map a WIT value type to its Go form for `_runner.go` dispatch deserialization.
/// Primitives map exactly; complex types (records, enums, variants, results, tuples) map to
/// `interface{}` — the user's `host.go` function accepts the JSON-decoded value and may
/// type-assert as needed. Lists recurse; options become Go pointers for primitive-containing
/// options (matching Go's idiomatic nil pointer as "absent").
pub(crate) fn go_type(resolve: &Resolve, ty: &Type) -> String {
    match ty {
        Type::Bool => "bool".into(),
        Type::U8 => "uint8".into(),
        Type::U16 => "uint16".into(),
        Type::U32 => "uint32".into(),
        Type::U64 => "uint64".into(),
        Type::S8 => "int8".into(),
        Type::S16 => "int16".into(),
        Type::S32 => "int32".into(),
        Type::S64 => "int64".into(),
        Type::F32 => "float32".into(),
        Type::F64 => "float64".into(),
        Type::Char => "rune".into(),
        Type::String => "string".into(),
        Type::Id(id) => go_type_def(resolve, *id),
        _ => "interface{}".into(),
    }
}

fn go_type_def(resolve: &Resolve, id: TypeId) -> String {
    match &resolve.types[id].kind {
        TypeDefKind::Type(t) => go_type(resolve, t),
        TypeDefKind::List(t) => format!("[]{}", go_type(resolve, t)),
        TypeDefKind::Option(inner) => {
            // Pointer-ify primitive and string options (Go idiom for optional scalar);
            // complex inner types use interface{} (pointer-to-complex is rare in JSON Go).
            match inner {
                Type::Bool | Type::U8 | Type::U16 | Type::U32 | Type::U64
                | Type::S8 | Type::S16 | Type::S32 | Type::S64
                | Type::F32 | Type::F64 | Type::Char | Type::String => {
                    format!("*{}", go_type(resolve, inner))
                }
                _ => "interface{}".into(),
            }
        }
        // Records, enums, variants, results, tuples → interface{} (JSON round-trip).
        _ => "interface{}".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lower a one-interface bridge from inline WIT (written to a temp file) into its [`Api`].
    fn api(body: &str) -> Result<Api> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        body.hash(&mut h);
        let dir = std::env::temp_dir().join(format!(
            "rusm-witmap-{}-{}",
            std::process::id(),
            // A content hash keeps concurrent cases on distinct files without RNG.
            h.finish()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let wit = dir.join("bridge.wit");
        std::fs::write(
            &wit,
            format!("package demo:pkg@0.1.0;\ninterface api {{\n{body}\n}}\n"),
        )
        .unwrap();
        bridge_api(&wit)
    }

    /// The single function of a single-function bridge.
    fn only_fn(body: &str) -> Func {
        let mut api = api(body).unwrap();
        assert_eq!(api.functions.len(), 1);
        api.functions.pop().unwrap()
    }

    #[test]
    fn primitives_are_passed_by_value_owned() {
        let f = only_fn("f: func(n: u32, ok: bool, r: f64) -> s64;");
        let kinds: Vec<_> = f
            .params
            .iter()
            .map(|p| (p.owned_rust.as_str(), &p.borrow, p.ts.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("u32", &Borrow::Value, "number"),
                ("bool", &Borrow::Value, "boolean"),
                ("f64", &Borrow::Value, "number"),
            ]
        );
        assert_eq!(f.result_ts.as_deref(), Some("number"));
        assert_eq!(f.call_path, "demo::pkg::api::f");
    }

    #[test]
    fn strings_and_lists_borrow_option_string_as_derefs() {
        let f = only_fn("f: func(s: string, xs: list<u8>, maybe: option<string>);");
        let p: Vec<_> = f
            .params
            .iter()
            .map(|p| (p.owned_rust.as_str(), &p.borrow, p.ts.as_str()))
            .collect();
        assert_eq!(
            p,
            [
                ("String", &Borrow::Ref, "string"),
                ("Vec<u8>", &Borrow::Ref, "number[]"),
                ("Option<String>", &Borrow::AsDeref, "string | null"),
            ]
        );
        // No result → JS wrapper returns undefined.
        assert_eq!(f.result_ts, None);
    }

    #[test]
    fn option_of_plain_is_by_value() {
        let f = only_fn("f: func(n: option<u32>);");
        assert_eq!(f.params[0].owned_rust, "Option<u32>");
        assert_eq!(f.params[0].borrow, Borrow::Value);
        assert_eq!(f.params[0].ts, "number | null");
    }

    #[test]
    fn record_with_heap_borrows_and_emits_a_ts_interface() {
        let api = api("record point { x: u32, label: string }\n\
             f: func(p: point) -> point;")
        .unwrap();
        let f = &api.functions[0];
        assert_eq!(f.params[0].owned_rust, "demo::pkg::api::Point");
        assert_eq!(f.params[0].borrow, Borrow::Ref); // contains a string
        assert_eq!(f.params[0].ts, "Point");
        assert_eq!(f.result_ts.as_deref(), Some("Point"));
        assert_eq!(
            api.ts_decls.get("Point").unwrap(),
            "interface Point {\n  x: number;\n  label: string;\n}"
        );
    }

    #[test]
    fn heap_free_record_is_by_value() {
        let api = api("record dim { w: u32, h: u32 }\nf: func(d: dim);").unwrap();
        assert_eq!(api.functions[0].params[0].borrow, Borrow::Value);
    }

    #[test]
    fn enum_lowers_to_a_string_union_by_value() {
        let api = api("enum color { red, sea-green }\n\
             f: func(c: color) -> color;")
        .unwrap();
        assert_eq!(api.functions[0].params[0].borrow, Borrow::Value);
        assert_eq!(api.functions[0].params[0].ts, "Color");
        assert_eq!(
            api.ts_decls.get("Color").unwrap(),
            "type Color = \"Red\" | \"SeaGreen\";"
        );
    }

    #[test]
    fn variant_lowers_to_a_tagged_union() {
        let api = api("variant shape { circle(f64), point, named(string) }\n\
             f: func(s: shape);")
        .unwrap();
        // A payload-bearing string case makes the whole variant heap → borrowed.
        assert_eq!(api.functions[0].params[0].borrow, Borrow::Ref);
        assert_eq!(
            api.ts_decls.get("Shape").unwrap(),
            "type Shape = { Circle: number } | \"Point\" | { Named: string };"
        );
    }

    #[test]
    fn result_and_tuple_results_lower_to_serde_shapes() {
        let f = only_fn("f: func() -> result<string, u32>;");
        assert_eq!(
            f.result_ts.as_deref(),
            Some("{ Ok: string } | { Err: number }")
        );
        let g = only_fn("g: func() -> tuple<u32, string>;");
        assert_eq!(g.result_ts.as_deref(), Some("[number, string]"));
        // A no-payload result → unit on both arms.
        let h = only_fn("h: func() -> result;");
        assert_eq!(h.result_ts.as_deref(), Some("{ Ok: null } | { Err: null }"));
    }

    #[test]
    fn list_of_named_type_nests_and_registers_the_element_decl() {
        let api = api("record item { name: string }\n\
             f: func() -> list<item>;")
        .unwrap();
        assert_eq!(api.functions[0].result_ts.as_deref(), Some("Item[]"));
        assert!(api.ts_decls.contains_key("Item"));
    }

    #[test]
    fn exotic_option_params_fail_loudly_with_the_guest_workaround() {
        let err = api("f: func(x: option<list<u8>>);")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("option") && err.contains("Rust or Go guest"),
            "{err}"
        );
    }

    #[test]
    fn resources_are_not_values_and_fail_loudly() {
        let err = api("resource conn;\nf: func() -> conn;")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Rust or Go guest"), "{err}");
    }

    #[test]
    fn every_primitive_lowers_to_its_rust_and_ts_form() {
        let f = only_fn(
            "f: func(a: u8, b: u16, c: u32, d: u64, e: s8, g: s16, h: s32, i: s64, \
             j: f32, k: f64, l: bool, m: char);",
        );
        let rust: Vec<&str> = f.params.iter().map(|p| p.owned_rust.as_str()).collect();
        assert_eq!(
            rust,
            ["u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64", "bool", "char"]
        );
        // Every numeric is `number`; bool is `boolean`; char is `string`.
        let ts: Vec<&str> = f.params.iter().map(|p| p.ts.as_str()).collect();
        assert_eq!(ts[10], "boolean");
        assert_eq!(ts[11], "string");
        assert!(ts[..10].iter().all(|t| *t == "number"));
    }

    #[test]
    fn type_aliases_and_compound_fields_resolve_through_heap_detection() {
        // A type-alias param recurses to its target (here a plain `u32` → by value); a record
        // whose single field is a tuple / result / option / alias drives each `contains_heap`
        // arm (the field is the only one, so nothing short-circuits it).
        let api = api("type nid = u32;\n\
             type sid = string;\n\
             record rtuple { v: tuple<u32, string> }\n\
             record rresult { v: result<u32, string> }\n\
             record roption { v: option<string> }\n\
             record ralias { v: sid }\n\
             falias-param: func(x: nid);\n\
             ftuple: func(r: rtuple);\n\
             fresult: func(r: rresult);\n\
             foption: func(r: roption);\n\
             falias: func(r: ralias);\n\
             flistopt: func() -> list<option<u32>>;")
        .unwrap();
        let by_name = |n: &str| api.functions.iter().find(|f| f.name == n).unwrap().clone();
        // alias param → resolves to u32, by value.
        let alias = by_name("falias-param");
        assert_eq!(alias.params[0].owned_rust, "u32");
        assert_eq!(alias.params[0].borrow, Borrow::Value);
        // each heap-bearing compound field makes its record borrow by reference.
        for n in ["ftuple", "fresult", "foption", "falias"] {
            assert_eq!(by_name(n).params[0].borrow, Borrow::Ref, "{n}");
        }
        // a union list element is parenthesized before `[]`.
        assert_eq!(
            by_name("flistopt").result_ts.as_deref(),
            Some("(number | null)[]")
        );
    }
}
