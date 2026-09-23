import Kernel

/-! Data-only composition/v1 contract. KP2's scalar kernel remains unchanged.
Constructed values carry bounded expanded-size summaries, not partial values. -/
namespace Kpopper.Composition

structure Size where
  nodes : Nat := 1
  depth : Nat := 0
  bytes : Nat := 0
  members : Nat := 0
  deriving Inhabited

inductive Value where
  | scalar (value : Kpopper.Value)
  | array (items : List Value) (size : Size)
  | object (fields : List (String × Value)) (size : Size)
  deriving Inhabited

inductive Expr where
  | literal (value : Kpopper.Value)
  | ref (id : String)
  | unavailable (code : String)
  | binary (op : Kpopper.Op) (left right : Expr)
  | logic (isAnd : Bool) (left right : Expr)
  | negate (value : Expr)
  | conditional (condition whenTrue whenFalse : Expr)
  | array (items : List Expr)
  | object (fields : List (String × Expr))
  | field (record : Expr) (key : String)
  deriving Inhabited

inductive Result where
  | known (value : Value)
  | unknown
  | error
  deriving Inhabited

structure Limits extends Kpopper.Limits where
  valueNodes : Nat := 10000
  valueDepth : Nat := 128
  valueBytes : Nat := 16777216
  deriving Inhabited

structure State where
  base : Kpopper.State := {}
  memo : Std.HashMap String Result := {}

abbrev EvalM := ExceptT String (StateM State)

inductive ValueType where
  | unknown
  | scalar (kind : Kpopper.ValueType)
  | array (items : List ValueType)
  | object (fields : List (String × ValueType))
  deriving Inhabited

structure TypeState where
  memo : Std.HashMap String ValueType := {}
  active : Std.HashSet String := {}
  visits : Nat := 0

abbrev TypeM := ExceptT String (StateM TypeState)

end Kpopper.Composition
