import CompositionTypes

/-! Bounded composition execution. No formal assurance beyond the unchanged scalar
kernel is claimed here. All recursive computation uses explicit fuel; structural
member scans consume the same v3 budgets as their owning execution phase. -/
namespace Kpopper.Composition

def charge (limits : Limits) : EvalM Unit := do
  if (← get).base.steps >= limits.steps then throw "step_limit"
  modify fun st => { st with base := { st.base with steps := st.base.steps + 1 } }

def typeCharge (limits : Limits) : TypeM Unit := do
  if (← get).visits >= limits.steps then throw "step_limit"
  modify fun st => { st with visits := st.visits + 1 }

def reverseEval {α : Type} (limits : Limits) (items : List α) : EvalM (List α) := do
  let mut out := []
  for item in items do
    charge limits
    out := item :: out
  return out

def reverseType {α : Type} (limits : Limits) (items : List α) : TypeM (List α) := do
  let mut out := []
  for item in items do
    typeCharge limits
    out := item :: out
  return out

def diagnose (code : String) : EvalM Unit :=
  modify fun st => { st with base := { st.base with diagnostics := st.base.diagnostics.insert code } }

def failure (code : String) : EvalM Result := do
  diagnose code
  return .error

def scalarRun (action : Kpopper.EvalM Kpopper.Result) : EvalM Result := do
  let (result, base) := action.run (← get).base
  modify fun st => { st with base := base }
  match result with
  | .error code => throw code
  | .ok (.known value) => return .known (.scalar value)
  | .ok .unknown => return .unknown
  | .ok .error => return .error

def scalarBytes : Kpopper.Value → Nat
  | .number n => 3 + (toString n.num).utf8ByteSize + (toString n.den).utf8ByteSize
  | .boolean _ => 3
  | .text s => 2 + 2 * s.utf8ByteSize
  | .null => 1

def valueSize : Value → Size
  | .scalar value => { bytes := scalarBytes value }
  | .array _ size | .object _ size => size

def checkSize (limits : Limits) (size : Size) : EvalM Unit := do
  if size.nodes > min 10000 limits.valueNodes || size.depth > min 128 limits.valueDepth ||
      size.bytes > min 16777216 limits.valueBytes then throw "collection_limit"

def checkedScalar (limits : Limits) (value : Kpopper.Value) : EvalM Result := do
  checkSize limits (valueSize (.scalar value))
  match value with
  | .number n => scalarRun (Kpopper.numeric n limits.toLimits)
  | _ => return .known (.scalar value)

def unavailable (a b : Result) : Result :=
  match a, b with
  | .error, _ | _, .error => .error
  | _, _ => .unknown

def scalarResult : Result → Kpopper.Result
  | .known (.scalar value) => .known value
  | .known _ => .error
  | .unknown => .unknown
  | .error => .error

def isContainer : Result → Bool
  | .known (.array ..) | .known (.object ..) => true
  | _ => false

/-- Key order is canonical at the parser boundary; reject manually built invalid
expressions as well, without an unmetered sorting operation. -/
def checkKeys {α : Type} (limits : Limits) (items : List (String × α)) : EvalM Unit := do
  let mut previous : Option String := none
  for (key, _) in items do
    charge limits
    if let some before := previous then
      if compare before key != .lt then throw "invalid_expression"
    previous := some key

def checkTypeKeys {α : Type} (limits : Limits) (items : List (String × α)) : TypeM Unit := do
  let mut previous : Option String := none
  for (key, _) in items do
    typeCharge limits
    if let some before := previous then
      if compare before key != .lt then throw "invalid_expression"
    previous := some key

def typeCount {α : Type} (limits : Limits) (items : List α) : TypeM Nat := do
  let mut count := 0
  for _ in items do
    typeCharge limits
    count := count + 1
  return count

