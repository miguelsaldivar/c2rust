//! AST template substitution.
//!
//! This module provides functions for substituting `Bindings` into a template AST.  Placeholder
//! forms in the template AST are similar to patterns used in the `matcher` module:
//!
//!  * `__x`: An ident whose name is present in the `Bindings` will be replaced with the
//!    corresponding AST fragment.  (If the `Bindings` came from a `matcher` invocation, then most
//!    of these names will start with double underscores.)
//!
//!    This placeholder form only works if the name is present in the `Bindings` and the
//!    corresponding AST fragment is the same type as the current node.  Like in `matcher`, the
//!    substitution code tries to replace at multiple levels.  For example, if the placeholder AST
//!    is the expr `__x`, then the substitution code will first try to replace the entire `Expr`,
//!    but if this fails (because the `Bindings` have a non-`Expr` for the name `__x`), then it
//!    will continue on to try replacing the `Path` and finally just the `Ident`.
//!
//!    For itemlikes, a lone ident can't be used as a placeholder because it's not a valid
//!    itemlike.  Use a zero-argument macro invocation `__x!()` instead.

use syntax::ast::{Ident, Path, Expr, ExprKind, Pat, Ty, TyKind, Stmt, Item, ImplItem};
use syntax::ast::Mac;
use syntax::fold::{self, Folder};
use syntax::ptr::P;
use smallvec::SmallVec;

use ast_manip::Fold;
use ast_manip::util::{PatternSymbol, macro_name};
use command::CommandState;
use driver;
use matcher::Bindings;
use util::Lone;


// `st` and `cx` were previously used for `def!` substitution, which has been removed.  I expect
// they'll be needed again for future subst extensions, so I've left them in to reduce API churn.
#[allow(unused)]
struct SubstFolder<'a, 'tcx: 'a> {
    st: &'a CommandState,
    cx: &'a driver::Ctxt<'a, 'tcx>,
    bindings: &'a Bindings,
}

impl<'a, 'tcx> Folder for SubstFolder<'a, 'tcx> {
    fn fold_ident(&mut self, i: Ident) -> Ident {
        // The `Ident` case is a bit different from the others.  If `fold_stmt` finds a non-`Stmt`
        // in `self.bindings`, it can ignore the problem and hope `fold_expr` or `fold_ident` will
        // find an `Expr`/`Ident` for the symbol later on.  If `fold_ident` fails, there is no
        // lower-level construct to try.  So we report an error if a binding exists at this point
        // but is not an `Ident`.

        if let Some(sym) = i.pattern_symbol() {
            if let Some(ident) = self.bindings.get_ident(sym) {
                return ident.clone();
            } else if let Some(ty) = self.bindings.get_type(sym) {
                panic!("binding {:?} (of type {:?}) has wrong type for hole", sym, ty);
            }
            // Otherwise, fall through
        }
        fold::noop_fold_ident(i, self)
    }

    fn fold_path(&mut self, p: Path) -> Path {
        if let Some(path) = p.pattern_symbol().and_then(|sym| self.bindings.get_path(sym)) {
            path.clone()
        } else {
            fold::noop_fold_path(p, self)
        }
    }

    fn fold_expr(&mut self, e: P<Expr>) -> P<Expr> {
        if let Some(expr) = e.pattern_symbol().and_then(|sym| self.bindings.get_expr(sym)) {
            return expr.clone();
        }

        if let ExprKind::Mac(ref mac) = e.node {
            match &macro_name(&mac).as_str() as &str {
                _ => {},
            }
        }

        e.map(|e| fold::noop_fold_expr(e, self))
    }

    fn fold_pat(&mut self, p: P<Pat>) -> P<Pat> {
        if let Some(pat) = p.pattern_symbol().and_then(|sym| self.bindings.get_pat(sym)) {
            pat.clone()
        } else {
            fold::noop_fold_pat(p, self)
        }
    }

    fn fold_ty(&mut self, ty: P<Ty>) -> P<Ty> {
        if let Some(ty) = ty.pattern_symbol().and_then(|sym| self.bindings.get_ty(sym)) {
            return ty.clone();
        }

        if let TyKind::Mac(ref mac) = ty.node {
            match &macro_name(&mac).as_str() as &str {
                _ => {},
            }
        }

        fold::noop_fold_ty(ty, self)
    }

    fn fold_stmt(&mut self, s: Stmt) -> SmallVec<[Stmt; 1]> {
        if let Some(stmt) = s.pattern_symbol().and_then(|sym| self.bindings.get_stmt(sym)) {
            smallvec![stmt.clone()]
        } else if let Some(stmts) = s.pattern_symbol()
                .and_then(|sym| self.bindings.get_multi_stmt(sym)) {
            SmallVec::from_vec(stmts.clone())
        } else {
            fold::noop_fold_stmt(s, self)
        }
    }

    fn fold_item(&mut self, i: P<Item>) -> SmallVec<[P<Item>; 1]> {
        if let Some(item) = i.pattern_symbol().and_then(|sym| self.bindings.get_item(sym)) {
            smallvec![item.clone()]
        } else {
            fold::noop_fold_item(i, self)
        }
    }

    fn fold_mac(&mut self, mac: Mac) -> Mac {
        fold::noop_fold_mac(mac, self)
    }
}


pub trait Subst {
    fn subst(self, st: &CommandState, cx: &driver::Ctxt, bindings: &Bindings) -> Self;
}

macro_rules! subst_impl {
    ($ty:ty, $fold_func:ident) => {
        impl Subst for $ty {
            fn subst(self, st: &CommandState, cx: &driver::Ctxt, bindings: &Bindings) -> Self {
                let mut f = SubstFolder {
                    st: st,
                    cx: cx,
                    bindings: bindings,
                };
                let result = self.fold(&mut f);
                result.lone()
            }
        }
    };
}

macro_rules! multi_subst_impl {
    ($ty:ty, $fold_func:ident) => {
        impl Subst for Vec<$ty> {
            fn subst(self, st: &CommandState, cx: &driver::Ctxt, bindings: &Bindings) -> Self {
                let mut f = SubstFolder {
                    st: st,
                    cx: cx,
                    bindings: bindings,
                };
                let mut results = Vec::with_capacity(self.len());
                for x in self {
                    results.extend_from_slice(&x.fold(&mut f));
                }
                results
            }
        }
    };
}

subst_impl!(Ident, fold_ident);
subst_impl!(P<Expr>, fold_expr);
subst_impl!(P<Pat>, fold_pat);
subst_impl!(P<Ty>, fold_ty);
subst_impl!(Stmt, fold_stmt);
subst_impl!(P<Item>, fold_item);
subst_impl!(ImplItem, fold_impl_item);

multi_subst_impl!(Stmt, fold_stmt);
multi_subst_impl!(P<Item>, fold_item);
multi_subst_impl!(ImplItem, fold_impl_item);
