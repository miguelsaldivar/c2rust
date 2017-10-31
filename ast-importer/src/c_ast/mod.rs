use std::collections::{HashSet, HashMap};
use std::ops::Index;

#[derive(Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Copy, Clone)]
pub struct CTypeId(u64);

#[derive(Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Copy, Clone)]
pub struct CExprId(u64);

#[derive(Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Copy, Clone)]
pub struct CDeclId(u64);

#[derive(Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Copy, Clone)]
pub struct CStmtId(u64);

// These are references into particular variants of AST nodes
pub type CLabelId = CStmtId;  // Labels point into the 'StmtKind::Label' that declared the label
pub type CFieldId = CDeclId;  // Records always contain 'DeclKind::Field's
pub type CParamId = CDeclId;  // Parameters always contain 'DeclKind::Variable's
pub type CFuncTypeId = CTypeId;  // Function declarations always have types which are 'TypeKind::Function'
pub type CRecordId = CDeclId;  // Record types need to point to 'DeclKind::Record'
pub type CTypedefId = CDeclId;  // Typedef types need to point to 'DeclKind::Typedef'

pub use self::conversion::*;
pub use self::print::Printer;

mod conversion;
mod print;

/// AST context containing all of the nodes in the Clang AST
#[derive(Debug, Clone)]
pub struct TypedAstContext {
    pub c_types: HashMap<CTypeId, CType>,
    pub c_exprs: HashMap<CExprId, CExpr>,
    pub c_decls: HashMap<CDeclId, CDecl>,
    pub c_stmts: HashMap<CStmtId, CStmt>,

    pub c_decls_top: HashSet<CDeclId>,
    pub c_files: HashMap<u64, String>,
}

impl TypedAstContext {
    pub fn new() -> TypedAstContext {
        TypedAstContext {
            c_types: HashMap::new(),
            c_exprs: HashMap::new(),
            c_decls: HashMap::new(),
            c_stmts: HashMap::new(),

            c_decls_top: HashSet::new(),
            c_files: HashMap::new(),
        }
    }

    pub fn resolve_type_id(&self, typ: CTypeId) -> CTypeId {
        match (*self.index(typ)).kind {
            CTypeKind::Elaborated(ty) => self.resolve_type_id(ty),
            CTypeKind::Decayed(ty) => self.resolve_type_id(ty),
            CTypeKind::TypeOf(ty) => self.resolve_type_id(ty),
            CTypeKind::Typedef(decl) => {
                match self.index(decl).kind {
                    CDeclKind::Typedef { typ: ty, .. } => self.resolve_type_id(ty),
                    _ => panic!("Typedef decl did not point to a typedef"),
                }
            },
            _ => typ,
        }
    }

    pub fn resolve_type(&self, typ: CTypeId) -> &CType {
        let resolved_typ_id = self.resolve_type_id(typ);
        self.index(resolved_typ_id)
    }
}

impl Index<CTypeId> for TypedAstContext {
    type Output = CType;

    fn index(&self, index: CTypeId) -> &CType {
        match self.c_types.get(&index) {
            None => panic!("Could not find {:?} in TypedAstContext", index),
            Some(ty) => ty,
        }
    }
}

impl Index<CExprId> for TypedAstContext {
    type Output = CExpr;

    fn index(&self, index: CExprId) -> &CExpr {
        match self.c_exprs.get(&index) {
            None => panic!("Could not find {:?} in TypedAstContext", index),
            Some(ty) => ty,
        }
    }
}

impl Index<CDeclId> for TypedAstContext {
    type Output = CDecl;

    fn index(&self, index: CDeclId) -> &CDecl {
        match self.c_decls.get(&index) {
            None => panic!("Could not find {:?} in TypedAstContext", index),
            Some(ty) => ty,
        }
    }
}

impl Index<CStmtId> for TypedAstContext {
    type Output = CStmt;

    fn index(&self, index: CStmtId) -> &CStmt {
        match self.c_stmts.get(&index) {
            None => panic!("Could not find {:?} in TypedAstContext", index),
            Some(ty) => ty,
        }
    }
}

/// Represents a position inside a C source file
#[derive(Debug, Copy, Clone)]
pub struct SrcLoc {
    pub line: u64,
    pub column: u64,
    pub fileid: u64,
}

/// Represents some AST node possibly with source location information bundled with it
#[derive(Debug, Clone)]
pub struct Located<T> {
    pub loc: Option<SrcLoc>,
    pub kind: T,
}