/-- A same-shaped equality checks every member's comparability, even after a
previous unequal member. Cached member counts avoid repeated length traversals. -/
def equalValue : Nat → Limits → Value → Value → EvalM (Option Bool)
  | 0, _, _, _ => throw "depth_limit"
  | fuel + 1, limits, left, right => do
    charge limits
    match left, right with
    | .scalar a, .scalar b => return Kpopper.equalValues a b
    | .array a sa, .array b sb =>
      if sa.members != sb.members then return some false
      let mut rest := b
      let mut same := true
      let mut comparable := true
      for value in a do
        charge limits
        match rest with
        | [] => throw "invalid_expression"
        | other :: tail =>
          rest := tail
          match ← equalValue fuel limits value other with
          | none => comparable := false
          | some result => same := same && result
      if !rest.isEmpty then throw "invalid_expression"
      return if comparable then some same else none
    | .object a sa, .object b sb =>
      if sa.members != sb.members then return some false
      let mut rest := b
      let mut sameKeys := true
      for (key, _) in a do
        charge limits
        match rest with
        | [] => throw "invalid_expression"
        | (other, _) :: tail =>
          rest := tail
          sameKeys := sameKeys && key == other
      if !rest.isEmpty then throw "invalid_expression"
      if !sameKeys then return some false
      let mut values := b
      let mut same := true
      let mut comparable := true
      for (_, value) in a do
        charge limits
        match values with
        | [] => throw "invalid_expression"
        | (_, other) :: tail =>
          values := tail
          match ← equalValue fuel limits value other with
          | none => comparable := false
          | some result => same := same && result
      return if comparable then some same else none
    | _, _ => return none

/-- Unknown means potentially comparable. Definite incompatible kinds reject;
different collection shapes are comparable and unequal. -/
def comparableTypes : Nat → Limits → ValueType → ValueType → TypeM Bool
  | 0, _, _, _ => throw "depth_limit"
  | fuel + 1, limits, left, right => do
    typeCharge limits
    match left, right with
    | .unknown, _ | _, .unknown => return true
    | .scalar a, .scalar b => return a == b
    | .array a, .array b =>
      let na ← typeCount limits a
      let nb ← typeCount limits b
      if na != nb then return true
      let mut rest := b
      let mut valid := true
      for value in a do
        typeCharge limits
        match rest with
        | [] => throw "invalid_expression"
        | other :: tail =>
          rest := tail
          let result ← comparableTypes fuel limits value other
          valid := valid && result
      return valid
    | .object a, .object b =>
      let na ← typeCount limits a
      let nb ← typeCount limits b
      if na != nb then return true
      let mut rest := b
      let mut sameKeys := true
      for (key, _) in a do
        typeCharge limits
        match rest with
        | [] => throw "invalid_expression"
        | (other, _) :: tail =>
          rest := tail
          sameKeys := sameKeys && key == other
      if !sameKeys then return true
      let mut values := b
      let mut valid := true
      for (_, value) in a do
        typeCharge limits
        match values with
        | [] => throw "invalid_expression"
        | (_, other) :: tail =>
          values := tail
          let result ← comparableTypes fuel limits value other
          valid := valid && result
      return valid
    | _, _ => return false

def joinTypes : Nat → Limits → ValueType → ValueType → TypeM ValueType
  | 0, _, _, _ => throw "depth_limit"
  | fuel + 1, limits, left, right => do
    typeCharge limits
    match left, right with
    | .scalar a, .scalar b => return if a == b then .scalar a else .unknown
    | .array a, .array b =>
      let na ← typeCount limits a
      let nb ← typeCount limits b
      if na != nb then return .unknown
      let mut rest := b
      let mut out := []
      for value in a do
        typeCharge limits
        match rest with
        | [] => throw "invalid_expression"
        | other :: tail =>
          rest := tail
          out := (← joinTypes fuel limits value other) :: out
      return .array (← reverseType limits out)
    | .object a, .object b =>
      let na ← typeCount limits a
      let nb ← typeCount limits b
      if na != nb then return .unknown
      let mut rest := b
      let mut sameKeys := true
      for (key, _) in a do
        typeCharge limits
        match rest with
        | [] => throw "invalid_expression"
        | (other, _) :: tail =>
          rest := tail
          sameKeys := sameKeys && key == other
      if !sameKeys then return .unknown
      let mut values := b
      let mut out := []
      for (key, value) in a do
        typeCharge limits
        match values with
        | [] => throw "invalid_expression"
        | (_, other) :: tail =>
          values := tail
          out := (key, ← joinTypes fuel limits value other) :: out
      return .object (← reverseType limits out)
    | _, _ => return .unknown

