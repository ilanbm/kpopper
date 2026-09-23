import Protocol
import Composition

/-! KP3/KR3 is additive. No composition tag changes the closed KP2 grammar. -/
namespace Kpopper.Composition

open Kpopper (ParseM takeToken takeNat takeBounded takeText parseNumber parseOp
  unavailableCodes sorted hex)

def takeKey : ParseM String := do
  let key ← takeText
  if key.length > 500 then throw "invalid_expression"
  return key

def takeId : ParseM String := do
  let id ← takeKey
  if id.isEmpty then throw "invalid_expression"
  return id

def parseExpr3 : Nat → ParseM Expr
  | 0 => throw "parser_depth_limit"
  | fuel + 1 => do
    if (← get).expressions >= 1000000 then throw "expression_limit"
    modify fun st => { st with expressions := st.expressions + 1 }
    match ← takeToken with
    | "n" =>
      match parseNumber (← takeToken) with
      | .ok value => return .literal (.number value)
      | .error code => throw code
    | "b" =>
      match ← takeToken with
      | "0" => return .literal (.boolean false)
      | "1" => return .literal (.boolean true)
      | _ => throw "invalid_expression"
    | "s" => return .literal (.text (← takeText))
    | "z" => return .literal .null
    | "r" => return .ref (← takeId)
    | "u" =>
      let code ← takeToken
      if !unavailableCodes.contains code then throw "invalid_expression"
      return .unavailable code
    | "o" =>
      let name ← takeToken
      if name == "not" then
        if (← takeNat 2) != 1 then throw "invalid_expression"
        return .negate (← parseExpr3 fuel)
      let op ← if name == "and" || name == "or" then pure none else
        match parseOp name with
        | .ok op => pure (some op)
        | .error code => throw code
      if (← takeNat 2) != 2 then throw "invalid_expression"
      let left ← parseExpr3 fuel
      let right ← parseExpr3 fuel
      match op with
      | some op => return .binary op left right
      | none => return .logic (name == "and") left right
    | "i" =>
      let condition ← parseExpr3 fuel
      let yes ← parseExpr3 fuel
      let no ← parseExpr3 fuel
      return .conditional condition yes no
    | "l" =>
      let count ← takeBounded 10000 "collection_limit"
      let mut values := []
      for _ in [:count] do values := (← parseExpr3 fuel) :: values
      return .array values.reverse
    | "m" =>
      let count ← takeBounded 10000 "collection_limit"
      let mut fields := []
      let mut previous : Option String := none
      for _ in [:count] do
        let key ← takeKey
        if previous.any (fun old => key ≤ old) then throw "invalid_expression"
        previous := some key
        fields := (key, ← parseExpr3 fuel) :: fields
      return .object fields.reverse
    | "f" =>
      let key ← takeKey
      return .field (← parseExpr3 fuel) key
    | _ => throw "invalid_expression"

structure Request where
  limits : Limits
  declared : Std.HashSet String
  nodes : Std.HashMap String Expr
  root : Expr

def parseRequest3 : ParseM Request := do
  if (← takeToken) != "KP3" then throw "invalid_transport"
  let steps ← takeBounded 10000000 "invalid_limits"
  let depth ← takeBounded 4096 "invalid_limits"
  let digits ← takeBounded 4096 "invalid_limits"
  let valueNodes ← takeBounded 10000 "invalid_limits"
  let valueDepth ← takeBounded 128 "invalid_limits"
  let valueBytes ← takeBounded 16777216 "invalid_limits"
  if steps == 0 || depth == 0 || digits == 0 || valueNodes == 0 || valueDepth == 0 || valueBytes == 0 then
    throw "invalid_limits"
  let declaredCount ← takeBounded 100000 "edge_limit"
  let mut declared : Std.HashSet String := {}
  for _ in [:declaredCount] do
    let id ← takeId
    if declared.contains id then throw "invalid_transport"
    declared := declared.insert id
  let nodeCount ← takeBounded 20000 "node_limit"
  let mut nodes : Std.HashMap String Expr := {}
  for _ in [:nodeCount] do
    let id ← takeId
    if nodes.contains id then throw "invalid_transport"
    nodes := nodes.insert id (← parseExpr3 129)
  let root ← parseExpr3 129
  if (← get).pos != (← get).tokens.size then throw "invalid_transport"
  return { limits := { steps, depth, digits, valueNodes, valueDepth, valueBytes }, declared, nodes, root }

structure OutputState where
  nodes : Nat := 0
  bytes : Nat := 0
  tokens : List String := []

abbrev OutputM := ExceptT String (StateM OutputState)

def outputToken (limits : Limits) (token : String) : OutputM Unit := do
  let st ← get
  let bytes := st.bytes + token.utf8ByteSize + (if st.tokens.isEmpty then 0 else 1)
  if bytes > limits.valueBytes then throw "collection_limit"
  set { st with bytes, tokens := token :: st.tokens }

