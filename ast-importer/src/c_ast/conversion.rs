use std::collections::HashMap;
use std::vec::Vec;
use c_ast::*;
use clang_ast::*;


/// Possible node types
pub type NodeType = u16;

mod node_types {
    pub const FUNC_TYPE  : super::NodeType = 0b0000000001;
    pub const OTHER_TYPE : super::NodeType = 0b0000000010;
    pub const TYPE       : super::NodeType = FUNC_TYPE | OTHER_TYPE;

    pub const EXPR       : super::NodeType = 0b0000000100;

    pub const FIELD_DECL : super::NodeType = 0b0000001000;
    pub const VAR_DECL   : super::NodeType = 0b0000010000;
    pub const RECORD_DECL: super::NodeType = 0b0000100000;
    pub const TYPDEF_DECL: super::NodeType = 0b0001000000;
    pub const OTHER_DECL : super::NodeType = 0b0010000000;
    pub const DECL       : super::NodeType = FIELD_DECL | VAR_DECL | RECORD_DECL | TYPDEF_DECL | OTHER_DECL;

    pub const LABEL_STMT : super::NodeType = 0b0100000000;
    pub const OTHER_STMT : super::NodeType = 0b1000000000;
    pub const STMT       : super::NodeType = LABEL_STMT | OTHER_STMT;

    pub const ANYTHING   : super::NodeType = TYPE | EXPR | DECL | STMT;

    // TODO
}

type ClangId = u64;
type NewId = u64;

/// Correspondance between old/new IDs.
///
/// We need to re-ID nodes since the mapping from Clang's AST to ours is not one-to-one. Sometimes
/// we need to add nodes (such as 'Semi' nodes to make the lifting of expressions into statements
/// explicit), sometimes we need to collapse (such as inlining 'FieldDecl' into the 'StructDecl').
pub struct IdMapper {
    new_id_source: NewId,
    old_to_new: HashMap<ClangId, NewId>,
    new_to_old: HashMap<NewId, ClangId>,
}

impl IdMapper {
    pub fn new() -> IdMapper {
        IdMapper {
            new_id_source: 0,
            old_to_new: HashMap::new(),
            new_to_old: HashMap::new(),
        }
    }

    /// Create a fresh NEW_ID not corresponding to a CLANG_ID
    fn fresh_id(&mut self) -> NewId {
        self.new_id_source += 1;
        self.new_id_source
    }

    /// Lookup the NEW_ID corresponding to a CLANG_ID
    pub fn get_new(&mut self, old_id: ClangId) -> Option<NewId> {
        self.old_to_new.get(&old_id).map(|o| *o)
    }

    /// Lookup (or create if not a found) a NEW_ID corresponding to a CLANG_ID
    pub fn get_or_create_new(&mut self, old_id: ClangId) -> NewId {
        match self.get_new(old_id) {
            Some(new_id) => new_id,
            None => {
                let new_id = self.fresh_id();
                self.old_to_new.insert(old_id, new_id);
                new_id
            }
        }
    }

    /// Lookup the CLANG_ID corresponding to a NEW_ID
    pub fn get_old(&mut self, new_id: NewId) -> Option<ClangId> {
        self.new_to_old.get(&new_id).map(|n| *n)
    }

    /// If the `old_id` is already present, map the `other_old_id` to point to the same `NewID`.
    pub fn merge_old(&mut self, old_id: ClangId, other_old_id: ClangId) -> Option<NewId> {
        self.get_new(old_id)
            .map(|new_id| {
                self.old_to_new.insert(other_old_id, new_id);
                new_id
            })
    }
}

/// Transfer location information off of an `AstNode` and onto something that is `Located`
fn located<T>(node: &AstNode, t: T) -> Located<T> {
    Located {
        loc: Some(SrcLoc { line: node.line, column: node.column, fileid: node.fileid }),
        kind: t
    }
}

/// Wrap something into a `Located` node without any location information
fn not_located<T>(t: T) -> Located<T> {
    Located {
        loc: None,
        kind: t
    }
}

