import Kernel
import Init.System.IO

namespace Kpopper

structure Cursor where
  tokens : Array String
  pos : Nat := 0
  expressions : Nat := 0

abbrev ParseM := ExceptT String (StateM Cursor)

def takeToken : ParseM String := do
  let st ← get
  let some token := st.tokens[st.pos]? | throw "invalid_transport"
  set { st with pos := st.pos + 1 }
  return token

def digit (c : Char) : Bool := c >= '0' && c <= '9'

def natural (token : String) (bound : Nat) : Except String Nat := do
  if token.isEmpty || token.length > 8 || !token.toList.all digit then throw "invalid_transport"
  let some n := token.toNat? | throw "invalid_transport"
  if toString n != token || n > bound then throw "invalid_transport"
  return n

def takeNat (bound : Nat) : ParseM Nat := do
  match natural (← takeToken) bound with
  | .ok n => return n
  | .error code => throw code

def takeBounded (bound : Nat) (code : String) : ParseM Nat := do
  let n ← takeNat 99999999
  if n > bound then throw code
  return n

def nibble (c : Char) : Option Nat :=
  if c >= '0' && c <= '9' then some (c.toNat - 48)
  else if c >= 'a' && c <= 'f' then some (c.toNat - 87)
  else none

def unhex (s : String) : Except String String := do
  let chars := s.toList.toArray
  if chars.size % 2 != 0 then throw "invalid_transport"
  let mut bytes := ByteArray.empty
  for i in [:chars.size / 2] do
    let some a := nibble chars[2 * i]! | throw "invalid_transport"
    let some b := nibble chars[2 * i + 1]! | throw "invalid_transport"
    bytes := bytes.push (UInt8.ofNat (a * 16 + b))
  let some result := String.fromUTF8? bytes | throw "invalid_transport"
  return result

def hex (s : String) : String := Id.run do
  let alphabet := "0123456789abcdef".toList.toArray
  let mut result := ""
  for byte in s.toUTF8 do
    result := result.push alphabet[byte.toNat / 16]!
    result := result.push alphabet[byte.toNat % 16]!
  return result

def takeText : ParseM String := do
  match unhex (← takeToken) with
  | .ok text => return text
  | .error code => throw code

/-- Decimal lexemes are interpreted exactly. Exponents and mantissas are
bounded before big integers/powers are constructed. -/
def parseNumber (s : String) : Except String Rat := do
  if s.isEmpty then throw "invalid_expression"
  if s.length > 8200 then throw "number_limit"
  let parts := (s.replace "E" "e").splitOn "e"
  if parts.length > 2 then throw "invalid_expression"
  let mut exponent : Int := 0
  if parts.length == 2 then
    let es := parts[1]!.toList
    let ds := match es with | '+' :: tail => tail | '-' :: tail => tail | _ => es
    if ds.isEmpty || !ds.all digit then throw "invalid_expression"
    if ds.length > 5 then throw "number_limit"
    let some mag := (String.ofList ds).toNat? | throw "invalid_expression"
    if mag > 4096 then throw "number_limit"
    exponent := if es.head? == some '-' then -(Int.ofNat mag) else Int.ofNat mag
  let chars := parts[0]!.toList
  let negative := chars.head? == some '-'
  let unsigned := if negative then chars.drop 1 else chars
  let decimal := (String.ofList unsigned).splitOn "."
  if decimal.length > 2 then throw "invalid_expression"
  let whole := decimal[0]!
  if whole.isEmpty || !whole.toList.all digit || (whole.length > 1 && whole.toList.head? == some '0') then
    throw "invalid_expression"
  let frac := if decimal.length == 2 then decimal[1]! else ""
  if decimal.length == 2 && (frac.isEmpty || !frac.toList.all digit) then throw "invalid_expression"
  if whole.length + frac.length > 8192 then throw "number_limit"
  let some magnitude := (whole ++ frac).toNat? | throw "invalid_expression"
  let numerator := if negative then -(Int.ofNat magnitude) else Int.ofNat magnitude
  let scale := (Int.ofNat frac.length) - exponent
  if scale >= 0 then return mkRat numerator (10 ^ scale.toNat)
  else return mkRat (numerator * Int.ofNat (10 ^ scale.natAbs)) 1

def parseOp : String → Except String Op
  | "add" => .ok .add | "sub" => .ok .sub | "mul" => .ok .mul | "div" => .ok .div
  | "eq" => .ok .eq | "ne" => .ok .ne | "lt" => .ok .lt | "le" => .ok .le
  | "gt" => .ok .gt | "ge" => .ok .ge | _ => .error "unsupported_capability"

def unavailableCodes : List String :=
  ["missing_reference", "missing_input", "contested", "unavailable_input"]

