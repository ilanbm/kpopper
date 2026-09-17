import Kpopper.QueryTypes

/-! Pure, bounded query/v1 semantics over an already captured finite scope.
The kernel has no record, filesystem, clock, network, or callback access. -/
namespace Kpopper.Query

private def stringListEq (a b : List String) : Bool :=
  a.length == b.length && (a.zip b).all fun pair => pair.1 == pair.2

def strictlySorted (items : List String) : Bool := Id.run do
  let mut previous : Option String := none
  for item in items do
    if previous.any (fun before => compare before item != .lt) then return false
    previous := some item
  return true

def locationLE (a b : Location) : Bool :=
  if a.candidate != b.candidate then a.candidate ≤ b.candidate
  else if a.column != b.column then a.column ≤ b.column
  else a.phase.name ≤ b.phase.name

def diagnosticLE (a b : Diagnostic) : Bool :=
  if a.code != b.code then a.code ≤ b.code
  else if !stringListEq a.relatedIds b.relatedIds then
    let rec listLE : List String → List String → Bool
      | [], _ => true
      | _ :: _, [] => false
      | x :: xs, y :: ys => if x == y then listLE xs ys else x ≤ y
    listLE a.relatedIds b.relatedIds
  else
    let rec locationsLE : List Location → List Location → Bool
      | [], _ => true
      | _ :: _, [] => false
      | x :: xs, y :: ys => if x == y then locationsLE xs ys else locationLE x y
    locationsLE a.locations b.locations

structure EvalState where
  steps : Nat := 0
  evaluatedFieldReads : Nat := 0
  diagnostics : List Diagnostic := []
  rowUnknown : Bool := false
  rowError : Bool := false

abbrev EvalM := ExceptT String (StateM EvalState)

def charge (resources : Resources) : EvalM Unit := do
  if (← get).steps >= resources.steps then throw "step_limit"
  modify fun st => { st with steps := st.steps + 1 }

private def insertLocation (location : Location) (locations : List Location) : List Location :=
  if locations.any (· == location) then locations else (location :: locations).mergeSort locationLE

def recordDiagnostic (scopeId candidate column : String) (phase : Phase) (code : String) : EvalM Unit := do
  let related := if scopeId == candidate then [scopeId]
    else [scopeId, candidate].mergeSort (· ≤ ·)
  let location := { candidate, column, phase }
  let mut found := false
  let mut diagnostics := []
  for diagnostic in (← get).diagnostics do
    if diagnostic.code == code && stringListEq diagnostic.relatedIds related then
      found := true
      diagnostics := { diagnostic with locations := insertLocation location diagnostic.locations } :: diagnostics
    else
      diagnostics := diagnostic :: diagnostics
  if !found then diagnostics := { code, relatedIds := related, locations := [location] } :: diagnostics
  modify fun st => { st with diagnostics := diagnostics.mergeSort diagnosticLE }

def numberDigits (value : Rat) : Nat :=
  max (toString value.num.natAbs).length (toString value.den).length

def valueMeasureFuel : Nat → Value → Size
  | 0, _ => { nodes := 10001, depth := 129, bytes := 16777217 }
  | _ + 1, .scalar scalar => Kpopper.Composition.valueSize (.scalar scalar)
  | fuel + 1, .array values _ => Id.run do
    let mut nodes := 1
    let mut depth := 0
    let mut bytes := 2
    for value in values do
      let size := valueMeasureFuel fuel value
      nodes := nodes + size.nodes
      depth := max depth (size.depth + 1)
      bytes := bytes + size.bytes + 1
    return { nodes, depth, bytes, members := values.length }
  | fuel + 1, .object fields _ => Id.run do
    let mut nodes := 1
    let mut depth := 0
    let mut bytes := 2
    for (key, value) in fields do
      let size := valueMeasureFuel fuel value
      nodes := nodes + size.nodes
      depth := max depth (size.depth + 1)
      bytes := bytes + size.bytes + 2 + 2 * key.utf8ByteSize
    return { nodes, depth, bytes, members := fields.length }

def valueMeasure (value : Value) : Size := valueMeasureFuel 130 value

def valueWithin (resources : Resources) (value : Value) : Bool :=
  let size := valueMeasure value
  size.nodes ≤ min 10000 resources.valueNodes &&
    size.depth ≤ min 128 resources.valueDepth &&
    size.bytes ≤ min 16777216 resources.valueBytes

def allNumbersWithinDigitsFuel (resources : Resources) : Nat → Value → Bool
  | 0, _ => false
  | _ + 1, .scalar (.number number) => numberDigits number ≤ resources.digits
  | _ + 1, .scalar _ => true
  | fuel + 1, .array values _ => values.all (allNumbersWithinDigitsFuel resources fuel)
  | fuel + 1, .object fields _ => fields.all fun pair => allNumbersWithinDigitsFuel resources fuel pair.2

def allNumbersWithinDigits (resources : Resources) (value : Value) : Bool :=
  allNumbersWithinDigitsFuel resources 130 value

def makeArray (values : List Value) : Value :=
  let provisional := Kpopper.Composition.Value.array values {}
  .array values (valueMeasure provisional)

def makeObject (fields : List (String × Value)) : Value :=
  let provisional := Kpopper.Composition.Value.object fields {}
  .object fields (valueMeasure provisional)

def checkedValue (resources : Resources) (value : Value) : EvalM RowResult := do
  if !valueWithin resources value then throw "value_limit"
  if !allNumbersWithinDigits resources value then throw "digit_limit"
  return .known value

private def lookupField (name : String) : List (String × FieldStatus) → Option FieldStatus
  | [] => none
  | (key, status) :: tail => if key == name then some status else lookupField name tail