/// Extract the qualifiers off of a `TypeNode`
fn qualifiers(ty_node: &TypeNode) -> Qualifiers {
    Qualifiers {
        is_const: ty_node.constant,
        is_restrict: false,
        is_volatile: false,
    }
}

/// This stores the information needed to convert an `AstContext` into a `TypedAstContext`.
pub struct ConversionContext {

    /// Keeps track of the mapping between old and new IDs
    pub id_mapper: IdMapper,

    /// Keep track of new nodes already processed and their types
    processed_nodes: HashMap<NewId, NodeType>,

    /// Stack of nodes to visit, and the types we expect to see out of them
    visit_as: Vec<(ClangId, NodeType)>,

    /// Typed context we are building up during the conversion
    pub typed_context: TypedAstContext,
}

impl ConversionContext {

    /// Create a new 'ConversionContext' seeded with top-level nodes from an 'AstContext'.
    pub fn new(untyped_context: &AstContext) -> ConversionContext {
        // This starts out as all of the top-level nodes, which we expect to be 'DECL's
        let mut visit_as: Vec<(ClangId, NodeType)> = Vec::new();
        for top_node in untyped_context.top_nodes.iter() {
            if untyped_context.ast_nodes.contains_key(&top_node) {
                visit_as.push((*top_node, node_types::DECL));
            }
        }

        ConversionContext {
            id_mapper: IdMapper::new(),
            processed_nodes: HashMap::new(),
            visit_as,
            typed_context: TypedAstContext::new(),
        }
    }

    /// Records the fact that we will need to visit a Clang node and the type we want it to have.
    ///
    /// Returns the new ID that identifies this new node.
    fn visit_node_type(&mut self, node_id: &ClangId, node_ty: NodeType) -> NewId {
        self.visit_as.push((*node_id, node_ty));
        self.id_mapper.get_or_create_new(*node_id)
    }

    /// Like `visit_node_type`, but specifically for type nodes
    fn visit_type(&mut self, node_id: &ClangId) -> CTypeId {
        CTypeId(self.visit_node_type(node_id, node_types::TYPE))
    }

    /// Like `visit_node_type`, but specifically for statement nodes
    fn visit_stmt(&mut self, node_id: &ClangId) -> CStmtId {
        CStmtId(self.visit_node_type(node_id, node_types::STMT))
    }

    /// Like `visit_node_type`, but specifically for expression nodes
    fn visit_expr(&mut self, node_id: &ClangId) -> CExprId {
        CExprId(self.visit_node_type(node_id, node_types::EXPR))
    }

    /// Like `visit_node_type`, but specifically for declaration nodes
    fn visit_decl(&mut self, node_id: &ClangId) -> CDeclId {
        CDeclId(self.visit_node_type(node_id, node_types::DECL))
    }

    /// Add a `CType`node into the `TypedAstContext`
    fn add_type(&mut self, id: NewId, typ: CType) -> () {
        self.typed_context.c_types.insert(CTypeId(id), typ);
    }

    /// Add a `CStmt` node into the `TypedAstContext`
    fn add_stmt(&mut self, id: NewId, stmt: CStmt) -> () {
        self.typed_context.c_stmts.insert(CStmtId(id), stmt);
    }

    /// Add a `CExpr` node into the `TypedAstContext`
    fn add_expr(&mut self, id: NewId, expr: CExpr) -> () {
        self.typed_context.c_exprs.insert(CExprId(id), expr);
    }

    /// Add a `CDecl` node into the `TypedAstContext`
    fn add_decl(&mut self, id: NewId, decl: CDecl) -> () {
        self.typed_context.c_decls.insert(CDeclId(id), decl);
    }