def requireType (kind : Kpopper.ValueType) : ValueType → TypeM Unit
  | .unknown => pure ()
  | .scalar actual => if actual == kind then pure () else throw "type_error"
  | _ => throw "type_error"

def infer : Nat → Std.HashMap String Expr → Limits → Nat → Expr → TypeM ValueType
  | 0, _, _, _, _ => throw "depth_limit"
  | fuel + 1, nodes, limits, depth, expr => do
    if depth > limits.depth then throw "depth_limit"
    typeCharge limits
    match expr with
    | .literal value => return .scalar (Kpopper.valueType value)
    | .unavailable _ => return .unknown
    | .ref id =>
      if (← get).active.contains id then throw "cyclic_reference"
      if let some value := (← get).memo[id]? then return value
      let some body := nodes[id]? | return .unknown
      modify fun st => { st with active := st.active.insert id }
      let value ← infer fuel nodes limits (depth + 1) body
      modify fun st => { st with active := st.active.erase id, memo := st.memo.insert id value }
      return value
    | .binary op left right =>
      let a ← infer fuel nodes limits (depth + 1) left
      let b ← infer fuel nodes limits (depth + 1) right
      if op == .eq || op == .ne then
        if !(← comparableTypes (limits.depth + 2) limits a b) then throw "type_error"
        return .scalar .boolean
      else
        requireType .number a
        requireType .number b
        return .scalar (if op == .add || op == .sub || op == .mul || op == .div then .number else .boolean)
    | .logic _ left right =>
      let a ← infer fuel nodes limits (depth + 1) left
      let b ← infer fuel nodes limits (depth + 1) right
      requireType .boolean a
      requireType .boolean b
      return .scalar .boolean
    | .negate value =>
      requireType .boolean (← infer fuel nodes limits (depth + 1) value)
      return .scalar .boolean
    | .conditional condition yes no =>
      requireType .boolean (← infer fuel nodes limits (depth + 1) condition)
      let a ← infer fuel nodes limits (depth + 1) yes
      let b ← infer fuel nodes limits (depth + 1) no
      joinTypes (limits.depth + 2) limits a b
    | .array items =>
      return .array (← items.mapM (infer fuel nodes limits (depth + 1)))
    | .object fields =>
      checkTypeKeys limits fields
      return .object (← fields.mapM fun (key, value) => do
        return (key, ← infer fuel nodes limits (depth + 1) value))
    | .field record key =>
      match ← infer fuel nodes limits (depth + 1) record with
      | .unknown => return .unknown
      | .object fields =>
        for (name, value) in fields do
          typeCharge limits
          if name == key then return value
        return .unknown
      | _ => throw "type_error"

def inferType (fuel : Nat) (nodes : Std.HashMap String Expr) (limits : Limits)
    (expr : Expr) : TypeM ValueType := infer fuel nodes limits 0 expr

/-- Size addition saturates at one above the effective bound. -/
def addSize (limits : Limits) (parent child : Size) (keyBytes : Nat) : Size :=
  { nodes := min (min 10000 limits.valueNodes + 1) (parent.nodes + child.nodes)
    depth := max parent.depth (child.depth + 1)
    bytes := min (min 16777216 limits.valueBytes + 1) (parent.bytes + child.bytes + keyBytes)
    members := parent.members + 1 }

def collectArray (limits : Limits) (items : List Result) : EvalM Result := do
  let mut values := []
  let mut size : Size := {}
  let mut failed := false
  let mut missing := false
  for item in items do
    charge limits
    match item with
    | .error => failed := true
    | .unknown => missing := true
    | .known value =>
      values := value :: values
      size := addSize limits size (valueSize value) 1
  if failed then return .error
  if missing then return .unknown
  size := { size with bytes := size.bytes + 2 + (toString size.members).utf8ByteSize }
  checkSize limits size
  return .known (.array (← reverseEval limits values) size)