def rowFailure (scopeId candidate column : String) (phase : Phase) (code : String) : EvalM RowResult := do
  modify fun st => { st with rowError := true }
  recordDiagnostic scopeId candidate column phase code
  return .error

def readColumn (resources : Resources) (scopeId : String) (member : Member)
    (phase : Phase) (name : String) : EvalM RowResult := do
  modify fun st => { st with evaluatedFieldReads := st.evaluatedFieldReads + 1 }
  match lookupField name member.fields with
  | none =>
    modify fun st => { st with rowUnknown := true }
    recordDiagnostic scopeId member.id name phase "missing_column"
    return .unknown
  | some (.known value) => checkedValue resources value
  | some .missing =>
    modify fun st => { st with rowUnknown := true }
    recordDiagnostic scopeId member.id name phase "missing_column"
    return .unknown
  | some .contested =>
    modify fun st => { st with rowUnknown := true }
    recordDiagnostic scopeId member.id name phase "contested_column"
    return .unknown
  | some (.unavailable _) =>
    modify fun st => { st with rowUnknown := true }
    recordDiagnostic scopeId member.id name phase "column_type_error"
    return .unknown

def scalarOf : Value → Option Kpopper.Value
  | .scalar value => some value
  | _ => none

def boolOf : RowResult → Option Bool
  | .known (.scalar (.boolean value)) => some value
  | _ => none

def knownWrongBool : RowResult → Bool
  | .known (.scalar (.boolean _)) | .unknown | .error => false
  | .known _ => true

def numericWrong : RowResult → Bool
  | .known (.scalar (.number _)) | .unknown | .error => false
  | .known _ => true

def errorBeforeUnknown (a b : RowResult) : RowResult :=
  match a, b with
  | .error, _ | _, .error => .error
  | _, _ => .unknown

def equalValue : Nat → Resources → Value → Value → EvalM (Option Bool)
  | 0, _, _, _ => throw "depth_limit"
  | fuel + 1, resources, left, right => do
    charge resources
    match left, right with
    | .scalar a, .scalar b => return Kpopper.equalValues a b
    | .array a _, .array b _ =>
      if a.length != b.length then return some false
      let mut remaining := b
      let mut same := true
      let mut comparable := true
      for value in a do
        let other := remaining.head!
        remaining := remaining.tail!
        match ← equalValue fuel resources value other with
        | none => comparable := false
        | some result => same := same && result
      return if comparable then some same else none
    | .object a _, .object b _ =>
      if a.length != b.length then return some false
      let mut remaining := b
      let mut same := true
      let mut comparable := true
      for (key, value) in a do
        let (otherKey, other) := remaining.head!
        remaining := remaining.tail!
        if key != otherKey then return some false
        match ← equalValue fuel resources value other with
        | none => comparable := false
        | some result => same := same && result
      return if comparable then some same else none
    | _, _ => return none

def numericResult (resources : Resources) (value : Rat) : EvalM RowResult := do
  if numberDigits value > resources.digits then throw "digit_limit"
  checkedValue resources (.scalar (.number value))

def binaryResult (resources : Resources) (scopeId candidate : String) (phase : Phase)
    (op : Kpopper.Op) (a b : RowResult) : EvalM RowResult := do
  if op == .eq || op == .ne then
    match a, b with
    | .known x, .known y =>
      match ← equalValue (resources.valueDepth + 2) resources x y with
      | none => rowFailure scopeId candidate "" phase "type_error"
      | some equal => checkedValue resources (.scalar (.boolean (if op == .eq then equal else !equal)))
    | _, _ => return errorBeforeUnknown a b
  else
    if numericWrong a || numericWrong b then return ← rowFailure scopeId candidate "" phase "type_error"
    match a, b with
    | .known (.scalar (.number x)), .known (.scalar (.number y)) =>
      match op with
      | .add => numericResult resources (x + y)
      | .sub => numericResult resources (x - y)
      | .mul => numericResult resources (x * y)
      | .div =>
        if y.num == 0 then rowFailure scopeId candidate "" phase "division_by_zero"
        else numericResult resources (x / y)
      | .lt => checkedValue resources (.scalar (.boolean (x < y)))
      | .le => checkedValue resources (.scalar (.boolean (x ≤ y)))
      | .gt => checkedValue resources (.scalar (.boolean (x > y)))
      | .ge => checkedValue resources (.scalar (.boolean (x ≥ y)))
      | .eq | .ne => rowFailure scopeId candidate "" phase "type_error"
    | _, _ => return errorBeforeUnknown a b

def collectArray (resources : Resources) (items : List RowResult) : EvalM RowResult := do
  let mut values := []
  let mut failed := false
  let mut unknown := false
  for item in items do
    match item with
    | .known value => values := value :: values
    | .unknown => unknown := true
    | .error => failed := true
  if failed then return .error
  if unknown then return .unknown
  checkedValue resources (makeArray values.reverse)

def collectObject (resources : Resources) (items : List (String × RowResult)) : EvalM RowResult := do
  let mut values := []
  let mut failed := false
  let mut unknown := false
  for (key, item) in items do
    match item with
    | .known value => values := (key, value) :: values
    | .unknown => unknown := true
    | .error => failed := true
  if failed then return .error
  if unknown then return .unknown
  checkedValue resources (makeObject values.reverse)