def parseExpr : Nat → ParseM Expr
  | 0 => throw "parser_depth_limit"
  | fuel + 1 => do
    if (← get).expressions >= 1000000 then throw "expression_limit"
    modify fun st => { st with expressions := st.expressions + 1 }
    match ← takeToken with
    | "n" =>
      match parseNumber (← takeToken) with
      | .ok n => return .literal (.number n)
      | .error code => throw code
    | "b" =>
      match ← takeToken with
      | "0" => return .literal (.boolean false)
      | "1" => return .literal (.boolean true)
      | _ => throw "invalid_expression"
    | "s" => return .literal (.text (← takeText))
    | "z" => return .literal .null
    | "r" => return .ref (← takeText)
    | "u" =>
      let code ← takeToken
      if !unavailableCodes.contains code then throw "invalid_expression"
      return .unavailable code
    | "o" =>
      let op ← match parseOp (← takeToken) with
        | .ok op => pure op
        | .error code => throw code
      if (← takeNat 2) != 2 then throw "invalid_expression"
      let left ← parseExpr fuel
      let right ← parseExpr fuel
      return .binary op left right
    | _ => throw "invalid_expression"

structure Request where
  limits : Limits
  declared : Std.HashSet String
  nodes : Std.HashMap String Expr
  root : Expr

def parseRequest : ParseM Request := do
  if (← takeToken) != "KP2" then throw "invalid_transport"
  let steps ← takeBounded 10000000 "invalid_limits"
  let depth ← takeBounded 4096 "invalid_limits"
  let digits ← takeBounded 4096 "invalid_limits"
  if steps == 0 || depth == 0 || digits == 0 then throw "invalid_limits"
  let declaredCount ← takeBounded 100000 "edge_limit"
  let mut declared : Std.HashSet String := {}
  for _ in [:declaredCount] do
    let id ← takeText
    if declared.contains id then throw "invalid_transport"
    declared := declared.insert id
  let nodeCount ← takeBounded 20000 "node_limit"
  let mut nodes : Std.HashMap String Expr := {}
  for _ in [:nodeCount] do
    let id ← takeText
    if nodes.contains id then throw "invalid_transport"
    let expr ← parseExpr 129
    nodes := nodes.insert id expr
  let root ← parseExpr 129
  if (← get).pos != (← get).tokens.size then throw "invalid_transport"
  return { limits := { steps, depth, digits }, declared, nodes, root }

def sorted (s : Std.HashSet String) : List String := s.toList.mergeSort (· ≤ ·)

def valueTokens : Value → List String
  | .number r => ["n", toString r.num, toString r.den]
  | .boolean b => ["b", if b then "1" else "0"]
  | .text s => ["s", hex s]
  | .null => ["z"]

def response (status : String) (value : Option Value) (potential : Std.HashSet String)
    (preflightSteps : Nat) (st : State) : String :=
  let diagnostics := sorted st.diagnostics
  let potential := sorted potential
  let reads := sorted st.reads
  let counts := st.counts.toList.mergeSort (fun a b => a.1 ≤ b.1)
  String.intercalate "\t" <| ["KR2", status] ++ (value.map valueTokens).getD ["u"] ++
    [toString diagnostics.length] ++ diagnostics ++ [toString potential.length] ++ potential.map hex ++
    [toString reads.length] ++ reads.map hex ++
    [toString st.steps, toString preflightSteps, toString counts.length] ++
    counts.flatMap (fun (id, count) => [hex id, toString count])

def errorStatus (code : String) : String :=
  if code == "unsupported_capability" then "unsupported_capability"
  else if (["step_limit", "depth_limit", "parser_depth_limit", "number_limit", "edge_limit",
            "expression_limit", "transport_limit", "node_limit"] : List String).contains code then "limit"
  else "error"

def errorResponse (code : String) (potential : Std.HashSet String := {})
    (preflightSteps : Nat := 0) : String :=
  response (errorStatus code) none potential preflightSteps
    { diagnostics := ({} : Std.HashSet String).insert code }

def handle (line : String) : String := Id.run do
  if line.utf8ByteSize > 16777216 then return errorResponse "transport_limit"
  if line.contains '\n' || line.contains '\r' then return errorResponse "invalid_transport"
  let (parsed, _) := (parseRequest.run).run { tokens := (line.splitOn "\t").toArray }
  let request ← match parsed with
    | .ok request => pure request
    | .error code => return errorResponse code
  let potential ← match closure 100000 request.nodes (refs request.root) {} with
    | .ok potential => pure potential
    | .error (code, potential) => return errorResponse code potential
  if !(potential.toList.all request.declared.contains) then
    return errorResponse "undeclared_dependency" potential
  let (typed, typeState) :=
    ((inferType (request.limits.depth + 1) request.nodes request.limits request.root).run).run {}
  if let .error code := typed then return errorResponse code potential typeState.visits
  let (result, st) := ((evaluate (request.limits.depth + 1) request.nodes request.limits 0 request.root).run).run {}
  if !(st.reads.toList.all potential.contains) then
    return errorResponse "undeclared_dependency" potential typeState.visits
  match result with
  | .error code =>
    return response "limit" none potential typeState.visits
      { st with diagnostics := st.diagnostics.insert code }
  | .ok (.known value) => return response "ok" (some value) potential typeState.visits st
  | .ok .unknown => return response "unknown" none potential typeState.visits st
  | .ok .error => return response "error" none potential typeState.visits st

end Kpopper