    /// Clang has `Expression <: Statement`, but we want to make that explicit via the
    /// `CStmtKind::Expr` statement constructor. This function automatically converts expressions
    /// into statements depending on the expected type argument.
    fn expr_possibly_as_stmt(
        &mut self,
        expected_ty: NodeType, // Should be one of `EXPR` or `STMT`
        new_id: NewId,
        node: &AstNode,
        expr: CExprKind,
    ) -> () {
        if expected_ty & node_types::STMT != 0 {
            // This is going to be an extra node not present in the Clang AST
            let new_expr_id = self.id_mapper.fresh_id();
            self.add_expr(new_expr_id, located(node, expr));
            self.processed_nodes.insert(new_expr_id, node_types::EXPR);

            // We wrap the expression in a STMT
            let semi_stmt = CStmtKind::Expr(CExprId(new_expr_id));
            self.add_stmt(new_id, located(node, semi_stmt));
            self.processed_nodes.insert(new_id, node_types::STMT);
        } else if expected_ty & node_types::EXPR != 0 {
            // No special work to do...
            self.add_expr(new_id, located(node, expr));
            self.processed_nodes.insert(new_id, node_types::EXPR);
        } else {
            panic!("'expr_possibly_as_stmt' expects 'expected_ty' to be either 'EXPR' or 'STMT'");
        }
    }

    /// Convert the contents of an `AstContext`, starting from the top-level declarations passed
    /// into the `ConversionContext` on creation.
    ///
    /// This populates the `typed_context` of the `ConversionContext` it is called on.
    pub fn convert(&mut self, untyped_context: &AstContext) -> () {

        // Continue popping Clang nodes off of the stack of nodes we have promised to visit
        while let Some((node_id, expected_ty)) = self.visit_as.pop() {

            // Check if we've already processed this node. If so, ascertain that it has the right
            // type.
            if let Some(ty) = self.id_mapper.get_new(node_id).and_then(|new_id| self.processed_nodes.get(&new_id)) {
                if ty & expected_ty != 0 {
                    continue;
                }
                panic!("Expected {} to be a node of type {}, not {}", &node_id, expected_ty, ty);
            }

            // Create a `NewId` for this node
            let new_id = self.id_mapper.get_or_create_new(node_id);

            // If the node is top-level, add it as such to the new context
            if untyped_context.top_nodes.contains(&node_id) {
                self.typed_context.c_decls_top.insert(CDeclId(new_id));
            }

            self.visit_node(untyped_context, node_id, new_id, expected_ty)
        }
    }