def evaluate : Nat → Resources → String → Member → Phase → Nat → Expr → EvalM RowResult
  | 0, _, _, _, _, _, _ => throw "depth_limit"
  | fuel + 1, resources, scopeId, member, phase, depth, expr => do
    if depth > resources.depth then throw "depth_limit"
    charge resources
    match expr with
    | .column name => readColumn resources scopeId member phase name
    | .literal value => checkedValue resources (.scalar value)
    | .binary op left right =>
      let a ← evaluate fuel resources scopeId member phase (depth + 1) left
      let b ← evaluate fuel resources scopeId member phase (depth + 1) right
      binaryResult resources scopeId member.id phase op a b
    | .logic isAnd left right =>
      let a ← evaluate fuel resources scopeId member phase (depth + 1) left
      let b ← evaluate fuel resources scopeId member phase (depth + 1) right
      let a ← if knownWrongBool a then rowFailure scopeId member.id "" phase "type_error" else pure a
      let b ← if knownWrongBool b then rowFailure scopeId member.id "" phase "type_error" else pure b
      let dominant := !isAnd
      if boolOf a == some dominant || boolOf b == some dominant then
        checkedValue resources (.scalar (.boolean dominant))
      else match boolOf a, boolOf b with
        | some x, some y => checkedValue resources (.scalar (.boolean (if isAnd then x && y else x || y)))
        | _, _ => return errorBeforeUnknown a b
    | .negate value =>
      match ← evaluate fuel resources scopeId member phase (depth + 1) value with
      | .known (.scalar (.boolean result)) => checkedValue resources (.scalar (.boolean (!result)))
      | .known _ => rowFailure scopeId member.id "" phase "type_error"
      | .unknown => return .unknown
      | .error => return .error
    | .conditional condition yes no =>
      match ← evaluate fuel resources scopeId member phase (depth + 1) condition with
      | .known (.scalar (.boolean true)) => evaluate fuel resources scopeId member phase (depth + 1) yes
      | .known (.scalar (.boolean false)) => evaluate fuel resources scopeId member phase (depth + 1) no
      | .known _ => rowFailure scopeId member.id "" phase "type_error"
      | .unknown => return .unknown
      | .error => return .error
    | .array items =>
      collectArray resources (← items.mapM (evaluate fuel resources scopeId member phase (depth + 1)))
    | .object fields =>
      collectObject resources (← fields.mapM fun (key, value) => do
        return (key, ← evaluate fuel resources scopeId member phase (depth + 1) value))
    | .field record key =>
      match ← evaluate fuel resources scopeId member phase (depth + 1) record with
      | .known (.object fields _) =>
        for (name, value) in fields do
          charge resources
          if name == key then return .known value
        modify fun st => { st with rowUnknown := true }
        recordDiagnostic scopeId member.id key phase "missing_field"
        return .unknown
      | .known _ => rowFailure scopeId member.id "" phase "type_error"
      | .unknown => return .unknown
      | .error => return .error

def exprStatsFuel : Nat → Expr → Nat × Nat
  | 0, _ => (1000001, 129)
  | _ + 1, .column _ | _ + 1, .literal _ => (1, 0)
  | fuel + 1, .binary _ a b | fuel + 1, .logic _ a b =>
    let sa := exprStatsFuel fuel a; let sb := exprStatsFuel fuel b
    (1 + sa.1 + sb.1, 1 + max sa.2 sb.2)
  | fuel + 1, .negate value | fuel + 1, .field value _ =>
    let size := exprStatsFuel fuel value; (1 + size.1, 1 + size.2)
  | fuel + 1, .conditional condition yes no =>
    let a := exprStatsFuel fuel condition; let b := exprStatsFuel fuel yes; let c := exprStatsFuel fuel no
    (1 + a.1 + b.1 + c.1, 1 + max a.2 (max b.2 c.2))
  | fuel + 1, .array items =>
    items.foldl (fun acc item =>
      let size := exprStatsFuel fuel item; (acc.1 + size.1, max acc.2 (size.2 + 1))) (1, 0)
  | fuel + 1, .object fields =>
    fields.foldl (fun acc item =>
      let size := exprStatsFuel fuel item.2; (acc.1 + size.1, max acc.2 (size.2 + 1))) (1, 0)

def exprStats (expr : Expr) : Nat × Nat := exprStatsFuel 130 expr

def exprs (operation : Operation) : List Expr :=
  match operation with
  | .filter where_ | .count where_ | .all where_ | .any where_ => [where_]
  | .project value => [value]
  | .select where_ value => [where_, value]
  | .sum none value => [value]
  | .sum (some where_) value => [where_, value]

def columnsFuel : Nat → Expr → List String
  | 0, _ => []
  | _ + 1, .column name => [name]
  | _ + 1, .literal _ => []
  | fuel + 1, .binary _ a b | fuel + 1, .logic _ a b => columnsFuel fuel a ++ columnsFuel fuel b
  | fuel + 1, .negate value | fuel + 1, .field value _ => columnsFuel fuel value
  | fuel + 1, .conditional condition yes no =>
    columnsFuel fuel condition ++ columnsFuel fuel yes ++ columnsFuel fuel no
  | fuel + 1, .array items => items.flatMap (columnsFuel fuel)
  | fuel + 1, .object fields => fields.flatMap fun item => columnsFuel fuel item.2

def columns (expr : Expr) : List String := columnsFuel 130 expr

def usesArithmeticFuel : Nat → Expr → Bool
  | 0, _ => true
  | _ + 1, .binary _ _ _ => true
  | _ + 1, .column _ | _ + 1, .literal _ => false
  | fuel + 1, .logic _ a b => usesArithmeticFuel fuel a || usesArithmeticFuel fuel b
  | fuel + 1, .negate value | fuel + 1, .field value _ => usesArithmeticFuel fuel value
  | fuel + 1, .conditional condition yes no =>
    usesArithmeticFuel fuel condition || usesArithmeticFuel fuel yes || usesArithmeticFuel fuel no
  | fuel + 1, .array items => items.any (usesArithmeticFuel fuel)
  | fuel + 1, .object fields => fields.any fun item => usesArithmeticFuel fuel item.2

