import Std.Data.HashMap
import Std.Data.HashSet
import Init.Data.Rat

/-! Scalar arithmetic/v1. Every recursive evaluator call consumes explicit fuel.
No parser, IO, or host environment is part of these functions. -/
namespace Kpopper

inductive Value where
  | number (r : Rat)
  | boolean (b : Bool)
  | text (s : String)
  | null
  deriving Inhabited

inductive Op where
  | add | sub | mul | div | eq | ne | lt | le | gt | ge
  deriving BEq, Inhabited

inductive Expr where
  | literal (v : Value)
  | ref (id : String)
  | unavailable (code : String)
  | binary (op : Op) (left right : Expr)
  deriving Inhabited

inductive Result where
  | known (v : Value)
  | unknown
  | error
  deriving Inhabited

structure Limits where
  steps : Nat := 1000000
  depth : Nat := 128
  digits : Nat := 256

structure State where
  steps : Nat := 0
  diagnostics : Std.HashSet String := {}
  reads : Std.HashSet String := {}
  counts : Std.HashMap String Nat := {}
  memo : Std.HashMap String Result := {}
  active : Std.HashSet String := {}

abbrev EvalM := ExceptT String (StateM State)

def diagnose (code : String) : EvalM Unit :=
  modify fun st => { st with diagnostics := st.diagnostics.insert code }

def failure (code : String) : EvalM Result := do
  diagnose code
  return .error

def numeric (r : Rat) (limits : Limits) : EvalM Result := do
  if (toString r.num.natAbs).length > limits.digits || (toString r.den).length > limits.digits then
    throw "number_limit"
  return .known (.number r)

def unavailable (a b : Result) : Result :=
  match a, b with
  | .error, _ | _, .error => .error
  | _, _ => .unknown

def badNumeric : Result → Bool
  | .known (.number _) => false
  | .known _ => true
  | _ => false

def equalValues : Value → Value → Option Bool
  | .number a, .number b => some (a == b)
  | .boolean a, .boolean b => some (a == b)
  | .text a, .text b => some (a == b)
  | .null, .null => some true
  | _, _ => none

def arithmetic (op : Op) (x y : Rat) (limits : Limits) : EvalM Result := do
  match op with
  | .add => numeric (x + y) limits
  | .sub => numeric (x - y) limits
  | .mul => numeric (x * y) limits
  | .div =>
    if y.num == 0 then failure "division_by_zero"
    else numeric (x / y) limits
  | .lt => return .known (.boolean (x < y))
  | .le => return .known (.boolean (x <= y))
  | .gt => return .known (.boolean (x > y))
  | .ge => return .known (.boolean (x >= y))
  | .eq | .ne => failure "type_error"

def binary (op : Op) (a b : Result) (limits : Limits) : EvalM Result := do
  if op == .eq || op == .ne then
    match a, b with
    | .known x, .known y =>
      match equalValues x y with
      | some v => return .known (.boolean (if op == .eq then v else !v))
      | none => failure "type_error"
    | _, _ => return unavailable a b
  else
    if badNumeric a || badNumeric b then return ← failure "type_error"
    match a, b with
    | .known (.number x), .known (.number y) => arithmetic op x y limits
    | _, _ => return unavailable a b

def evaluate : Nat → Std.HashMap String Expr → Limits → Nat → Expr → EvalM Result
  | 0, _, _, _, _ => throw "depth_limit"
  | fuel + 1, nodes, limits, depth, expr => do
    if depth > limits.depth then throw "depth_limit"
    if (← get).steps >= limits.steps then throw "step_limit"
    modify fun st => { st with steps := st.steps + 1 }
    match expr with
    | .literal (.number r) => numeric r limits
    | .literal v => return .known v
    | .unavailable code =>
      diagnose code
      return .unknown
    | .ref id =>
      modify fun st => { st with reads := st.reads.insert id }
      if (← get).active.contains id then return ← failure "cyclic_reference"
      if let some result := (← get).memo[id]? then return result
      let some body := nodes[id]? | do
        diagnose "missing_reference"
        return .unknown
      modify fun st => { st with counts := st.counts.insert id ((st.counts[id]?).getD 0 + 1),
                                  active := st.active.insert id }
      let result ← evaluate fuel nodes limits (depth + 1) body
      modify fun st => { st with active := st.active.erase id, memo := st.memo.insert id result }
      return result
    | .binary op left right =>
      let a ← evaluate fuel nodes limits (depth + 1) left
      let b ← evaluate fuel nodes limits (depth + 1) right
      binary op a b limits

def refs : Expr → List String
  | .literal _ | .unavailable _ => []
  | .ref id => [id]
  | .binary _ left right => refs left ++ refs right

/-- Worklist fuel counts edges, including repeated edges; cycles never recurse. -/
def closure : Nat → Std.HashMap String Expr → List String → Std.HashSet String →
    Except (String × Std.HashSet String) (Std.HashSet String)
  | _, _, [], seen => .ok seen
  | 0, _, _ :: _, seen => .error ("edge_limit", seen)
  | fuel + 1, nodes, id :: rest, seen =>
    if seen.contains id then closure fuel nodes rest seen
    else
      let seen := seen.insert id
      match nodes[id]? with
      | none => closure fuel nodes rest seen
      | some expr => closure fuel nodes (refs expr ++ rest) seen

inductive ValueType where
  | number | boolean | text | null
  deriving BEq, Inhabited

def valueType : Value → ValueType
  | .number _ => .number | .boolean _ => .boolean | .text _ => .text | .null => .null

structure TypeState where
  memo : Std.HashMap String (Option ValueType) := {}
  active : Std.HashSet String := {}
  visits : Nat := 0

abbrev TypeM := ExceptT String (StateM TypeState)

/-- Unknown inputs and cycles assert no scalar type. Known types are checked
before evaluation; unavailable data never acquire an invented null type. -/
def inferType : Nat → Std.HashMap String Expr → Expr → TypeM (Option ValueType)
  | 0, _, _ => throw "depth_limit"
  | fuel + 1, nodes, expr => do
    if (← get).visits >= 1000000 then throw "step_limit"
    modify fun st => { st with visits := st.visits + 1 }
    match expr with
    | .literal v => return some (valueType v)
    | .unavailable _ => return none
    | .ref id =>
      if let some result := (← get).memo[id]? then return result
      if (← get).active.contains id then return none
      let some body := nodes[id]? | return none
      modify fun st => { st with active := st.active.insert id }
      let result ← inferType fuel nodes body
      modify fun st => { st with active := st.active.erase id, memo := st.memo.insert id result }
      return result
    | .binary op left right =>
      let a ← inferType fuel nodes left
      let b ← inferType fuel nodes right
      if op == .eq || op == .ne then
        if let some x := a then
          if let some y := b then
            if x != y then throw "type_error"
        return some .boolean
      else
        if (a.any (· != .number)) || (b.any (· != .number)) then throw "type_error"
        return some (if op == .add || op == .sub || op == .mul || op == .div then .number else .boolean)

end Kpopper
