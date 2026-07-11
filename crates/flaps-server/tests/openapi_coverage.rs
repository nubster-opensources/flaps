//! Bidirectional coverage guard between `build_router` and `docs/spec/openapi.json`.
//!
//! The router in `src/lib.rs` is the single source of truth for the HTTP surface.
//! This test extracts the exact set of `(METHOD, path)` pairs from the router via
//! AST parsing (`syn`), extracts the same shape from the `paths` object of the
//! `OpenAPI` contract, and asserts the two sets are identical.
//!
//! A route added to `build_router` without a matching contract entry (or the
//! reverse: a contract entry with no matching route) fails this test with a
//! message listing the exact symmetric difference.

use std::collections::BTreeSet;

use syn::visit::Visit;
use syn::{Expr, Lit};

/// One HTTP operation identified by its uppercase method and its literal path.
type Operation = (String, String);

/// HTTP method names recognised as `axum::routing` route builders.
const HTTP_METHODS: &[&str] = &["get", "post", "put", "delete", "patch", "options", "head"];

// ---------------------------------------------------------------------------
// Router side: AST extraction from `src/lib.rs`
// ---------------------------------------------------------------------------

/// Walks the router-building expression tree and collects `(METHOD, path)` pairs
/// from every `.route(path, METHOD(handler)...)` call.
#[derive(Default)]
struct RouteVisitor {
    routes: BTreeSet<Operation>,
}

impl RouteVisitor {
    /// Extracts every HTTP method builder used in `expr`, including chained
    /// forms such as `get(handler).put(other_handler)`.
    fn collect_methods(expr: &Expr, methods: &mut Vec<String>) {
        match expr {
            Expr::Call(call) => {
                if let Expr::Path(path) = call.func.as_ref() {
                    if let Some(segment) = path.path.segments.last() {
                        let name = segment.ident.to_string();
                        if HTTP_METHODS.contains(&name.as_str()) {
                            methods.push(name.to_uppercase());
                        }
                    }
                }
            }
            Expr::MethodCall(method_call) => {
                Self::collect_methods(&method_call.receiver, methods);
                let name = method_call.method.to_string();
                if HTTP_METHODS.contains(&name.as_str()) {
                    methods.push(name.to_uppercase());
                }
            }
            _ => {}
        }
    }
}

impl<'ast> Visit<'ast> for RouteVisitor {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if node.method == "route" {
            let mut args = node.args.iter();
            let path_arg = args.next();
            let route_arg = args.next();

            if let (Some(Expr::Lit(path_lit)), Some(route_expr)) = (path_arg, route_arg) {
                if let Lit::Str(path_str) = &path_lit.lit {
                    let path = path_str.value();
                    let mut methods = Vec::new();
                    Self::collect_methods(route_expr, &mut methods);
                    for method in methods {
                        self.routes.insert((method, path.clone()));
                    }
                }
            }
        }

        // Keep walking: `.route(...)` calls are nested inside a long method
        // chain, and the receiver of this call may hide earlier ones.
        syn::visit::visit_expr_method_call(self, node);
    }
}

/// Parses `src/lib.rs` and returns every `(METHOD, path)` pair registered on
/// the router built by `build_router`.
fn routes_from_code() -> BTreeSet<Operation> {
    let source = include_str!("../src/lib.rs");
    let file = syn::parse_file(source).expect("src/lib.rs must be valid Rust syntax");

    let mut visitor = RouteVisitor::default();
    visitor.visit_file(&file);
    visitor.routes
}

// ---------------------------------------------------------------------------
// Contract side: `paths` extraction from `docs/spec/openapi.json`
// ---------------------------------------------------------------------------

/// Parses `docs/spec/openapi.json` and returns every `(METHOD, path)` pair
/// declared under its top-level `paths` object.
fn operations_from_contract() -> BTreeSet<Operation> {
    let source = include_str!("../../../docs/spec/openapi.json");
    let contract: serde_json::Value =
        serde_json::from_str(source).expect("docs/spec/openapi.json must be valid JSON");

    let mut operations = BTreeSet::new();

    let Some(paths) = contract.get("paths").and_then(serde_json::Value::as_object) else {
        return operations;
    };

    for (path, item) in paths {
        let Some(item) = item.as_object() else {
            continue;
        };
        for (key, _operation) in item {
            if HTTP_METHODS.contains(&key.as_str()) {
                operations.insert((key.to_uppercase(), path.clone()));
            }
        }
    }

    operations
}

// ---------------------------------------------------------------------------
// The guard
// ---------------------------------------------------------------------------

/// Formats the symmetric difference between the router and the contract into
/// an actionable message: which routes are undocumented, which documented
/// operations do not exist as routes.
fn format_diff(routes: &BTreeSet<Operation>, operations: &BTreeSet<Operation>) -> String {
    use std::fmt::Write as _;

    let undocumented: Vec<&Operation> = routes.difference(operations).collect();
    let phantom: Vec<&Operation> = operations.difference(routes).collect();

    let mut message = String::new();
    if !undocumented.is_empty() {
        message.push_str("routes present in build_router but missing from openapi.json:\n");
        for (method, path) in &undocumented {
            let _ = writeln!(message, "  - {method} {path}");
        }
    }
    if !phantom.is_empty() {
        message.push_str("operations present in openapi.json but not routed in build_router:\n");
        for (method, path) in &phantom {
            let _ = writeln!(message, "  - {method} {path}");
        }
    }
    message
}

#[test]
fn openapi_contract_matches_build_router_exactly() {
    let routes = routes_from_code();
    let operations = operations_from_contract();

    assert_eq!(
        routes,
        operations,
        "openapi.json is out of sync with build_router:\n{}",
        format_diff(&routes, &operations)
    );
}

#[test]
fn build_router_exposes_the_expected_route_count() {
    // Locks the known route count (28 operations) so an accidental drop in
    // the AST extraction itself (e.g. a parsing regression) is caught even
    // if it happens to still match a stale contract.
    let routes = routes_from_code();
    assert_eq!(
        routes.len(),
        28,
        "expected exactly 28 (method, path) operations in build_router, found {}",
        routes.len()
    );
}