def collectObject (limits : Limits) (items : List (String × Result)) : EvalM Result := do
  let mut values := []
  let mut size : Size := {}
  let mut failed := false
  let mut missing := false
  for (key, item) in items do
    charge limits
    match item with
    | .error => failed := true
    | .unknown => missing := true
    | .known value =>
      values := (key, value) :: values
      size := addSize limits size (valueSize value) (2 + 2 * key.utf8ByteSize)
  if failed then return .error
  if missing then return .unknown
  size := { size with bytes := size.bytes + 2 + (toString size.members).utf8ByteSize }
  checkSize limits size
  return .known (.object (← reverseEval limits values) size)

def booleanValue : Result → Option Bool
  | .known (.scalar (.boolean value)) => some value
  | _ => none

def badBoolean : Result → Bool
  | .known (.scalar (.boolean _)) | .unknown | .error => false
  | _ => true

def evaluate : Nat → Std.HashMap String Expr → Limits → Nat → Expr → EvalM Result
  | 0, _, _, _, _ => throw "depth_limit"
  | fuel + 1, nodes, limits, depth, expr => do
    if depth > limits.depth then throw "depth_limit"
    charge limits
    match expr with
    | .literal value => checkedScalar limits value
    | .unavailable code =>
      diagnose code
      return .unknown
    | .ref id =>
      modify fun st => { st with base := { st.base with reads := st.base.reads.insert id } }
      if (← get).base.active.contains id then return ← failure "cyclic_reference"
      if let some result := (← get).memo[id]? then return result
      let some body := nodes[id]? | do
        diagnose "missing_reference"
        return .unknown
      modify fun st => { st with base := { st.base with active := st.base.active.insert id, counts := st.base.counts.insert id ((st.base.counts[id]?).getD 0 + 1) } }
      let result ← evaluate fuel nodes limits (depth + 1) body
      modify fun st => { st with memo := st.memo.insert id result, base := { st.base with active := st.base.active.erase id } }
      return result
    | .binary op left right =>
      let a ← evaluate fuel nodes limits (depth + 1) left
      let b ← evaluate fuel nodes limits (depth + 1) right
      if op == .eq || op == .ne then
        match a, b with
        | .known x, .known y =>
          match ← equalValue (limits.valueDepth + 2) limits x y with
          | none => failure "type_error"
          | some same => checkedScalar limits (.boolean (if op == .eq then same else !same))
        | _, _ => return unavailable a b
      else
        if isContainer a || isContainer b then return ← failure "type_error"
        let result ← scalarRun (Kpopper.binary op (scalarResult a) (scalarResult b) limits.toLimits)
        if let .known value := result then checkSize limits (valueSize value)
        return result
    | .logic isAnd left right =>
      let a ← evaluate fuel nodes limits (depth + 1) left
      let b ← evaluate fuel nodes limits (depth + 1) right
      let a ← if badBoolean a then failure "type_error" else pure a
      let b ← if badBoolean b then failure "type_error" else pure b
      let dominant := !isAnd
      if booleanValue a == some dominant || booleanValue b == some dominant then
        checkedScalar limits (.boolean dominant)
      else match booleanValue a, booleanValue b with
        | some x, some y => checkedScalar limits (.boolean (if isAnd then x && y else x || y))
        | _, _ => return unavailable a b
    | .negate value =>
      match ← evaluate fuel nodes limits (depth + 1) value with
      | .known (.scalar (.boolean value)) => checkedScalar limits (.boolean (!value))
      | .known _ => failure "type_error"
      | .unknown => return .unknown
      | .error => return .error
    | .conditional condition yes no =>
      match ← evaluate fuel nodes limits (depth + 1) condition with
      | .known (.scalar (.boolean true)) => evaluate fuel nodes limits (depth + 1) yes
      | .known (.scalar (.boolean false)) => evaluate fuel nodes limits (depth + 1) no
      | .known _ => failure "type_error"
      | .unknown => return .unknown
      | .error => return .error
    | .array items =>
      collectArray limits (← items.mapM (evaluate fuel nodes limits (depth + 1)))
    | .object fields =>
      checkKeys limits fields
      collectObject limits (← fields.mapM fun (key, value) => do
        return (key, ← evaluate fuel nodes limits (depth + 1) value))
    | .field record key =>
      match ← evaluate fuel nodes limits (depth + 1) record with
      | .known (.object fields _) =>
        for (name, value) in fields do
          charge limits
          if name == key then return .known value
        diagnose "missing_field"
        return .unknown
      | .known _ => failure "type_error"
      | .unknown => return .unknown
      | .error => return .error

