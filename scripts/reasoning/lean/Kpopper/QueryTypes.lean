import Composition

/-! Data-only types for the query/v1 kernel. The outer KP4 JSON/framing
decoder validates canonical wire spelling before constructing these values. -/
namespace Kpopper.Query

abbrev Value := Kpopper.Composition.Value
abbrev Size := Kpopper.Composition.Size

inductive Phase where
  | where_ | value | preflight | aggregate
  deriving BEq, Inhabited

def Phase.name : Phase → String
  | .where_ => "where"
  | .value => "value"
  | .preflight => "preflight"
  | .aggregate => "aggregate"

structure Location where
  candidate : String
  column : String
  phase : Phase
  deriving BEq, Inhabited

structure Diagnostic where
  code : String
  relatedIds : List String
  locations : List Location
  deriving Inhabited

inductive UnavailableReason where
  | unsupportedType | formulaValue | invalidValue
  deriving BEq, Inhabited

inductive FieldStatus where
  | known (value : Value)
  | missing
  | contested
  | unavailable (reason : UnavailableReason)
  deriving Inhabited

structure Member where
  id : String
  fields : List (String × FieldStatus)
  deriving Inhabited

structure ScopeWitness where
  scopeId : String
  definitionDigest : String
  membershipDigest : String
  projectedInputsDigest : String
  deriving Inhabited

structure NodeWitness where
  id : String
  fingerprint : String
  deriving Inhabited

inductive Witness where
  | node (value : NodeWitness)
  | scope (value : ScopeWitness)
  deriving Inhabited

/-- `fields` is required even when `members` is empty. It is part of the
scope definition and lets the native kernel reject undeclared columns without
deriving grants from a row that may not exist. -/
structure Scope where
  id : String
  fields : List String
  witness : ScopeWitness
  members : List Member
  deriving Inhabited

inductive Expr where
  | column (name : String)
  | literal (value : Kpopper.Value)
  | binary (op : Kpopper.Op) (left right : Expr)
  | logic (isAnd : Bool) (left right : Expr)
  | negate (value : Expr)
  | conditional (condition whenTrue whenFalse : Expr)
  | array (items : List Expr)
  | object (fields : List (String × Expr))
  | field (record : Expr) (key : String)
  deriving Inhabited

inductive Operation where
  | filter (where_ : Expr)
  | project (value : Expr)
  | select (where_ value : Expr)
  | count (where_ : Expr)
  | sum (where_ : Option Expr) (value : Expr)
  | all (where_ : Expr)
  | any (where_ : Expr)
  deriving Inhabited

structure NormalizedQuery where
  version : Nat := 1
  scope : String
  operation : Operation
  deriving Inhabited

structure Resources extends toCompositionLimits : Kpopper.Composition.Limits where
  version : String := "resources/v4"
  candidates : Nat := 10000
  fieldReads : Nat := 100000
  deriving Inhabited

structure Request where
  version : Nat := 4
  requestId : String
  resources : Resources
  requiredModules : List String
  query : NormalizedQuery
  rootWitness : Option NodeWitness := none
  scope : Scope
  deriving Inhabited

structure QueryCounts where
  inputCount : Nat := 0
  definiteMatchCount : Nat := 0
  unknownMembershipCount : Nat := 0
  unknownValueCount : Nat := 0
  errorCount : Nat := 0
  deriving BEq, Inhabited

structure Cost where
  steps : Nat := 0
  preflightSteps : Nat := 0
  nodeEvaluations : List (String × Nat) := []
  candidates : Nat := 0
  fieldReads : Nat := 0
  evaluatedFieldReads : Nat := 0
  deriving Inhabited

inductive Status where
  | ok | unknown | error | limit | unsupportedCapability
  deriving BEq, Inhabited

def Status.name : Status → String
  | .ok => "ok"
  | .unknown => "unknown"
  | .error => "error"
  | .limit => "limit"
  | .unsupportedCapability => "unsupported_capability"

structure Response where
  version : Nat := 4
  requestId : Option String
  status : Status
  value : Option Value := none
  diagnostics : List Diagnostic := []
  queryCounts : Option QueryCounts := none
  executedReads : List Witness := []
  cost : Cost := {}
  deriving Inhabited

inductive RowResult where
  | known (value : Value)
  | unknown
  | error
  deriving Inhabited

structure Completed where
  status : Status
  result : Option Value
  counts : QueryCounts
  diagnostics : List Diagnostic
  steps : Nat
  evaluatedFieldReads : Nat
  deriving Inhabited

structure Prepared where
  preflightSteps : Nat
  candidates : Nat
  fieldReads : Nat
  deriving Inhabited

end Kpopper.Query