def usesArithmetic (expr : Expr) : Bool := usesArithmeticFuel 130 expr

def usesCompositionFuel : Nat → Expr → Bool
  | 0, _ => true
  | _ + 1, .logic _ _ _ | _ + 1, .negate _ | _ + 1, .conditional _ _ _
  | _ + 1, .array _ | _ + 1, .object _ | _ + 1, .field _ _ => true
  | fuel + 1, .binary _ a b => usesCompositionFuel fuel a || usesCompositionFuel fuel b
  | _ + 1, .column _ | _ + 1, .literal _ => false

def usesComposition (expr : Expr) : Bool := usesCompositionFuel 130 expr

def requiredModules (operation : Operation) : List String :=
  let values := exprs operation
  (if values.any usesArithmetic then ["arithmetic/v1"] else []) ++
    (if values.any usesComposition then ["composition/v1"] else []) ++ ["query/v1"]

def validateExprFuel (scopeFields : List String) : Nat → Expr → Except String Unit
  | 0, _ => .error "depth_limit"
  | _ + 1, .column name =>
    if name.isEmpty || !scopeFields.contains name then .error "invalid_expression" else .ok ()
  | _ + 1, .literal _ => .ok ()
  | fuel + 1, .binary _ a b | fuel + 1, .logic _ a b => do
    validateExprFuel scopeFields fuel a; validateExprFuel scopeFields fuel b
  | fuel + 1, .negate value | fuel + 1, .field value _ => validateExprFuel scopeFields fuel value
  | fuel + 1, .conditional condition yes no => do
    validateExprFuel scopeFields fuel condition
    validateExprFuel scopeFields fuel yes
    validateExprFuel scopeFields fuel no
  | fuel + 1, .array items => items.forM (validateExprFuel scopeFields fuel)
  | fuel + 1, .object fields => do
    if !strictlySorted (fields.map (·.1)) then throw "invalid_expression"
    fields.forM fun item => validateExprFuel scopeFields fuel item.2

def validateExpr (scopeFields : List String) (expr : Expr) : Except String Unit :=
  validateExprFuel scopeFields 130 expr

def valueTypeFuel : Nat → Value → Kpopper.Composition.ValueType
  | 0, _ => .unknown
  | _ + 1, .scalar value => .scalar (Kpopper.valueType value)
  | fuel + 1, .array values _ => .array (values.map (valueTypeFuel fuel))
  | fuel + 1, .object fields _ => .object (fields.map fun item => (item.1, valueTypeFuel fuel item.2))

def valueType (value : Value) : Kpopper.Composition.ValueType := valueTypeFuel 130 value

def sameType : Nat → Kpopper.Composition.ValueType → Kpopper.Composition.ValueType → Bool
  | 0, _, _ => false
  | _ + 1, .unknown, .unknown => true
  | _ + 1, .scalar a, .scalar b => a == b
  | fuel + 1, .array a, .array b =>
    a.length == b.length && (a.zip b).all fun pair => sameType fuel pair.1 pair.2
  | fuel + 1, .object a, .object b =>
    a.length == b.length && (a.zip b).all fun pair =>
      pair.1.1 == pair.2.1 && sameType fuel pair.1.2 pair.2.2
  | _, _, _ => false

def comparableType : Nat → Kpopper.Composition.ValueType → Kpopper.Composition.ValueType → Bool
  | 0, _, _ => false
  | _ + 1, .unknown, _ | _ + 1, _, .unknown => true
  | _ + 1, .scalar a, .scalar b => a == b
  | fuel + 1, .array a, .array b =>
    a.length != b.length || (a.zip b).all fun pair => comparableType fuel pair.1 pair.2
  | fuel + 1, .object a, .object b =>
    a.length != b.length || !(a.zip b).all (fun pair => pair.1.1 == pair.2.1) ||
      (a.zip b).all fun pair => comparableType fuel pair.1.2 pair.2.2
  | _, _, _ => false

private def fieldType (member : Member) (name : String) : Kpopper.Composition.ValueType :=
  match lookupField name member.fields with
  | some (.known value) => valueType value
  | _ => .unknown

def requireScalarType (expected : Kpopper.ValueType) : Kpopper.Composition.ValueType → Except String Unit
  | .unknown => .ok ()
  | .scalar actual => if actual == expected then .ok () else .error "type_error"
  | _ => .error "type_error"

def inferType : Nat → Member → Nat → Expr → Except String Kpopper.Composition.ValueType
  | 0, _, _, _ => .error "depth_limit"
  | fuel + 1, member, depth, expr => do
    match expr with
    | .column name => return fieldType member name
    | .literal value => return .scalar (Kpopper.valueType value)
    | .binary op left right =>
      let a ← inferType fuel member (depth + 1) left
      let b ← inferType fuel member (depth + 1) right
      if op == .eq || op == .ne then
        if !comparableType (depth + fuel + 2) a b then throw "type_error"
        return .scalar .boolean
      requireScalarType .number a
      requireScalarType .number b
      return .scalar (if op == .add || op == .sub || op == .mul || op == .div then .number else .boolean)
    | .logic _ left right =>
      requireScalarType .boolean (← inferType fuel member (depth + 1) left)
      requireScalarType .boolean (← inferType fuel member (depth + 1) right)
      return .scalar .boolean
    | .negate value =>
      requireScalarType .boolean (← inferType fuel member (depth + 1) value)
      return .scalar .boolean
    | .conditional condition yes no =>
      requireScalarType .boolean (← inferType fuel member (depth + 1) condition)
      let a ← inferType fuel member (depth + 1) yes
      let b ← inferType fuel member (depth + 1) no
      return if sameType (depth + fuel + 2) a b then a else .unknown
    | .array items => return .array (← items.mapM (inferType fuel member (depth + 1)))
    | .object fields => return .object (← fields.mapM fun item => do
      return (item.1, ← inferType fuel member (depth + 1) item.2))
    | .field record key =>
      match ← inferType fuel member (depth + 1) record with
      | .unknown => return .unknown
      | .object fields =>
        for (name, value) in fields do if name == key then return value
        return .unknown
      | _ => throw "type_error"