def outputText (limits : Limits) (value : String) : OutputM Unit := do
  let st ← get
  if st.bytes + 2 * value.utf8ByteSize + (if st.tokens.isEmpty then 0 else 1) > limits.valueBytes then
    throw "collection_limit"
  outputToken limits (hex value)

def memberCount {α : Type} (limits : Limits) (items : List α) : OutputM Nat := do
  let remaining := limits.valueNodes - (← get).nodes
  let mut count := 0
  for _ in items do
    if count >= remaining then throw "collection_limit"
    count := count + 1
  return count

def outputValue : Nat → Limits → Value → OutputM Unit
  | 0, _, _ => throw "collection_limit"
  | fuel + 1, limits, value => do
    let st ← get
    if st.nodes >= limits.valueNodes then throw "collection_limit"
    set { st with nodes := st.nodes + 1 }
    match value with
    | .scalar (.text text) =>
      outputToken limits "s"
      outputText limits text
    | .scalar scalar =>
      for token in Kpopper.valueTokens scalar do outputToken limits token
    | .array values _ =>
      let count ← memberCount limits values
      outputToken limits "l"
      outputToken limits (toString count)
      for child in values do outputValue fuel limits child
    | .object fields _ =>
      let count ← memberCount limits fields
      outputToken limits "m"
      outputToken limits (toString count)
      let mut previous : Option String := none
      for (key, child) in fields do
        if key.length > 500 || previous.any (fun old => key ≤ old) then throw "invalid_expression"
        previous := some key
        outputText limits key
        outputValue fuel limits child

def valueTokens3 (limits : Limits) (value : Value) : Except String (List String) :=
  let (result, state) := ((outputValue (limits.valueDepth + 1) limits value).run).run {}
  match result with
  | .ok () => .ok state.tokens.reverse
  | .error code => .error code

def response3 (status : String) (value : List String) (potential : Std.HashSet String)
    (preflightSteps : Nat) (st : Kpopper.State) : String :=
  let diagnostics := sorted st.diagnostics
  let potential := sorted potential
  let reads := sorted st.reads
  let counts := st.counts.toList.mergeSort (fun a b => a.1 ≤ b.1)
  String.intercalate "\t" <| ["KR3", status] ++ value ++
    [toString diagnostics.length] ++ diagnostics ++ [toString potential.length] ++ potential.map hex ++
    [toString reads.length] ++ reads.map hex ++
    [toString st.steps, toString preflightSteps, toString counts.length] ++
    counts.flatMap (fun (id, count) => [hex id, toString count])

def errorStatus3 (code : String) : String :=
  if code == "collection_limit" then "limit" else Kpopper.errorStatus code

def errorResponse3 (code : String) (potential : Std.HashSet String := {})
    (preflightSteps : Nat := 0) : String :=
  response3 (errorStatus3 code) ["u"] potential preflightSteps
    { diagnostics := ({} : Std.HashSet String).insert code }

def handle3 (line : String) : String := Id.run do
  if line.utf8ByteSize > 16777216 then return errorResponse3 "transport_limit"
  if line.contains '\n' || line.contains '\r' then return errorResponse3 "invalid_transport"
  let (parsed, _) := (parseRequest3.run).run { tokens := (line.splitOn "\t").toArray }
  let request ← match parsed with
    | .ok request => pure request
    | .error code => return errorResponse3 code
  let roots ← match refs request.root with
    | .ok roots => pure roots
    | .error code => return errorResponse3 code
  let potential ← match closure 100000 request.nodes roots {} with
    | .ok potential => pure potential
    | .error (code, potential) => return errorResponse3 code potential
  if !(potential.toList.all request.declared.contains) then return errorResponse3 "undeclared_dependency" potential
  let (typed, types) := ((inferType (request.limits.depth + 1) request.nodes request.limits request.root).run).run {}
  if let .error code := typed then return errorResponse3 code potential types.visits
  let (result, state) := ((evaluate (request.limits.depth + 1) request.nodes request.limits 0 request.root).run).run {}
  let st := state.base
  if !(st.reads.toList.all potential.contains) then return errorResponse3 "undeclared_dependency" potential types.visits
  match result with
  | .error code =>
    return response3 (errorStatus3 code) ["u"] potential types.visits { st with diagnostics := st.diagnostics.insert code }
  | .ok (.known value) =>
    match valueTokens3 request.limits value with
    | .ok tokens => return response3 "ok" tokens potential types.visits st
    | .error code =>
      return response3 (errorStatus3 code) ["u"] potential types.visits { st with diagnostics := st.diagnostics.insert code }
  | .ok .unknown => return response3 "unknown" ["u"] potential types.visits st
  | .ok .error => return response3 "error" ["u"] potential types.visits st

end Kpopper.Composition