/// All of our AST types should have location information bundled with them
pub type CDecl = Located<CDeclKind>;
pub type CStmt = Located<CStmtKind>;
pub type CExpr = Located<CExprKind>;
pub type CType = Located<CTypeKind>;


#[derive(Debug, Clone)]
pub enum CDeclKind {
    // http://clang.llvm.org/doxygen/classclang_1_1FunctionDecl.html
    Function {
        typ: CFuncTypeId,
        name: String,
        parameters: Vec<CParamId>,
        body: CStmtId,
    },

    // http://clang.llvm.org/doxygen/classclang_1_1VarDecl.html
    Variable {
        ident: String,
        initializer: Option<CExprId>,
        typ: CQualTypeId,
    },

    // Enum       // http://clang.llvm.org/doxygen/classclang_1_1EnumDecl.html

    // Typedef
    Typedef {
        name: String,
        typ: CTypeId,
    },

    // Record
    Record {
        name: Option<String>,
        fields: Vec<CFieldId>,
    },

    // Field
    Field {
        /* TODO: type */
        name: String,
    },
}

impl CDeclKind {
    pub fn get_name(&self) -> Option<&String> {
        match self {
            &CDeclKind::Function { name: ref i, .. } => Some(i),
            &CDeclKind::Variable { ident: ref i, .. } => Some(i),
//            &CDeclKind::Record { ref name, fields } => ???,
            &CDeclKind::Field { name: ref i, .. } => Some(i),
            _ => None,
        }
    }
}

/// Represents an expression in C (6.5 Expressions)
#[derive(Debug, Clone)]
pub enum CExprKind {
    // Literals
    Literal(CTypeId, CLiteral),

    // Unary operator. The boolean field tells us if the operator is prefix
    Unary(CTypeId, UnOp, bool, CExprId),

    // Binary operator
    Binary(CTypeId, BinOp, CExprId, CExprId),

    // Implicit cast
    // TODO: consider adding the cast type (see OperationKinds.def)
    ImplicitCast(CTypeId, CExprId),

    // Reference to a decl (a variable, for instance)
    // TODO: consider enforcing what types of declarations are allowed here
    DeclRef(CTypeId, CDeclId),

    // Function call
    Call(CTypeId, CExprId, Vec<CExprId>),

    // Member access
    Member(CTypeId, CExprId, CDeclId),

    // Array subscript access
    ArraySubscript(CTypeId, CExprId, CExprId),
}

impl CExprKind {
    pub fn get_type(&self) -> CTypeId {
        match *self {
            CExprKind::Literal(ty, _) => ty,
            CExprKind::Unary(ty, _, _, _) => ty,
            CExprKind::Binary(ty, _, _, _) => ty,
            CExprKind::ImplicitCast(ty, _) => ty,
            CExprKind::DeclRef(ty, _) => ty,
            CExprKind::Call(ty, _, _) => ty,
            CExprKind::Member(ty, _, _) => ty,
            CExprKind::ArraySubscript(ty, _, _) => ty,
        }
    }
}

/// Represents a unary operator in C (6.5.3 Unary operators)
#[derive(Debug, Clone, Copy)]
pub enum UnOp {
    AddressOf,  // &
    Deref,      // *
    Plus,       // +
    Increment,  // ++
    Negate,     // -
    Decrement,  // --
    Complement, // ~
    Not,        // !
}

/// Represents a binary operator in C (6.5.5 Multiplicative operators - 6.5.14 Logical OR operator)
#[derive(Debug, Clone, Copy)]
pub enum BinOp {
    Multiply,         // *
    Divide,           // /
    Modulus,          // %
    Add,              // +
    Subtract,         // -
    ShiftLeft,        // <<
    ShiftRight,       // >>
    Less,             // <
    Greater,          // >
    LessEqual,        // <=
    GreaterEqual,     // >=
    EqualEqual,       // ==
    NotEqual,         // !=
    BitAnd,           // &
    BitXor,           // ^
    BitOr,            // |
    And,              // &&
    Or,               // ||

    AssignAdd,        // +=
    AssignSubtract,   // -=
    AssignMultiply,   // *=
    AssignDivide,     // /=
    AssignModulus,    // %=
    AssignBitXor,     // ^=
    AssignShiftLeft,  // <<=
    AssignShiftRight, // >>=
    AssignBitOr,      // |=
    AssignBitAnd,     // &=

    Assign,           // =
    Comma,            // ,
}

#[derive(Debug, Clone, Copy)]
pub enum CLiteral {
    Integer(u64),
    Character(u64),
    Floating(f64),
    // TODO: String
}