def checkOperationTypes (scope : Scope) (operation : Operation) : Except String Unit := do
  let members := if scope.members.isEmpty then [{ id := "", fields := scope.fields.map fun key => (key, .missing) }] else scope.members
  for member in members do
    match operation with
    | .filter where_ | .count where_ | .all where_ | .any where_ =>
      requireScalarType .boolean (← inferType 130 member 0 where_)
    | .project value => let _ ← inferType 130 member 0 value
    | .select where_ value =>
      requireScalarType .boolean (← inferType 130 member 0 where_)
      let _ ← inferType 130 member 0 value
    | .sum where_ value =>
      if let some predicate := where_ then
        requireScalarType .boolean (← inferType 130 member 0 predicate)
      requireScalarType .number (← inferType 130 member 0 value)

def operationNodeCount (operation : Operation) : Nat :=
  (exprs operation).foldl (fun total expr => total + (exprStats expr).1) 0

def operationMaxDepth (operation : Operation) : Nat :=
  (exprs operation).foldl (fun depth expr => max depth (exprStats expr).2) 0

def estimateExprNodesFuel (member : Member) : Nat → Expr → Nat
  | 0, _ => 10001
  | _ + 1, .column name =>
    match lookupField name member.fields with
    | some (.known value) => (valueMeasure value).nodes
    | _ => 1
  | _ + 1, .literal _ => 1
  | _ + 1, .binary _ _ _ | _ + 1, .logic _ _ _ | _ + 1, .negate _ => 1
  | fuel + 1, .conditional _ yes no =>
    max (estimateExprNodesFuel member fuel yes) (estimateExprNodesFuel member fuel no)
  | fuel + 1, .array items =>
    1 + items.foldl (fun total item => total + estimateExprNodesFuel member fuel item) 0
  | fuel + 1, .object fields =>
    1 + fields.foldl (fun total item => total + estimateExprNodesFuel member fuel item.2) 0
  | fuel + 1, .field record _ => estimateExprNodesFuel member fuel record

def estimateExprNodes (member : Member) (expr : Expr) : Nat :=
  estimateExprNodesFuel member 130 expr

structure ValueUpper where
  size : Size := {}
  digits : Nat := 0

structure ExprUpper where
  output : ValueUpper := {}
  peak : ValueUpper := {}

def maxSize (a b : Size) : Size := {
  nodes := max a.nodes b.nodes
  depth := max a.depth b.depth
  bytes := max a.bytes b.bytes
  members := max a.members b.members }

def maxUpper (a b : ValueUpper) : ValueUpper := {
  size := maxSize a.size b.size, digits := max a.digits b.digits }

def maxNumberDigitsFuel : Nat → Value → Nat
  | 0, _ => 257
  | _ + 1, .scalar (.number value) => numberDigits value
  | _ + 1, .scalar _ => 0
  | fuel + 1, .array values _ =>
      values.foldl (fun result value => max result (maxNumberDigitsFuel fuel value)) 0
  | fuel + 1, .object fields _ =>
      fields.foldl (fun result item => max result (maxNumberDigitsFuel fuel item.2)) 0

def upperOfValue (value : Value) : ValueUpper := {
  size := valueMeasure value, digits := maxNumberDigitsFuel 130 value }

def boolUpper : ValueUpper := { size := { nodes := 1, bytes := 3 } }

def nullUpper : ValueUpper := { size := { nodes := 1, bytes := 1 } }

def numberUpper (digits : Nat) : ValueUpper := {
  size := { nodes := 1, bytes := 4 + 2 * digits }, digits }

def textUpper (value : String) : ValueUpper := {
  size := { nodes := 1, bytes := 2 + 2 * value.utf8ByteSize } }

def arrayUpper (values : List ValueUpper) : ValueUpper := Id.run do
  let mut size : Size := { nodes := 1, bytes := 2, members := values.length }
  let mut digits := 0
  for value in values do
    let child := value.size
    size := { nodes := size.nodes + child.nodes, depth := max size.depth (child.depth + 1), bytes := size.bytes + child.bytes + 1, members := size.members }
    digits := max digits value.digits
  return { size, digits }

def objectUpper (fields : List (String × ValueUpper)) : ValueUpper := Id.run do
  let mut size : Size := { nodes := 1, bytes := 2, members := fields.length }
  let mut digits := 0
  for (key, value) in fields do
    let child := value.size
    size := { nodes := size.nodes + child.nodes, depth := max size.depth (child.depth + 1), bytes := size.bytes + child.bytes + 2 + 2 * key.utf8ByteSize, members := size.members }
    digits := max digits value.digits
  return { size, digits }