/-- Lazy frames avoid copying a potentially oversized child list before checking
its discovery budget. A reference reserves one additional unit for output order. -/
inductive RefFrame where
  | expr (value : Expr)
  | items (values : List Expr)
  | fields (values : List (String × Expr))

def refWork : Nat → List RefFrame → List String → Except String (List String)
  | _, [], out => .ok out.reverse
  | 0, _ :: _, _ => .error "step_limit"
  | fuel + 1, frame :: rest, out =>
    match frame with
    | .items [] | .fields [] => refWork fuel rest out
    | .items (value :: tail) => refWork fuel (.expr value :: .items tail :: rest) out
    | .fields ((_, value) :: tail) => refWork fuel (.expr value :: .fields tail :: rest) out
    | .expr expr =>
      match expr with
      | .literal _ | .unavailable _ => refWork fuel rest out
      | .ref id =>
        match fuel with
        | 0 => .error "step_limit"
        | remaining + 1 => refWork remaining rest (id :: out)
      | .binary _ a b | .logic _ a b => refWork fuel (.expr a :: .expr b :: rest) out
      | .negate value | .field value _ => refWork fuel (.expr value :: rest) out
      | .conditional condition yes no => refWork fuel (.expr condition :: .expr yes :: .expr no :: rest) out
      | .array items => refWork fuel (.items items :: rest) out
      | .object fields => refWork fuel (.fields fields :: rest) out

def refs (expr : Expr) : Except String (List String) := refWork 1000000 [.expr expr] []

/-- DFS exit frames keep the active path exact without copying child lists.
Empty and exit frames do not consume an edge; every reference occurrence does,
including memoized fan-in and the back edge that establishes a cycle. -/
inductive ClosureFrame where
  | edges (values : List String)
  | exit (id : String)

def nextEdge : List ClosureFrame → Std.HashSet String →
    Option (String × List ClosureFrame × Std.HashSet String)
  | [], _ => none
  | .edges [] :: tail, active => nextEdge tail active
  | .exit id :: tail, active => nextEdge tail (active.erase id)
  | .edges (id :: tail) :: rest, active => some (id, .edges tail :: rest, active)

def closureResult (cyclic : Bool) (seen : Std.HashSet String) :
    Except (String × Std.HashSet String) (Std.HashSet String) :=
  if cyclic then .error ("cyclic_reference", seen) else .ok seen

def closureWork : Nat → Std.HashMap String Expr → List ClosureFrame →
    Std.HashSet String → Std.HashSet String → Bool →
    Except (String × Std.HashSet String) (Std.HashSet String)
  | 0, _, pending, seen, active, cyclic =>
    match nextEdge pending active with
    | none => closureResult cyclic seen
    | some _ => .error ("edge_limit", seen)
  | fuel + 1, nodes, pending, seen, active, cyclic =>
    match nextEdge pending active with
    | none => closureResult cyclic seen
    | some (id, rest, active) =>
      if active.contains id then closureWork fuel nodes rest seen active true
      else if seen.contains id then closureWork fuel nodes rest seen active cyclic
      else
        let seen := seen.insert id
        match nodes[id]? with
        | none => closureWork fuel nodes rest seen active cyclic
        | some expr =>
          match refs expr with
          | .error code => .error (code, seen)
          | .ok found => closureWork fuel nodes
              (.edges found :: .exit id :: rest) seen (active.insert id) cyclic

def closure (fuel : Nat) (nodes : Std.HashMap String Expr) (pending : List String)
    (seen : Std.HashSet String) : Except (String × Std.HashSet String) (Std.HashSet String) :=
  closureWork (min 100000 fuel) nodes [.edges pending] seen {} false

end Kpopper.Composition