/// Represents a statement in C (6.8 Statements)
///
/// Reflects the types in <http://clang.llvm.org/doxygen/classclang_1_1Stmt.html>
#[derive(Debug, Clone)]
pub enum CStmtKind {
    // Labeled statements (6.8.1)
    Label(CStmtId),
    Case(CExpr, CStmtId), // The second argument is only the immediately following statement
    Default(CStmtId),

    // Compound statements (6.8.2)
    Compound(Vec<CStmtId>),
  
    // Expression and null statements (6.8.3)
    Expr(CExprId),
    Empty,

    // Selection statements (6.8.4)
    If {
        scrutinee: CExprId,
        true_variant: CStmtId,
        false_variant: Option<CStmtId>,
    },
    Switch {
        scrutinee: CExprId,
        body: CStmtId,
    },
 
    // Iteration statements (6.8.5)
    While {
        condition: CExprId,
        body: CStmtId,
    },
    DoWhile {
        body: CStmtId,
        condition: CExprId,
    },
    ForLoop {
        init: Option<CStmtId>,
        condition: Option<CExprId>,
        increment: Option<CExprId>,
        body: CStmtId,
    },

    // Jump statements (6.8.6)
    Goto(CLabelId),
    Break,
    Continue,
    Return(Option<CExprId>),

    // Declarations (variables, etc.)
    Decls(Vec<CDeclId>),
}

/// Type qualifiers (6.7.3)
#[derive(Debug, Copy, Clone)]
pub struct Qualifiers {
    pub is_const: bool,
    pub is_restrict: bool,
    pub is_volatile: bool,
}

/// Qualified type
#[derive(Debug, Copy, Clone)]
pub struct CQualTypeId {
    pub qualifiers: Qualifiers,
    pub ctype: CTypeId,
}


// TODO: these may be interesting, but I'm not sure if they fit here:
//
//  * UnaryTranformType <http://clang.llvm.org/doxygen/classclang_1_1UnaryTransformType.html>
//  * AdjustedType <http://clang.llvm.org/doxygen/classclang_1_1AdjustedType.html>

/// Represents a type in C (6.2.5 Types)
///
/// Reflects the types in <http://clang.llvm.org/doxygen/classclang_1_1Type.html>
#[derive(Debug, Clone)]
pub enum CTypeKind {
    /* Builtin types: <https://github.com/llvm-mirror/clang/include/clang/AST/BuiltinTypes.def> */

    // Void type (6.2.5.19)
    Void,
  
    // Boolean type (6.2.5.2)
    Bool,
  
    Size,
  
    // Character type (6.2.5.3)
    Char,
  
    // Signed types (6.2.5.4)
    SChar, Short, Int, Long, LongLong,
  
    // Unsigned types (6.2.5.6) (actually this also includes `_Bool`)
    UChar, UShort, UInt, ULong, ULongLong,

    // Real floating types (6.2.5.10). Ex: `double`
    Float, Double, LongDouble,

  
    /* Compound types <https://github.com/llvm-mirror/clang/include/clang/AST/TypeNodes.def> */
  
    // Complex types (6.2.5.11). Ex: `float _Complex`.
    Complex(CTypeId),

    // Pointer types (6.7.5.1)
    Pointer(CQualTypeId),

    // Array types (6.7.5.2)
    ConstantArray(CQualTypeId, usize),
    IncompleteArray(CQualTypeId),
    VariableArray(CQualTypeId, CExprId),

    // Type of type or expression (GCC extension)
    TypeOf(CTypeId),
    TypeOfExpr(CExprId),

    // Function type (6.7.5.3)
    //
    // Note a function taking no arguments should have one `void` argument. Functions without any
    // arguments and in K&R format.
    Function(CQualTypeId, Vec<CQualTypeId>),

    // Type definition type (6.7.7)
    Typedef(CTypedefId),

    // Represents a pointer type decayed from an array or function type.
    Decayed(CTypeId),
    Elaborated(CTypeId),

    // Struct or union type
    //
    // XXX: distinction between `struct` and `union`
    Record(CRecordId),

    Enum(CDeclId),    // TODO same comment as Typedef
}

impl CTypeKind {

    pub fn is_pointer(&self) -> bool {
        match *self {
            CTypeKind::Pointer(_) => true,
            _ => false,
        }
    }

    pub fn is_unsigned_integral_type(&self) -> bool {
        match *self {
            CTypeKind::UInt => true,
            CTypeKind::UShort => true,
            CTypeKind::ULong => true,
            CTypeKind::ULongLong => true,
            _ => false,
        }
    }
}