def exprUpperFuel (member : Member) : Nat → Expr → ExprUpper
  | 0, _ =>
      let value : ValueUpper := {
        size := { nodes := 10001, depth := 129, bytes := 16777217 }
        digits := 257 }
      { output := value, peak := value }
  | _ + 1, .column name =>
      let value := match lookupField name member.fields with
        | some (.known value) => upperOfValue value
        | _ => ({} : ValueUpper)
      { output := value, peak := value }
  | _ + 1, .literal value =>
      let value := upperOfValue (.scalar value)
      { output := value, peak := value }
  | fuel + 1, .binary op left right =>
      let a := exprUpperFuel member fuel left
      let b := exprUpperFuel member fuel right
      let digits := if op == .add || op == .sub then a.output.digits + b.output.digits + 1
        else a.output.digits + b.output.digits
      let output := if op == .add || op == .sub || op == .mul || op == .div
        then numberUpper digits else boolUpper
      { output, peak := maxUpper output (maxUpper a.peak b.peak) }
  | fuel + 1, .logic _ left right =>
      let a := exprUpperFuel member fuel left
      let b := exprUpperFuel member fuel right
      { output := boolUpper, peak := maxUpper boolUpper (maxUpper a.peak b.peak) }
  | fuel + 1, .negate value =>
      let child := exprUpperFuel member fuel value
      { output := boolUpper, peak := maxUpper boolUpper child.peak }
  | fuel + 1, .conditional condition yes no =>
      let a := exprUpperFuel member fuel condition
      let b := exprUpperFuel member fuel yes
      let c := exprUpperFuel member fuel no
      let output := maxUpper b.output c.output
      { output, peak := maxUpper output (maxUpper a.peak (maxUpper b.peak c.peak)) }
  | fuel + 1, .array items =>
      let values := items.map (exprUpperFuel member fuel)
      let output := arrayUpper (values.map (·.output))
      { output, peak := values.foldl (fun result value => maxUpper result value.peak) output }
  | fuel + 1, .object fields =>
      let values := fields.map fun item => (item.1, exprUpperFuel member fuel item.2)
      let output := objectUpper (values.map fun item => (item.1, item.2.output))
      { output, peak := values.foldl (fun result item => maxUpper result item.2.peak) output }
  | fuel + 1, .field value _ =>
      let child := exprUpperFuel member fuel value
      { output := child.output, peak := child.peak }

def exprUpper (member : Member) (expr : Expr) : ExprUpper :=
  exprUpperFuel member 130 expr

def valueExpr? : Operation → Option Expr
  | .project value | .select _ value | .sum _ value => some value
  | _ => none

def resultResourceUpper (scope : Scope) (operation : Operation) : ExprUpper := Id.run do
  let synthetic : Member := { id := "", fields := scope.fields.map fun key => (key, .missing) }
  let analysis := if scope.members.isEmpty then [synthetic] else scope.members
  let mut peak : ValueUpper := {}
  let mut outputs : List ValueUpper := []
  for member in analysis do
    for expression in exprs operation do
      let upper := exprUpper member expression
      peak := maxUpper peak upper.peak
    if !scope.members.isEmpty then
      if let some expression := valueExpr? operation then
        outputs := outputs ++ [(exprUpper member expression).output]
  let digits := (toString scope.members.length).length
  let counter := numberUpper digits
  let raw := match operation with
    | .filter _ => arrayUpper (scope.members.map fun member => textUpper member.id)
    | .project _ | .select _ _ => arrayUpper outputs
    | .sum _ _ =>
        let sumDigits := max 1 (outputs.foldl (fun total value => total + value.digits + 1) 0)
        numberUpper sumDigits
    | .count _ => counter
    | .all _ | .any _ => boolUpper
  let result := objectUpper [
    ("definite_match_count", counter), ("error_count", counter), ("input_count", counter),
    ("result", raw), ("unknown_membership_count", counter), ("unknown_value_count", counter)]
  return { output := result, peak := maxUpper peak result }

def resultNodeUpperBound (scope : Scope) (operation : Operation) : Nat :=
  let raw := match operation with
    | .filter _ => 1 + scope.members.length
    | .project value | .select _ value =>
      1 + scope.members.foldl (fun total member => total + estimateExprNodes member value) 0
    | .count _ | .sum _ _ | .all _ | .any _ => 1
  6 + raw

/-- Conservative execution work for one row. It includes every executed AST
node, the largest conditional branch, container equality traversal, and the
largest literal-key record scan. -/
def exprExecutionUpperFuel (member : Member) : Nat → Expr → Nat
  | 0, _ => 1000001
  | _ + 1, .column _ | _ + 1, .literal _ => 1
  | fuel + 1, .binary op left right =>
    1 + exprExecutionUpperFuel member fuel left + exprExecutionUpperFuel member fuel right +
      (if op == .eq || op == .ne then
        estimateExprNodesFuel member fuel left + estimateExprNodesFuel member fuel right
       else 0)
  | fuel + 1, .logic _ left right =>
    1 + exprExecutionUpperFuel member fuel left + exprExecutionUpperFuel member fuel right
  | fuel + 1, .negate value => 1 + exprExecutionUpperFuel member fuel value
  | fuel + 1, .conditional condition yes no =>
    1 + exprExecutionUpperFuel member fuel condition +
      max (exprExecutionUpperFuel member fuel yes) (exprExecutionUpperFuel member fuel no)
  | fuel + 1, .array items =>
    1 + items.foldl (fun total item => total + exprExecutionUpperFuel member fuel item) 0
  | fuel + 1, .object fields =>
    1 + fields.foldl (fun total item => total + exprExecutionUpperFuel member fuel item.2) 0
  | fuel + 1, .field record _ =>
    1 + exprExecutionUpperFuel member fuel record + estimateExprNodesFuel member fuel record

def exprExecutionUpper (member : Member) (expr : Expr) : Nat :=
  exprExecutionUpperFuel member 130 expr