    /// Visit one node.
    fn visit_node(
        &mut self,
        untyped_context: &AstContext,
        node_id: ClangId,                 // Clang ID of node to visit
        new_id: NewId,                    // New ID of node to visit
        expected_ty: NodeType             // Expected type of node to visit
    ) -> () {
        use self::node_types::*;

        if expected_ty & TYPE != 0 {

            // Convert the node
            let ty_node: &TypeNode = untyped_context.type_nodes
                .get(&node_id)
                .expect("Could not find type node");

            match ty_node.tag {
                TypeTag::TagBool if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::Bool));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagVoid if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::Void));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagChar if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::Char));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagInt if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::Int));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagShort if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::Short));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagLong if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::Long));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagLongLong if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::LongLong));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagUInt if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::UInt));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagUChar if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::UChar));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagSChar if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::SChar));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagUShort if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::UShort));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagULong if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::ULong));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagULongLong if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::ULongLong));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagDouble if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::Double));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagLongDouble if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::LongDouble));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagFloat if expected_ty & OTHER_TYPE != 0 => {
                    self.add_type(new_id, not_located(CTypeKind::Float));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagPointer if expected_ty & OTHER_TYPE != 0 => {
                    let pointed = expect_u64(&ty_node.extras[0])
                        .expect("Pointer child not found");
                    let pointed_new = self.visit_type( &pointed);

                    let pointed_ty = CQualTypeId {
                        qualifiers: qualifiers(ty_node),
                        ctype: pointed_new
                    };
                    let pointer_ty = CTypeKind::Pointer(pointed_ty);
                    self.add_type(new_id, not_located(pointer_ty));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagRecordType if expected_ty & OTHER_TYPE != 0 => {
                    let decl = expect_u64(&ty_node.extras[0])
                        .expect("Record decl not found");
                    let decl_new = CDeclId(self.visit_node_type(&decl, RECORD_DECL));

                    let record_ty = CTypeKind::Record(decl_new);
                    self.add_type(new_id, not_located(record_ty));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagFunctionType if expected_ty & FUNC_TYPE != 0 => {
                    let mut arguments: Vec<CQualTypeId> = expect_array(&ty_node.extras[0])
                        .expect("Function type expects array argument")
                        .iter()
                        .map(|cbor| {
                            let ty_node_id = expect_u64(cbor).expect("Bad function type child id");
                            let ty_node = untyped_context.type_nodes
                                .get(&ty_node_id)
                                .expect("Function type child not found");

                            let ty_node_new_id = self.visit_type( &ty_node_id);

                            CQualTypeId { qualifiers: qualifiers(ty_node), ctype: ty_node_new_id }
                        })
                        .collect();
                    let ret = arguments.remove(0);
                    let function_ty = CTypeKind::Function(ret, arguments);
                    self.add_type(new_id, not_located(function_ty));
                    self.processed_nodes.insert(new_id, FUNC_TYPE);
                }

                TypeTag::TagTypeOfType if expected_ty & OTHER_TYPE != 0 => {
                    let type_of_old = expect_u64(&ty_node.extras[0]).expect("Type of (type) child not found");
                    let type_of = self.visit_type(&type_of_old);

                    let type_of_ty = CTypeKind::TypeOf(type_of);
                    self.add_type(new_id, not_located(type_of_ty));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagTypedefType if expected_ty & OTHER_TYPE != 0 => {
                    let decl = expect_u64(&ty_node.extras[0])
                        .expect("Typedef decl not found");
                    let decl_new = CDeclId(self.visit_node_type(&decl, TYPDEF_DECL));

                    let typedef_ty = CTypeKind::Typedef(decl_new);
                    self.add_type(new_id, not_located(typedef_ty));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagDecayedType if expected_ty & OTHER_TYPE != 0 => {
                    let decayed_id = expect_u64(&ty_node.extras[0]).expect("Decayed type child not found");
                    let decayed = self.visit_type(&decayed_id);

                    let decayed_ty = CTypeKind::Decayed(decayed);
                    self.add_type(new_id, not_located(decayed_ty));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                TypeTag::TagElaboratedType if expected_ty & OTHER_TYPE != 0 => {
                    let elaborated_id = expect_u64(&ty_node.extras[0]).expect("Elaborated type child not found");
                    let elaborated = self.visit_type(&elaborated_id);

                    let elaborated_ty = CTypeKind::Elaborated(elaborated);
                    self.add_type(new_id, not_located(elaborated_ty));
                    self.processed_nodes.insert(new_id, OTHER_TYPE);
                }

                t => panic!("Type conversion not implemented for {:?}", t),
            }

        } else {
            // Convert the node
            let node: &AstNode = untyped_context.ast_nodes
                .get(&node_id)
                .expect(format!("Could not find ast node {}", node_id).as_ref());

            match node.tag {
                // Statements

                ASTEntryTag::TagCompoundStmt if expected_ty & OTHER_STMT != 0 => {
                    let constituent_stmts: Vec<CStmtId> = node.children
                        .iter()
                        .map(|id| {
                            let arg_id = id.expect("Compound stmt child not found");
                            self.visit_stmt(&arg_id)
                        })
                        .collect();

                    let compound_stmt = CStmtKind::Compound(constituent_stmts);

                    self.add_stmt(new_id, located(node, compound_stmt));
                    self.processed_nodes.insert(new_id, OTHER_STMT);
                }

                ASTEntryTag::TagDeclStmt if expected_ty & OTHER_STMT != 0 => {
                    let decls = node.children
                        .iter()
                        .map(|decl| {
                            let decl_id = decl.expect("Decl not found in decl-statement");
                            self.visit_decl(&decl_id)
                        })
                        .collect();

                    let decls_stmt = CStmtKind::Decls(decls);

                    self.add_stmt(new_id, located(node, decls_stmt));
                    self.processed_nodes.insert(new_id, OTHER_STMT);
                }

                ASTEntryTag::TagReturnStmt if expected_ty & OTHER_STMT != 0 => {
                    let return_expr_opt = node.children[0]
                        .map(|id| self.visit_expr(&id));

                    let return_stmt = CStmtKind::Return(return_expr_opt);

                    self.add_stmt(new_id, located(node, return_stmt));
                    self.processed_nodes.insert(new_id, OTHER_STMT);
                }

                ASTEntryTag::TagIfStmt if expected_ty & OTHER_STMT != 0 => {
                    let scrutinee_old = node.children[0].expect("If condition expression not found");
                    let scrutinee = self.visit_expr(&scrutinee_old);

                    let true_variant_old = node.children[1].expect("If then body statement not found");
                    let true_variant = self.visit_stmt(&true_variant_old);

                    let false_variant = node.children[2]
                        .map(|id| self.visit_stmt(&id));

                    let if_stmt = CStmtKind::If { scrutinee, true_variant, false_variant };

                    self.add_stmt(new_id, located(node, if_stmt));
                    self.processed_nodes.insert(new_id, OTHER_STMT);
                }

                ASTEntryTag::TagGotoStmt if expected_ty & OTHER_STMT != 0 => {
                    let target_label_old = node.children[0].expect("Goto target label not found");
                    let target_label = CStmtId(self.visit_node_type(&target_label_old, LABEL_STMT));

                    let goto_stmt = CStmtKind::Goto(target_label);

                    self.add_stmt(new_id, located(node, goto_stmt));
                    self.processed_nodes.insert(new_id, OTHER_STMT);
                }

                ASTEntryTag::TagNullStmt if expected_ty & OTHER_STMT != 0 => {
                    let null_stmt = CStmtKind::Empty;

                    self.add_stmt(new_id, located(node, null_stmt));
                }

                ASTEntryTag::TagForStmt if expected_ty & OTHER_STMT != 0 => {
                    let init = node.children[0].map(|id| self.visit_stmt(&id));

                    let condition = node.children[1].map(|id| self.visit_expr(&id));

                    let increment = node.children[2].map(|id| self.visit_expr(&id));

                    let body_old = node.children[3].expect("For loop body not found");
                    let body = self.visit_stmt(&body_old);

                    let for_stmt = CStmtKind::ForLoop { init, condition, increment, body };

                    self.add_stmt(new_id, located(node, for_stmt));
                }

                ASTEntryTag::TagWhileStmt if expected_ty & OTHER_STMT != 0 => {
                    let condition_old = node.children[0].expect("While loop condition not found");
                    let condition = self.visit_expr(&condition_old);

                    let body_old = node.children[1].expect("While loop body not found");
                    let body = self.visit_stmt(&body_old);

                    let while_stmt = CStmtKind::While { condition, body };

                    self.add_stmt(new_id, located(node, while_stmt));
                }

                ASTEntryTag::TagDoStmt if expected_ty & OTHER_STMT != 0 => {

                    let body_old = node.children[0].expect("Do loop body not found");
                    let body = self.visit_stmt(&body_old);

                    let condition_old = node.children[1].expect("Do loop condition not found");
                    let condition = self.visit_expr(&condition_old);

                    let do_stmt = CStmtKind::DoWhile { body, condition };

                    self.add_stmt(new_id, located(node, do_stmt));
                }

                ASTEntryTag::TagLabelStmt if expected_ty & LABEL_STMT != 0 => {
                    let pointed_stmt_old = node.children[0].expect("Label statement not found");
                    let pointed_stmt = self.visit_stmt(&pointed_stmt_old);

                    let label_stmt = CStmtKind::Label(pointed_stmt);

                    self.add_stmt(new_id, located(node, label_stmt));
                    self.processed_nodes.insert(new_id, LABEL_STMT);
                }

                // Expressions

                ASTEntryTag::TagParenExpr if expected_ty & (EXPR | STMT) != 0 => {
                    let wrapped = node.children[0].expect("Expected wrapped paren expression");

                    self.id_mapper.merge_old(node_id, wrapped);
                    self.visit_node_type(&wrapped, expected_ty);
                }

                ASTEntryTag::TagIntegerLiteral if expected_ty & (EXPR | STMT) != 0 => {
                    let value = expect_u64(&node.extras[0]).expect("Expected integer literal value");

                    let ty_old = node.type_id.expect("Expected expression to have type");
                    let ty = self.visit_type(&ty_old);

                    let integer_literal = CExprKind::Literal(ty, CLiteral::Integer(value));

                    self.expr_possibly_as_stmt(expected_ty, new_id, node, integer_literal);
                }

                ASTEntryTag::TagCharacterLiteral if expected_ty & (EXPR | STMT) != 0 => {
                    let value = expect_u64(&node.extras[0]).expect("Expected character literal value");

                    let ty_old = node.type_id.expect("Expected expression to have type");
                    let ty = self.visit_type(&ty_old);

                    let character_literal = CExprKind::Literal(ty, CLiteral::Character(value));

                    self.expr_possibly_as_stmt(expected_ty, new_id, node, character_literal);
                }

                ASTEntryTag::TagFloatingLiteral if expected_ty & (EXPR | STMT) != 0 => {
                    let value = expect_f64(&node.extras[0]).expect("Expected float literal value");

                    let ty_old = node.type_id.expect("Expected expression to have type");
                    let ty = self.visit_type(&ty_old);

                    let floating_literal = CExprKind::Literal(ty, CLiteral::Floating(value));

                    self.expr_possibly_as_stmt(expected_ty, new_id, node, floating_literal);
                }

                ASTEntryTag::TagUnaryOperator if expected_ty & (EXPR | STMT) != 0 => {
                    let operator = match expect_str(&node.extras[0]).expect("Expected operator") {
                        "&" => UnOp::AddressOf,
                        "*" => UnOp::Deref,
                        "+" => UnOp::Plus,
                        "-" => UnOp::Negate,
                        "~" => UnOp::Complement,
                        "!" => UnOp::Not,
                        "++" => UnOp::Increment,
                        "--" => UnOp::Decrement,
                        o => panic!("Unexpected operator: {}", o),
                    };

                    let operand_old = node.children[0].expect("Expected operand");
                    let operand = self.visit_expr(&operand_old);

                    let ty_old = node.type_id.expect("Expected expression to have type");
                    let ty = self.visit_type(&ty_old);

                    let prefix = expect_bool(&node.extras[1]).expect("Expected prefix information");

                    let unary = CExprKind::Unary(ty, operator, prefix, operand);

                    self.expr_possibly_as_stmt(expected_ty, new_id, node, unary);
                }

                ASTEntryTag::TagImplicitCastExpr if expected_ty & (EXPR | STMT) != 0 => {
                    let expression_old = node.children[0].expect("Expected expression for implicit cast");
                    let expression = self.visit_expr(&expression_old);

                    let typ_old = node.type_id.expect("Expected type for implicit cast");
                    let typ = self.visit_type(&typ_old);

                    let implicit = CExprKind::ImplicitCast(typ, expression);

                    self.expr_possibly_as_stmt(expected_ty, new_id, node, implicit);
                }

                ASTEntryTag::TagCallExpr if expected_ty & (EXPR | STMT) != 0 => {
                    let func_old = node.children[0].expect("Expected function for function call");
                    let func = self.visit_expr(&func_old);

                    let args: Vec<CExprId> = node.children
                        .iter()
                        .skip(1)
                        .map(|id| {
                            let arg_id = id.expect("Expected call expression argument");
                            self.visit_expr(&arg_id)
                        })
                        .collect();

                    let ty_old = node.type_id.expect("Expected expression to have type");
                    let ty = self.visit_type(&ty_old);

                    let call = CExprKind::Call(ty, func, args);

                    self.expr_possibly_as_stmt(expected_ty, new_id, node, call);
                }

                ASTEntryTag::TagMemberExpr if expected_ty & (EXPR | STMT) != 0 => {
                    let base_old = node.children[0].expect("Expected base for member expression");
                    let base = self.visit_expr(&base_old);

                    let field_old = node.children[1].expect("Expected field for member expression");
                    let field = self.visit_decl(&field_old);

                    let ty_old = node.type_id.expect("Expected expression to have type");
                    let ty = self.visit_type(&ty_old);

                    let member = CExprKind::Member(ty, base, field);

                    self.expr_possibly_as_stmt(expected_ty, new_id, node, member);
                }

                ASTEntryTag::TagBinaryOperator if expected_ty & (EXPR | STMT) != 0 => {
                    let operator = match expect_str(&node.extras[0]).expect("Expected operator") {
                        "*" => BinOp::Multiply,
                        "/" => BinOp::Divide,
                        "%" => BinOp::Modulus,
                        "+" => BinOp::Add,
                        "-" => BinOp::Subtract,
                        "<<" => BinOp::ShiftLeft,
                        ">>" => BinOp::ShiftRight,
                        "<" => BinOp::Less,
                        ">" => BinOp::Greater,
                        "<=" => BinOp::LessEqual,
                        ">=" => BinOp::GreaterEqual,
                        "==" => BinOp::EqualEqual,
                        "!=" => BinOp::NotEqual,
                        "&" => BinOp::BitAnd,
                        "^" => BinOp::BitXor,
                        "|" => BinOp::BitOr,
                        "&&" => BinOp::And,
                        "||" => BinOp::Or,
                        "+=" => BinOp::AssignAdd,
                        "-=" => BinOp::AssignSubtract,
                        "*=" => BinOp::AssignMultiply,
                        "/=" => BinOp::AssignDivide,
                        "%=" => BinOp::AssignModulus,
                        "^=" => BinOp::AssignBitXor,
                        "<<=" => BinOp::AssignShiftLeft,
                        ">>=" => BinOp::AssignShiftRight,
                        "|=" => BinOp::AssignBitOr,
                        "&=" => BinOp::AssignBitAnd,
                        "=" => BinOp::Assign,
                        "," => BinOp::Comma,
                        _ => unimplemented!(),
                    };

                    let left_operand_old = node.children[0].expect("Expected left operand");
                    let left_operand = self.visit_expr(&left_operand_old);

                    let right_operand_old = node.children[1].expect("Expected right operand");
                    let right_operand = self.visit_expr(&right_operand_old);

                    let ty_old = node.type_id.expect("Expected expression to have type");
                    let ty = self.visit_type(&ty_old);

                    let binary = CExprKind::Binary(ty, operator, left_operand, right_operand);

                    self.expr_possibly_as_stmt(expected_ty, new_id, node, binary);
                }

                ASTEntryTag::TagDeclRefExpr if expected_ty & (EXPR | STMT) != 0 => {
                    let declaration_old = node.children[0].expect("Expected declaration on expression tag decl");
                    let declaration = self.visit_decl(&declaration_old);

                    let ty_old = node.type_id.expect("Expected expression to have type");
                    let ty = self.visit_type(&ty_old);

                    let decl = CExprKind::DeclRef(ty, declaration);

                    self.expr_possibly_as_stmt(expected_ty, new_id, node, decl);
                }

                ASTEntryTag::TagArraySubscriptExpr if expected_ty & (EXPR | STMT) != 0 => {
                    let lhs_old = node.children[0].expect("Expected LHS on array subscript expression");
                    let lhs = self.visit_expr(&lhs_old);

                    let rhs_old = node.children[1].expect("Expected RHS on array subscript expression");
                    let rhs = self.visit_expr(&rhs_old);

                    let ty_old = node.type_id.expect("Expected expression to have type");
                    let ty = self.visit_type(&ty_old);

                    let subcript = CExprKind::ArraySubscript(ty, lhs, rhs);

                    self.expr_possibly_as_stmt(expected_ty, new_id, node, subcript);
                }

                // Declarations

                ASTEntryTag::TagFunctionDecl if expected_ty & OTHER_DECL != 0 => {
                    let name = expect_str(&node.extras[0]).expect("Expected to find function name").to_string();

                    let typ_old = node.type_id.expect("Expected to find a type on a function decl");
                    let typ = CTypeId(self.visit_node_type(&typ_old, FUNC_TYPE));

                    let (body_id, parameter_ids) = node.children.split_last().expect("Expected to find a fucntion body");

                    let body_old = body_id.expect("Function body not found");
                    let body = self.visit_stmt(&body_old);

                    let parameters = parameter_ids
                        .iter()
                        .map(|id| {
                            let param = id.expect("Param field decl not found");
                            CDeclId(self.visit_node_type(&param, VAR_DECL))
                        })
                        .collect();

                    let function_decl = CDeclKind::Function { typ, name, parameters, body };

                    self.add_decl(new_id, located(node, function_decl));
                    self.processed_nodes.insert(new_id, OTHER_DECL);
                }

                ASTEntryTag::TagTypedefDecl if expected_ty & TYPDEF_DECL != 0 => {
                    let name = expect_str(&node.extras[0]).expect("Expected to find typedef name").to_string();

                    let typ_old = node.type_id.expect("Expected to find type on typedef declaration");
                    let typ = self.visit_type(&typ_old);

                    let typdef_decl = CDeclKind::Typedef { name, typ };

                    self.add_decl(new_id, located(node, typdef_decl));
                    self.processed_nodes.insert(new_id, TYPDEF_DECL);
                }

                ASTEntryTag::TagVarDecl if expected_ty & VAR_DECL != 0 => {
                    let ident = expect_str(&node.extras[0]).expect("Expected to find variable name").to_string();

                    let initializer = node.children[0]
                        .map(|id| self.visit_expr(&id));

                    let typ_old = node.type_id.expect("Expected to find type on variable declaration");
                    let typ_old_node = untyped_context.type_nodes
                        .get(&typ_old)
                        .expect("Variable type child not found");
                    let new_typ = self.visit_type(&typ_old);

                    let typ = CQualTypeId { qualifiers: qualifiers(typ_old_node), ctype: new_typ };

                    let variable_decl = CDeclKind::Variable { ident, initializer, typ };

                    self.add_decl(new_id, located(node, variable_decl));
                    self.processed_nodes.insert(new_id, VAR_DECL);
                }

                ASTEntryTag::TagRecordDecl if expected_ty & RECORD_DECL != 0 => {
                    let name = expect_str(&node.extras[0]).ok().map(str::to_string);
                    let fields: Vec<CDeclId> = node.children
                        .iter()
                        .map(|id| {
                            let field = id.expect("Record field decl not found");
                            CDeclId(self.visit_node_type(&field, FIELD_DECL))
                        })
                        .collect();

                    let record = CDeclKind::Record { name, fields };

                    self.add_decl(new_id, located(node, record));
                    self.processed_nodes.insert(new_id, RECORD_DECL);
                },

                ASTEntryTag::TagFieldDecl if expected_ty & FIELD_DECL != 0 => {
                    let name = expect_str(&node.extras[0]).expect("A field needs a name").to_string();
                    let field = CDeclKind::Field { name };
                    self.add_decl(new_id, located(node, field));
                    self.processed_nodes.insert(new_id, FIELD_DECL);
                }

                t => println!("Could not translate node {:?} as type {}", t, expected_ty),
            }
        }
    }
}