def operationExecutionUpper (scope : Scope) (operation : Operation) : Nat :=
  scope.members.foldl (fun total member => total + 1 + match operation with
    | .filter where_ | .count where_ | .all where_ | .any where_ =>
      exprExecutionUpper member where_
    | .project value => exprExecutionUpper member value
    | .select where_ value => exprExecutionUpper member where_ + exprExecutionUpper member value
    | .sum none value => exprExecutionUpper member value
    | .sum (some where_) value => exprExecutionUpper member where_ + exprExecutionUpper member value) 0

def validateCapturedValues (resources : Resources) (scope : Scope) : Except String Unit := do
  for member in scope.members do
    for (_, status) in member.fields do
      if let .known value := status then
        if !valueWithin resources value then throw "value_limit"
        if !allNumbersWithinDigits resources value then throw "digit_limit"

def prepare (resources : Resources) (scope : Scope) (operation : Operation) : Except String Prepared := do
  if scope.id.isEmpty || scope.witness.scopeId != scope.id then throw "invalid_expression"
  if !strictlySorted scope.fields || scope.fields.any String.isEmpty then throw "invalid_expression"
  if !strictlySorted (scope.members.map (·.id)) || scope.members.any fun member => member.id.isEmpty then
    throw "invalid_expression"
  for member in scope.members do
    if !stringListEq (member.fields.map (·.1)) scope.fields then throw "invalid_expression"
  for expr in exprs operation do validateExpr scope.fields expr
  if operationMaxDepth operation > resources.depth then throw "depth_limit"
  if scope.members.length > min 10000 resources.candidates then throw "candidate_limit"
  let fieldReads := scope.members.length * scope.fields.length
  if fieldReads > min 100000 resources.fieldReads then throw "field_read_limit"
  validateCapturedValues resources scope
  checkOperationTypes scope operation
  let nodes := operationNodeCount operation
  let edges := if nodes == 0 then 0 else nodes - (exprs operation).length
  let preflightSteps := nodes + edges + fieldReads
  let executionBound := operationExecutionUpper scope operation
  if preflightSteps > resources.steps || executionBound > resources.steps then throw "step_limit"
  let upper := resultResourceUpper scope operation
  if upper.peak.size.nodes > min 10000 resources.valueNodes ||
      upper.peak.size.depth > min 128 resources.valueDepth ||
      upper.peak.size.bytes > min 16777216 resources.valueBytes then throw "value_limit"
  if upper.peak.digits > resources.digits then throw "digit_limit"
  return { preflightSteps, candidates := scope.members.length, fieldReads }

private def classifyWhere (scopeId : String) (member : Member) (result : RowResult) : EvalM Unit :=
  match result with
  | .unknown => recordDiagnostic scopeId member.id "$expression" .where_ "where_unknown"
  | .error => recordDiagnostic scopeId member.id "$expression" .where_ "where_error"
  | .known _ => pure ()

private def classifyValue (scopeId : String) (member : Member) (result : RowResult) : EvalM Unit :=
  match result with
  | .unknown | .error => recordDiagnostic scopeId member.id "$expression" .value "value_error"
  | .known _ => pure ()

def evaluateTracked (resources : Resources) (scopeId : String) (member : Member)
    (phase : Phase) (expr : Expr) : EvalM (RowResult × Bool × Bool) := do
  modify fun st => { st with rowUnknown := false, rowError := false }
  let result ← evaluate (resources.depth + 1) resources scopeId member phase 0 expr
  let st ← get
  return (result, st.rowUnknown, st.rowError)

private def rowIsUnknown : RowResult → Bool
  | .unknown => true
  | _ => false

private def rowIsError : RowResult → Bool
  | .error => true
  | _ => false

private def whereTruth : RowResult → Option Bool
  | .known (.scalar (.boolean value)) => some value
  | _ => none

private def summarizeWhere (scopeId : String) (member : Member)
    (unknown error : Bool) : EvalM Unit := do
  if unknown then recordDiagnostic scopeId member.id "$expression" .where_ "where_unknown"
  if error then recordDiagnostic scopeId member.id "$expression" .where_ "where_error"

private def summarizeValue (scopeId : String) (member : Member)
    (unknown error : Bool) : EvalM Unit := do
  if unknown || error then recordDiagnostic scopeId member.id "$expression" .value "value_error"

def natValue (value : Nat) : Value := .scalar (.number (mkRat (Int.ofNat value) 1))

def resultRecord (counts : QueryCounts) (result : Value) : Value :=
  makeObject [
    ("definite_match_count", natValue counts.definiteMatchCount),
    ("error_count", natValue counts.errorCount),
    ("input_count", natValue counts.inputCount),
    ("result", result),
    ("unknown_membership_count", natValue counts.unknownMembershipCount),
    ("unknown_value_count", natValue counts.unknownValueCount)]

def execute (resources : Resources) (scope : Scope) (operation : Operation) : Except String Completed := do
  let action : EvalM (Status × Option Value × QueryCounts) := do
    let mut counts : QueryCounts := { inputCount := scope.members.length }
    let mut values : List Value := []
    let mut total : Rat := 0
    let mut dominated := false
    for member in scope.members do
      match operation with
      | .filter where_ | .count where_ =>
        let (membership, taintedUnknown, taintedError) ←
          evaluateTracked resources scope.id member .where_ where_
        charge resources
        let badKnown := match membership with
          | .known (.scalar (.boolean _)) | .unknown | .error => false
          | .known _ => true
        let unknown := taintedUnknown || rowIsUnknown membership
        let error := taintedError || rowIsError membership || badKnown
        if unknown then counts := { counts with unknownMembershipCount := counts.unknownMembershipCount + 1 }
        if error then counts := { counts with errorCount := counts.errorCount + 1 }
        summarizeWhere scope.id member unknown error
        if whereTruth membership == some true then
          counts := { counts with definiteMatchCount := counts.definiteMatchCount + 1 }
          if let .filter _ := operation then values := (.scalar (.text member.id)) :: values
      | .project value =>
        counts := { counts with definiteMatchCount := counts.definiteMatchCount + 1 }
        let (projected, taintedUnknown, taintedError) ←
          evaluateTracked resources scope.id member .value value
        charge resources
        let unknown := taintedUnknown || rowIsUnknown projected
        let error := taintedError || rowIsError projected
        if unknown then counts := { counts with unknownValueCount := counts.unknownValueCount + 1 }
        if error then counts := { counts with errorCount := counts.errorCount + 1 }
        summarizeValue scope.id member unknown error
        if let .known result := projected then values := result :: values
      | .select where_ value =>
        let (membership, whereTaintedUnknown, whereTaintedError) ←
          evaluateTracked resources scope.id member .where_ where_
        charge resources
        let badKnown := match membership with
          | .known (.scalar (.boolean _)) | .unknown | .error => false
          | .known _ => true
        let whereUnknown := whereTaintedUnknown || rowIsUnknown membership
        let whereError := whereTaintedError || rowIsError membership || badKnown
        if whereUnknown then counts := { counts with unknownMembershipCount := counts.unknownMembershipCount + 1 }
        summarizeWhere scope.id member whereUnknown whereError
        let mut valueError := false
        if whereTruth membership == some true then
          counts := { counts with definiteMatchCount := counts.definiteMatchCount + 1 }
          let (projected, valueTaintedUnknown, valueTaintedError) ←
            evaluateTracked resources scope.id member .value value
          let valueUnknown := valueTaintedUnknown || rowIsUnknown projected
          valueError := valueTaintedError || rowIsError projected
          if valueUnknown then counts := { counts with unknownValueCount := counts.unknownValueCount + 1 }
          summarizeValue scope.id member valueUnknown valueError
          if let .known result := projected then values := result :: values
        if whereError || valueError then
          counts := { counts with errorCount := counts.errorCount + 1 }
      | .sum where_ value =>
        let (membership, whereTaintedUnknown, whereTaintedError) ← match where_ with
          | none => pure (.known (.scalar (.boolean true)), false, false)
          | some predicate => evaluateTracked resources scope.id member .where_ predicate
        charge resources
        let badKnown := match membership with
          | .known (.scalar (.boolean _)) | .unknown | .error => false
          | .known _ => true
        let whereUnknown := whereTaintedUnknown || rowIsUnknown membership
        let whereError := whereTaintedError || rowIsError membership || badKnown
        if whereUnknown then counts := { counts with unknownMembershipCount := counts.unknownMembershipCount + 1 }
        summarizeWhere scope.id member whereUnknown whereError
        let mut valueError := false
        if whereTruth membership == some true then
          counts := { counts with definiteMatchCount := counts.definiteMatchCount + 1 }
          let (projected, valueTaintedUnknown, valueTaintedError) ←
            evaluateTracked resources scope.id member .value value
          let valueUnknown := valueTaintedUnknown || rowIsUnknown projected
          valueError := valueTaintedError || rowIsError projected
          match projected with
          | .known (.scalar (.number number)) => total := total + number
          | .known _ =>
            valueError := true
            recordDiagnostic scope.id member.id "$expression" .value "type_error"
          | _ => pure ()
          if valueUnknown then counts := { counts with unknownValueCount := counts.unknownValueCount + 1 }
          summarizeValue scope.id member valueUnknown valueError
        if whereError || valueError then
          counts := { counts with errorCount := counts.errorCount + 1 }
      | .all where_ | .any where_ =>
        let (membership, taintedUnknown, taintedError) ←
          evaluateTracked resources scope.id member .where_ where_
        charge resources
        let badKnown := match membership with
          | .known (.scalar (.boolean _)) | .unknown | .error => false
          | .known _ => true
        let unknown := taintedUnknown || rowIsUnknown membership
        let error := taintedError || rowIsError membership || badKnown
        if unknown then counts := { counts with unknownMembershipCount := counts.unknownMembershipCount + 1 }
        if error then counts := { counts with errorCount := counts.errorCount + 1 }
        summarizeWhere scope.id member unknown error
        if let some value := whereTruth membership then
          if value then counts := { counts with definiteMatchCount := counts.definiteMatchCount + 1 }
          if (match operation with | .all _ => !value | .any _ => value | _ => false) then dominated := true
    let status :=
      if dominated then .ok
      else if counts.errorCount > 0 then .error
      else if counts.unknownMembershipCount > 0 || counts.unknownValueCount > 0 then .unknown
      else .ok
    let rawResult : Value := match operation with
      | .filter _ | .project _ | .select _ _ => makeArray values.reverse
      | .count _ => natValue counts.definiteMatchCount
      | .sum _ _ => .scalar (.number total)
      | .all _ => .scalar (.boolean (counts.inputCount == counts.definiteMatchCount))
      | .any _ => .scalar (.boolean (counts.definiteMatchCount > 0))
    let result := if status == .ok then some (resultRecord counts rawResult) else none
    if let some value := result then
      if !valueWithin resources value then throw "value_limit"
      if !allNumbersWithinDigits resources value then throw "digit_limit"
    return (status, result, counts)
  let (outcome, state) := action.run {}
  match outcome with
  | .error code => .error code
  | .ok (status, result, counts) => .ok {
      status, result, counts, diagnostics := state.diagnostics,
      steps := state.steps, evaluatedFieldReads := state.evaluatedFieldReads }

end Kpopper.Query
